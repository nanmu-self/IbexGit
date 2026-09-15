//! Line-level patch builder (P4 行级暂存/丢弃/取消暂存).
//!
//! The frontend displays a cached [`DiffModel`] and reports selected line
//! indices back; this module reconstructs a unified patch **from the same
//! model** (PLAN P4: 同源，杜绝双端解析漂移) and the command layer feeds it
//! to `git apply`:
//!
//! - [`LineOp::Stage`]   → `git apply --cached`        (worktree → index)
//! - [`LineOp::Discard`] → `git apply --reverse`       (revert in worktree)
//! - [`LineOp::Unstage`] → `git apply --cached --reverse` (index → HEAD-ward)
//!
//! # Selection semantics
//!
//! Unselected changes must survive untouched, which dictates how each
//! original diff line is re-emitted:
//!
//! | line  | Stage (forward, old=index anchor) | Discard/Unstage (reverse Q, new=current anchor) |
//! |-------|-----------------------------------|--------------------------------------------------|
//! | ctx   | context                           | context                                          |
//! | `+` sel | `+`                             | `+`                                              |
//! | `+` unsel | *skip* (not in old index)     | context (kept in worktree/result)                |
//! | `-` sel | `-`                             | `-`                                              |
//! | `-` unsel | context (kept in index)       | *skip* (removal stays)                           |
//!
//! Both variants keep one side byte-identical to the original diff (the
//! "anchor"): Stage keeps the old side, Discard/Unstage keep the new side —
//! that side reuses the original hunk numbers verbatim; the computed side is
//! renumbered with a running delta of the *selected* changes only
//! (`#sel- − #sel+` per hunk).
//!
//! # `\ No newline at end of file`
//!
//! The marker can only terminate a side. After filtering, a converted
//! context line may end one side without a newline while the other side
//! continues — unified format cannot express that as context, so the line is
//! split into a remove/add pair (`-text` + marker + `+text`), which is
//! exactly what git itself emits for this situation.

use super::{DiffFile, DiffLineKind, LineSelection};
use crate::core::error::AppError;

/// Which direction a line-level patch flows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineOp {
    /// Stage selected worktree lines into the index (`apply --cached`).
    Stage,
    /// Revert selected lines in the worktree (`apply --reverse`).
    Discard,
    /// Remove selected lines from the index (`apply --cached --reverse`).
    Unstage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Emit {
    Context,
    Add,
    Del,
}

struct Emitted {
    kind: Emit,
    text: String,
    /// Original diff flagged this line as the no-trailing-newline end of the
    /// old side (`-` and context lines).
    old_mark: bool,
    /// Same for the new side (`+` and context lines).
    new_mark: bool,
}

/// Build the unified patch for the selected lines of one file.
///
/// Returns `Ok(String::new())` when the selection produces no changes
/// (nothing selected / only context selected) — the caller no-ops.
pub fn build_line_patch(
    file: &DiffFile,
    path: &str,
    sels: &[LineSelection],
    op: LineOp,
) -> Result<String, AppError> {
    if file.binary {
        return Err(AppError::parse(
            "line-level operations are not supported for binary files",
        ));
    }
    // A rename with content changes spans two paths; `git apply` rename
    // patches are out of scope for v1 (UI gates this and offers file ops).
    if file.hunks.is_empty() {
        return Ok(String::new());
    }
    if let (Some(old), Some(new)) = (&file.old_path, &file.new_path) {
        if old != new {
            return Err(AppError::parse(
                "line-level operations are not supported for renamed files",
            ));
        }
    }
    let old_path = file.old_path.as_deref().unwrap_or(path);
    let new_path = file.new_path.as_deref().unwrap_or(path);
    for p in [old_path, new_path] {
        if p.is_empty()
            || p == "/dev/null"
            || p.bytes().any(|b| b == b'\n' || b == b'\r' || b == b'\0')
        {
            return Err(AppError::parse(format!("unsupported diff path: {p:?}")));
        }
    }

    let stage = op == LineOp::Stage;
    let mut bodies: Vec<Vec<Emitted>> = Vec::with_capacity(file.hunks.len());

    for (h, hunk) in file.hunks.iter().enumerate() {
        let h = h as u32;
        // Hunks carrying `\ No newline` markers on changed lines are only
        // safely operable as a whole: the EOL-ness of the last line is
        // entangled with every neighbouring change (e.g. an appended line
        // forces the EOL fix), so partial selections would produce patches
        // whose outcome is ambiguous or impossible. All-or-nothing per hunk.
        let marker_hunk = hunk
            .lines
            .iter()
            .any(|l| l.no_eol && l.kind != DiffLineKind::Context);
        if marker_hunk {
            let changed_idxs: Vec<usize> = hunk
                .lines
                .iter()
                .enumerate()
                .filter(|(_, l)| matches!(l.kind, DiffLineKind::Add | DiffLineKind::Remove))
                .map(|(i, _)| i)
                .collect();
            if !changed_idxs.is_empty() {
                let selected_count = changed_idxs
                    .iter()
                    .filter(|&&i| sels.iter().any(|s| s.hunk == h && s.line == i as u32))
                    .count();
                if selected_count > 0 && selected_count < changed_idxs.len() {
                    return Err(AppError::parse(
                        "this hunk contains no-newline-at-EOF changes; \
                         select all of its lines or none",
                    ));
                }
            }
        }
        let mut body: Vec<Emitted> = Vec::new();
        for (l, line) in hunk.lines.iter().enumerate() {
            let l = l as u32;
            let selected = sels.iter().any(|s| s.hunk == h && s.line == l);
            let emit = match line.kind {
                DiffLineKind::Context => Some(Emit::Context),
                DiffLineKind::Header => None,
                DiffLineKind::Add => {
                    if selected {
                        Some(Emit::Add)
                    } else if stage {
                        None // unselected addition: not in the anchor (index)
                    } else {
                        Some(Emit::Context) // kept in worktree / result index
                    }
                }
                DiffLineKind::Remove => {
                    if selected {
                        Some(Emit::Del)
                    } else if stage {
                        Some(Emit::Context) // kept in index
                    } else {
                        None // unselected removal: absent from anchor side
                    }
                }
            };
            let Some(kind) = emit else { continue };
            // Parse-time `no_eol` sits on the file's genuine last line of a
            // side: `-`/ctx mark the old side, `+`/ctx the new side.
            let (old_mark, new_mark) = match line.kind {
                DiffLineKind::Context => (line.no_eol, line.no_eol),
                DiffLineKind::Remove => (line.no_eol, false),
                DiffLineKind::Add => (false, line.no_eol),
                DiffLineKind::Header => (false, false),
            };
            body.push(Emitted {
                kind,
                text: line.content.clone(),
                old_mark,
                new_mark,
            });
        }
        // A body without any add/del line means nothing was selected in
        // this hunk (context lines are always emitted) — skip it.
        let has_change = body.iter().any(|e| e.kind != Emit::Context);
        if has_change {
            bodies.push(fix_no_eol(body));
        } else {
            bodies.push(Vec::new());
        }
    }

    // Drop hunks whose selection produced nothing, keeping hunk alignment
    // with `file.hunks` for the numbering pass below.
    if bodies.iter().all(|b| b.is_empty()) {
        return Ok(String::new());
    }

    // Numbering: the anchor side reuses the original header values; the
    // computed side is renumbered with a running delta of previous hunks.
    // git's empty-range convention: a count-0 range is numbered by the line
    // BEFORE it (0 = before the first line), hence the ±1 adjustments below
    // (e.g. `@@ -0,0 +1,N @@` for new files, `@@ -1,N +0,0 @@` for deletions).
    // Counts are computed AFTER the no-EOL fix pass (splits change both).
    let mut out = String::new();
    out.push_str(&format!("diff --git a/{old_path} b/{new_path}\n"));
    let mut any_body = false;
    let mut running_delta: i64 = 0;
    for (hunk, body) in file.hunks.iter().zip(&bodies) {
        if body.is_empty() {
            continue;
        }
        any_body = true;
        let count = |kind: Emit| -> u32 { body.iter().filter(|e| e.kind == kind).count() as u32 };
        let (ctx, add, del) = (count(Emit::Context), count(Emit::Add), count(Emit::Del));
        let (anchor_start, anchor_count, computed_count) = if stage {
            // Anchor = old side (index): original numbers.
            (hunk.old_start, ctx + del, ctx + add)
        } else {
            // Anchor = new side (worktree / result index): original numbers.
            (hunk.new_start, ctx + add, ctx + del)
        };
        let adjust: i64 = match (anchor_count > 0, computed_count > 0) {
            (true, true) => 0,
            (true, false) => -1,
            (false, true) => 1,
            (false, false) => 0,
        };
        let computed_start = (anchor_start as i64 + running_delta + adjust).max(0) as u32;
        running_delta += computed_count as i64 - anchor_count as i64;
        let (old_start, new_start, old_count, new_count) = if stage {
            (anchor_start, computed_start, anchor_count, computed_count)
        } else {
            (computed_start, anchor_start, computed_count, anchor_count)
        };
        out.push_str(&format!(
            "@@ -{} +{} @@\n",
            format_range(old_start, old_count),
            format_range(new_start, new_count)
        ));
        // Find the final marker positions for this hunk's body.
        let last_old = body
            .iter()
            .rposition(|e| e.kind == Emit::Context || e.kind == Emit::Del);
        let last_new = body
            .iter()
            .rposition(|e| e.kind == Emit::Context || e.kind == Emit::Add);
        let old_no_eol = last_old.map(|i| body[i].old_mark).unwrap_or(false);
        let new_no_eol = last_new.map(|i| body[i].new_mark).unwrap_or(false);

        for (i, e) in body.iter().enumerate() {
            let prefix = match e.kind {
                Emit::Context => ' ',
                Emit::Add => '+',
                Emit::Del => '-',
            };
            out.push(prefix);
            out.push_str(&e.text);
            out.push('\n');
            let ends_old = Some(i) == last_old && old_no_eol;
            let ends_new = Some(i) == last_new && new_no_eol;
            if ends_old || ends_new {
                out.push_str("\\ No newline at end of file\n");
            }
        }
    }

    if !any_body {
        // No hunk bodies survived — nothing to apply.
        return Ok(String::new());
    }

    // File headers with correct /dev/null sides and mode lines (git apply
    // requires `new file mode`/`deleted file mode` to treat a /dev/null side
    // as creation/deletion rather than a path lookup):
    // - old side is /dev/null iff the diff itself is a new file AND the
    //   computed old side is empty (nothing context-kept);
    // - new side is /dev/null iff the diff is a deletion AND the computed
    //   new side is empty (every `-` line selected → full deletion); a
    //   partially kept deletion keeps `+++ b/path`.
    let old_side_empty = file.hunks.iter().zip(&bodies).all(|(_, b)| {
        !b.iter()
            .any(|e| e.kind == Emit::Context || e.kind == Emit::Del)
    });
    let new_side_empty = file.hunks.iter().zip(&bodies).all(|(_, b)| {
        !b.iter()
            .any(|e| e.kind == Emit::Context || e.kind == Emit::Add)
    });
    let old_devnull = file.old_path.is_none() && old_side_empty;
    let new_devnull = file.new_path.is_none() && new_side_empty;
    let mut headers = String::new();
    if old_devnull {
        headers.push_str("new file mode 100644\n");
    }
    if new_devnull {
        headers.push_str("deleted file mode 100644\n");
    }
    let old_header = if old_devnull {
        "--- /dev/null\n".to_string()
    } else {
        format!("--- a/{old_path}\n")
    };
    let new_header = if new_devnull {
        "+++ /dev/null\n".to_string()
    } else {
        format!("+++ b/{new_path}\n")
    };
    // Splice headers in after the `diff --git` line.
    if let Some(pos) = out.find('\n') {
        out.insert_str(pos + 1, &format!("{headers}{old_header}{new_header}"));
    }
    Ok(out)
}

/// Split context lines whose no-EOL-ness no longer matches between the two
/// sides after filtering (see module docs). Runs once per hunk body.
fn fix_no_eol(body: Vec<Emitted>) -> Vec<Emitted> {
    let mut body = body;
    let last_old = body
        .iter()
        .rposition(|e| e.kind == Emit::Context || e.kind == Emit::Del);
    let last_new = body
        .iter()
        .rposition(|e| e.kind == Emit::Context || e.kind == Emit::Add);
    let Some(ctx_idx) = last_old.filter(|i| body[*i].kind == Emit::Context) else {
        return body;
    };
    let ctx_is_last_new = last_new == Some(ctx_idx);
    if ctx_is_last_new {
        return body; // context ends both sides: single marker, no split
    }
    // Old side ends at this context (with no EOL) but the new side continues:
    // split into `-text` + `\ No newline` + `+text` (the add copy gets an EOL
    // because more new-side lines follow).
    if body[ctx_idx].old_mark {
        let text = body[ctx_idx].text.clone();
        body[ctx_idx] = Emitted {
            kind: Emit::Del,
            text: text.clone(),
            old_mark: true,
            new_mark: false,
        };
        body.insert(
            ctx_idx + 1,
            Emitted {
                kind: Emit::Add,
                text,
                old_mark: false,
                new_mark: false,
            },
        );
        return body;
    }
    // Mirror case: the new side ends at this context without an EOL while the
    // old side continues (`-text` plain, `+text` + marker).
    let last_old_after = body
        .iter()
        .rposition(|e| e.kind == Emit::Context || e.kind == Emit::Del);
    if last_new == Some(ctx_idx)
        && body[ctx_idx].new_mark
        && last_old_after.is_some()
        && last_old_after != Some(ctx_idx)
    {
        let text = body[ctx_idx].text.clone();
        body[ctx_idx] = Emitted {
            kind: Emit::Del,
            text: text.clone(),
            old_mark: false,
            new_mark: false,
        };
        body.insert(
            ctx_idx + 1,
            Emitted {
                kind: Emit::Add,
                text,
                old_mark: false,
                new_mark: true,
            },
        );
    }
    body
}

/// `1` for count 1 (git's compact form), `start,count` otherwise.
fn format_range(start: u32, count: u32) -> String {
    if count == 1 {
        start.to_string()
    } else {
        format!("{start},{count}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::engine::{DiffHunk, DiffLine, DiffModel, DiffSource};

    fn line(kind: DiffLineKind, content: &str, left: Option<u32>, right: Option<u32>) -> DiffLine {
        DiffLine {
            content: content.into(),
            left_no: left,
            right_no: right,
            kind,
            no_eol: false,
        }
    }

    fn hunk(
        old_start: u32,
        old_count: u32,
        new_start: u32,
        new_count: u32,
        lines: Vec<DiffLine>,
    ) -> DiffHunk {
        DiffHunk {
            old_start,
            old_count,
            new_start,
            new_count,
            header: String::new(),
            lines,
        }
    }

    fn file(hunks: Vec<DiffHunk>) -> DiffFile {
        DiffFile {
            old_path: Some("f.txt".into()),
            new_path: Some("f.txt".into()),
            similarity: None,
            binary: false,
            hunks,
        }
    }

    fn sels(pairs: &[(u32, u32)]) -> Vec<LineSelection> {
        pairs
            .iter()
            .map(|&(hunk, line)| LineSelection { hunk, line })
            .collect()
    }

    #[test]
    fn stage_subset_of_replace_hunk() {
        // ctx / -a / -b / +A / +B / ctx — stage only `+A`.
        let f = file(vec![hunk(
            1,
            4,
            1,
            4,
            vec![
                line(DiffLineKind::Context, "top", Some(1), Some(1)),
                line(DiffLineKind::Remove, "a", Some(2), None),
                line(DiffLineKind::Remove, "b", Some(3), None),
                line(DiffLineKind::Add, "A", None, Some(2)),
                line(DiffLineKind::Add, "B", None, Some(3)),
                line(DiffLineKind::Context, "bot", Some(4), Some(4)),
            ],
        )]);
        let patch = build_line_patch(&f, "f.txt", &sels(&[(0, 3)]), LineOp::Stage).unwrap();
        // Unselected `+B` drops out; unselected `-a`/`-b` become context.
        // New index region: top, a, b, A, bot (staging only adds A).
        assert_eq!(
            patch,
            "diff --git a/f.txt b/f.txt\n\
             --- a/f.txt\n\
             +++ b/f.txt\n\
             @@ -1,4 +1,5 @@\n\
             \x20top\n\
             \x20a\n\
             \x20b\n\
             +A\n\
             \x20bot\n"
        );
    }

    #[test]
    fn stage_selected_removal_keeps_unselected_additions_out() {
        let f = file(vec![hunk(
            1,
            3,
            1,
            2,
            vec![
                line(DiffLineKind::Context, "top", Some(1), Some(1)),
                line(DiffLineKind::Remove, "old", Some(2), None),
                line(DiffLineKind::Add, "new1", None, Some(2)),
                line(DiffLineKind::Add, "new2", None, Some(3)),
            ],
        )]);
        // Stage only the removal: index drops `old`, gains nothing.
        let patch = build_line_patch(&f, "f.txt", &sels(&[(0, 1)]), LineOp::Stage).unwrap();
        assert_eq!(
            patch,
            "diff --git a/f.txt b/f.txt\n\
             --- a/f.txt\n\
             +++ b/f.txt\n\
             @@ -1,2 +1 @@\n\
             \x20top\n\
             -old\n"
        );
    }

    #[test]
    fn discard_subset_anchors_new_side() {
        // Same hunk; discard only `+new1`. Reverse patch Q:
        // old side (W') = ctx, new2; new side (worktree) = ctx, new1, new2.
        let f = file(vec![hunk(
            1,
            2,
            1,
            3,
            vec![
                line(DiffLineKind::Context, "top", Some(1), Some(1)),
                line(DiffLineKind::Remove, "old", Some(2), None),
                line(DiffLineKind::Add, "new1", None, Some(2)),
                line(DiffLineKind::Add, "new2", None, Some(3)),
            ],
        )]);
        let patch = build_line_patch(&f, "f.txt", &sels(&[(0, 2)]), LineOp::Discard).unwrap();
        assert_eq!(
            patch,
            "diff --git a/f.txt b/f.txt\n\
             --- a/f.txt\n\
             +++ b/f.txt\n\
             @@ -1,2 +1,3 @@\n\
             \x20top\n\
             +new1\n\
             \x20new2\n"
        );
    }

    #[test]
    fn unstage_matches_discard_emission() {
        let f = file(vec![hunk(
            1,
            2,
            1,
            3,
            vec![
                line(DiffLineKind::Context, "top", Some(1), Some(1)),
                line(DiffLineKind::Remove, "old", Some(2), None),
                line(DiffLineKind::Add, "new1", None, Some(2)),
                line(DiffLineKind::Add, "new2", None, Some(3)),
            ],
        )]);
        let discard = build_line_patch(&f, "f.txt", &sels(&[(0, 2)]), LineOp::Discard).unwrap();
        let unstage = build_line_patch(&f, "f.txt", &sels(&[(0, 2)]), LineOp::Unstage).unwrap();
        assert_eq!(discard, unstage);
    }

    #[test]
    fn multi_hunk_running_delta() {
        // Two hunks; select one add in each.
        let f = file(vec![
            hunk(
                1,
                3,
                1,
                4,
                vec![
                    line(DiffLineKind::Context, "a", Some(1), Some(1)),
                    line(DiffLineKind::Add, "X", None, Some(2)),
                    line(DiffLineKind::Context, "b", Some(2), Some(3)),
                    line(DiffLineKind::Context, "c", Some(3), Some(4)),
                ],
            ),
            hunk(
                10,
                2,
                11,
                3,
                vec![
                    line(DiffLineKind::Context, "d", Some(10), Some(11)),
                    line(DiffLineKind::Add, "Y", None, Some(12)),
                    line(DiffLineKind::Context, "e", Some(11), Some(13)),
                ],
            ),
        ]);
        let patch = build_line_patch(&f, "f.txt", &sels(&[(0, 1), (1, 1)]), LineOp::Stage).unwrap();
        assert!(patch.contains("@@ -1,3 +1,4 @@\n"));
        // Second hunk's new start shifts by +1 (the staged X line above).
        assert!(patch.contains("@@ -10,2 +11,3 @@\n"), "{patch}");
    }

    #[test]
    fn new_file_partial_stage_uses_devnull_old_side() {
        let mut f = file(vec![hunk(
            0,
            0,
            1,
            3,
            vec![
                line(DiffLineKind::Add, "one", None, Some(1)),
                line(DiffLineKind::Add, "two", None, Some(2)),
                line(DiffLineKind::Add, "three", None, Some(3)),
            ],
        )]);
        f.old_path = None;
        f.new_path = Some("new.txt".into());
        let patch =
            build_line_patch(&f, "new.txt", &sels(&[(0, 0), (0, 2)]), LineOp::Stage).unwrap();
        assert!(patch.contains("--- /dev/null\n"), "{patch}");
        assert!(patch.contains("+++ b/new.txt\n"));
        assert!(patch.contains("@@ -0,0 +1,2 @@\n"));
        assert!(patch.contains("+one\n"));
        assert!(!patch.contains("two"));
        assert!(patch.contains("+three\n"));
    }

    #[test]
    fn deleted_file_fully_staged_uses_devnull_new_side() {
        let mut f = file(vec![hunk(
            1,
            2,
            0,
            0,
            vec![
                line(DiffLineKind::Remove, "a", Some(1), None),
                line(DiffLineKind::Remove, "b", Some(2), None),
            ],
        )]);
        f.new_path = None;
        let patch = build_line_patch(&f, "f.txt", &sels(&[(0, 0), (0, 1)]), LineOp::Stage).unwrap();
        assert!(patch.contains("--- a/f.txt\n"), "{patch}");
        assert!(patch.contains("+++ /dev/null\n"));
        assert!(patch.contains("-a\n") && patch.contains("-b\n"));
    }

    #[test]
    fn deleted_file_partially_staged_keeps_b_side() {
        let mut f = file(vec![hunk(
            1,
            2,
            0,
            0,
            vec![
                line(DiffLineKind::Remove, "a", Some(1), None),
                line(DiffLineKind::Remove, "b", Some(2), None),
            ],
        )]);
        f.new_path = None;
        let patch = build_line_patch(&f, "f.txt", &sels(&[(0, 0)]), LineOp::Stage).unwrap();
        assert!(patch.contains("@@ -1,2 +1 @@\n"), "{patch}");
        assert!(patch.contains("-a\n") && !patch.contains("-b\n"));
    }

    #[test]
    fn no_newline_hunk_verbatim_when_fully_selected() {
        // index: "last" (no EOL); worktree: last\nnew.
        // Marker hunks are all-or-nothing; selecting every changed line
        // reproduces the original hunk verbatim.
        let f = file(vec![hunk(
            1,
            1,
            1,
            2,
            vec![
                DiffLine {
                    no_eol: true,
                    ..line(DiffLineKind::Remove, "last", Some(1), None)
                },
                line(DiffLineKind::Add, "last", None, Some(1)),
                line(DiffLineKind::Add, "new", None, Some(2)),
            ],
        )]);
        // Partial selections are refused: the EOL-ness of the last line is
        // entangled with its neighbouring lines, so the outcome would be
        // ambiguous (e.g. staging `+last` alone would duplicate the line).
        assert!(build_line_patch(&f, "f.txt", &sels(&[(0, 2)]), LineOp::Stage).is_err());
        assert!(build_line_patch(&f, "f.txt", &sels(&[(0, 1)]), LineOp::Stage).is_err());

        // Full selection (including the removal) → verbatim hunk.
        let patch =
            build_line_patch(&f, "f.txt", &sels(&[(0, 0), (0, 1), (0, 2)]), LineOp::Stage).unwrap();
        assert_eq!(
            patch,
            "diff --git a/f.txt b/f.txt\n\
             --- a/f.txt\n\
             +++ b/f.txt\n\
             @@ -1 +1,2 @@\n\
             -last\n\
             \\ No newline at end of file\n\
             +last\n\
             +new\n"
        );
    }

    #[test]
    fn no_newline_discard_all_restores_index_bytes() {
        // worktree file ends without EOL: -a / +b(marker). Discard both
        // changed lines (all-or-nothing) restores the worktree to "a\n".
        let f = file(vec![hunk(
            1,
            1,
            1,
            1,
            vec![
                line(DiffLineKind::Remove, "a", Some(1), None),
                DiffLine {
                    no_eol: true,
                    ..line(DiffLineKind::Add, "b", None, Some(1))
                },
            ],
        )]);
        // Partial selection (only +b) is refused.
        assert!(build_line_patch(&f, "f.txt", &sels(&[(0, 1)]), LineOp::Discard).is_err());
        let patch =
            build_line_patch(&f, "f.txt", &sels(&[(0, 0), (0, 1)]), LineOp::Discard).unwrap();
        // Q = verbatim hunk; reverse-applied it restores "a\n".
        assert_eq!(
            patch,
            "diff --git a/f.txt b/f.txt\n\
             --- a/f.txt\n\
             +++ b/f.txt\n\
             @@ -1 +1 @@\n\
             -a\n\
             +b\n\
             \\ No newline at end of file\n"
        );
    }

    #[test]
    fn context_no_eol_kept_on_both_sides() {
        // ctx "end" marked (both sides end without newline); nothing after.
        // Discard the removal above it → ctx stays last on both sides.
        let f = file(vec![hunk(
            1,
            2,
            1,
            1,
            vec![
                line(DiffLineKind::Remove, "gone", Some(1), None),
                DiffLine {
                    no_eol: true,
                    ..line(DiffLineKind::Context, "end", Some(2), Some(2))
                },
            ],
        )]);
        let patch = build_line_patch(&f, "f.txt", &sels(&[(0, 0)]), LineOp::Discard).unwrap();
        assert_eq!(
            patch,
            "diff --git a/f.txt b/f.txt\n\
             --- a/f.txt\n\
             +++ b/f.txt\n\
             @@ -1,2 +1 @@\n\
             -gone\n\
             \x20end\n\
             \\ No newline at end of file\n"
        );
    }

    #[test]
    fn first_line_of_hunk_selectable_alone() {
        // Hunk starts with a removal (no leading context): selecting only the
        // first `-` must produce a valid patch (PLAN P4 边界: hunk 首行).
        let f = file(vec![hunk(
            1,
            2,
            1,
            2,
            vec![
                line(DiffLineKind::Remove, "one", Some(1), None),
                line(DiffLineKind::Add, "ONE", None, Some(1)),
                line(DiffLineKind::Remove, "two", Some(2), None),
                line(DiffLineKind::Add, "TWO", None, Some(2)),
            ],
        )]);
        let patch = build_line_patch(&f, "f.txt", &sels(&[(0, 0)]), LineOp::Stage).unwrap();
        assert!(patch.contains("@@ -1,2 +1 @@\n"), "{patch}");
        assert!(patch.contains("-one\n"));
        assert!(patch.contains("\x20two\n"));
        assert!(!patch.contains("+ONE"));
        assert!(!patch.contains("+TWO"));
    }

    #[test]
    fn empty_selection_and_context_only_are_noops() {
        let f = file(vec![hunk(
            1,
            2,
            1,
            2,
            vec![
                line(DiffLineKind::Context, "a", Some(1), Some(1)),
                line(DiffLineKind::Add, "b", None, Some(2)),
            ],
        )]);
        assert_eq!(
            build_line_patch(&f, "f.txt", &[], LineOp::Stage).unwrap(),
            ""
        );
        // Context-only selection ignores context lines.
        assert_eq!(
            build_line_patch(&f, "f.txt", &sels(&[(0, 0)]), LineOp::Stage).unwrap(),
            ""
        );
    }

    #[test]
    fn binary_and_rename_rejected() {
        let mut f = file(vec![hunk(
            1,
            1,
            1,
            1,
            vec![line(DiffLineKind::Add, "x", None, Some(1))],
        )]);
        f.binary = true;
        assert!(build_line_patch(&f, "f", &sels(&[(0, 0)]), LineOp::Stage).is_err());
        let mut f = file(vec![hunk(
            1,
            1,
            1,
            1,
            vec![line(DiffLineKind::Add, "x", None, Some(1))],
        )]);
        f.old_path = Some("old.txt".into());
        f.new_path = Some("new.txt".into());
        assert!(build_line_patch(&f, "new.txt", &sels(&[(0, 0)]), LineOp::Stage).is_err());
    }

    #[test]
    fn out_of_range_selection_errors() {
        // Out-of-range selections must not panic; unknown indices are simply
        // never matched, so the result is an empty (no-op) patch.
        let f = file(vec![hunk(
            1,
            1,
            1,
            1,
            vec![line(DiffLineKind::Add, "x", None, Some(1))],
        )]);
        let patch = build_line_patch(&f, "f.txt", &sels(&[(9, 9)]), LineOp::Stage).unwrap();
        assert_eq!(patch, "");
    }

    #[test]
    fn model_helper_finds_files() {
        let model = DiffModel {
            id: 7,
            source: DiffSource::Worktree,
            old_revision: None,
            new_revision: None,
            files: vec![file(vec![])],
        };
        assert!(model.find_file("f.txt").is_some());
        assert!(model.find_file("nope").is_none());
    }
}
