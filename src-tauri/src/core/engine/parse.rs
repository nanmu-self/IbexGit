//! Git output parsers.
//!
//! All parsers are pure functions over raw git output, unit-testable without
//! spawning processes (P1 acceptance: full parser unit-test coverage).

use super::{
    BranchInfo, CommitInfo, DiffFile, DiffHunk, DiffLine, DiffLineKind, DiffModel, DiffSource,
    FileStatus, IndexEntry, ReflogEntry, RemoteInfo, StashEntry, TagInfo,
};
use crate::core::error::AppError;

// =====================
// status --porcelain=v2 -z
// =====================

/// Parse `git status --porcelain=v2 -z` output.
///
/// Entry grammar (NUL-terminated):
/// - `1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>`            ordinary change
/// - `2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path>` rename/copy,
///   followed by a second NUL-separated field `<origPath>`
/// - `u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>`   unmerged
/// - `? <path>`                                                 untracked
/// - `! <path>`                                                 ignored (skipped here)
pub fn parse_status(raw: &str) -> Vec<FileStatus> {
    let mut out = Vec::new();
    let parts: Vec<&str> = raw.split('\0').collect();
    let mut i = 0;
    while i < parts.len() {
        let rec = parts[i];
        if rec.is_empty() {
            i += 1;
            continue;
        }
        // Take the record and, for renames, the following origPath element.
        if let Some(rest) = rec.strip_prefix("2 ") {
            let orig = parts.get(i + 1).copied().unwrap_or("");
            i += 2;
            if let Some(fs) = parse_v2_rename(rest, orig) {
                out.push(fs);
            }
            continue;
        }
        i += 1;
        match rec.as_bytes().first() {
            Some(b'1') => {
                if let Some(fs) = parse_v2_ordinary(rec) {
                    out.push(fs);
                }
            }
            Some(b'u') => {
                if let Some(fs) = parse_v2_unmerged(rec) {
                    out.push(fs);
                }
            }
            Some(b'?') => {
                let path = rec[2..].to_string();
                if !path.is_empty() {
                    out.push(FileStatus {
                        path,
                        status: "?".into(),
                        orig_path: None,
                        submodule: false,
                        submodule_dirty: false,
                        submodule_commit_changed: false,
                        eol_only: false,
                        staged: false,
                        unstaged: false,
                        untracked: true,
                        skipped: false,
                        conflict: false,
                    });
                }
            }
            // `!` ignored entries and anything else are skipped.
            _ => {}
        }
    }
    out
}

fn parse_v2_ordinary(rec: &str) -> Option<FileStatus> {
    let rest = rec.strip_prefix("1 ")?;
    let mut it = rest.splitn(9, ' ');
    let xy = it.next()?;
    let sub = it.next()?;
    let _mh = it.next()?;
    let _mi = it.next()?;
    let _mw = it.next()?;
    let _hh = it.next()?;
    let _hi = it.next()?;
    let path = it.next()?.to_string();
    Some(build_file_status(xy, sub, path, None, false))
}

fn parse_v2_rename(rest: &str, orig: &str) -> Option<FileStatus> {
    let mut it = rest.splitn(10, ' ');
    let xy = it.next()?;
    let sub = it.next()?;
    let _mh = it.next()?;
    let _mi = it.next()?;
    let _mw = it.next()?;
    let _hh = it.next()?;
    let _hi = it.next()?;
    let _score = it.next()?; // e.g. "R100"
    let path = it.next()?.to_string();
    let orig_path = if orig.is_empty() {
        None
    } else {
        Some(orig.to_string())
    };
    Some(build_file_status(xy, sub, path, orig_path, false))
}

fn parse_v2_unmerged(rec: &str) -> Option<FileStatus> {
    let rest = rec.strip_prefix("u ")?;
    let mut it = rest.splitn(11, ' ');
    let xy = it.next()?;
    let sub = it.next()?;
    let _m1 = it.next()?;
    let _m2 = it.next()?;
    let _m3 = it.next()?;
    let _mw = it.next()?;
    let _h1 = it.next()?;
    let _h2 = it.next()?;
    let _h3 = it.next()?;
    let path = it.next()?.to_string();
    Some(build_file_status(xy, sub, path, None, true))
}

fn build_file_status(
    xy: &str,
    sub: &str,
    path: String,
    orig_path: Option<String>,
    conflict: bool,
) -> FileStatus {
    let x = xy.as_bytes().first().copied().unwrap_or(b'.') as char;
    let y = xy.as_bytes().get(1).copied().unwrap_or(b'.') as char;
    // Porcelain v2 `<sub>` field (git ≥2.40, verified on 2.54):
    // "N..." = not a submodule; "SC.." = submodule with a different
    // checked-in commit; "S..U"/"S.M." = submodule with untracked/modified
    // content. First char N/S decides, C/M/U flags carry the details.
    let sub_first = sub.as_bytes().first().copied().unwrap_or(b'N') as char;
    let submodule = sub_first == 'S';
    FileStatus {
        path,
        status: xy.to_string(),
        orig_path,
        submodule,
        submodule_dirty: submodule && (sub.contains('M') || sub.contains('U')),
        submodule_commit_changed: submodule && sub.contains('C'),
        eol_only: false,
        staged: conflict || (x != '.' && x != '?'),
        unstaged: conflict || (y != '.' && y != '?'),
        untracked: false,
        skipped: x == 'S' || y == 'S',
        conflict,
    }
}

// =====================
// ls-files -s -z
// =====================

/// Parse `git ls-files -s -z` output: `<mode> <sha> <stage>\t<path>` NUL-
/// separated records. Unmerged paths appear once per stage (1..3).
pub fn parse_ls_files(raw: &str) -> Vec<IndexEntry> {
    let mut out = Vec::new();
    for rec in raw.split('\0') {
        if rec.is_empty() {
            continue;
        }
        // mode SP sha SP stage TAB path
        let (meta, path) = match rec.split_once('\t') {
            Some(pair) => pair,
            None => continue,
        };
        let mut it = meta.split_whitespace();
        let (Some(mode), Some(sha), Some(stage)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let Ok(stage) = stage.parse::<u32>() else {
            continue;
        };
        if path.is_empty() {
            continue;
        }
        out.push(IndexEntry {
            path: path.to_string(),
            mode: mode.to_string(),
            sha: sha.to_string(),
            stage,
        });
    }
    out
}

// =====================
// diff --numstat -z (path extraction)
// =====================

/// Extract the changed paths from `git diff --numstat -z` output.
///
/// Record layout (verified on git 2.54):
/// - ordinary: `<added>\t<removed>\t<path>\0` (TAB-separated, NUL-terminated)
/// - binary: counts are `-`
/// - rename/copy: `<added>\t<removed>\t\0<origPath>\0<path>\0` — the path
///   slot is empty and the two names follow as separate NUL fields.
pub fn parse_numstat_paths(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let parts: Vec<&str> = raw.split('\0').collect();
    let mut i = 0;
    while i < parts.len() {
        let fields: Vec<&str> = parts[i].split('\t').collect();
        if fields.len() < 2 {
            i += 1;
            continue;
        }
        if fields.len() >= 3 && !fields[2].is_empty() {
            out.push(fields[2].to_string());
            i += 1;
        } else {
            // Rename layout: `<a>\t<r>\t` + `\0<origPath>\0<path>\0`.
            if let Some(path) = parts.get(i + 2) {
                if !path.is_empty() {
                    out.push(path.to_string());
                }
            }
            i += 3;
        }
    }
    out
}

// =====================
// diff (unified)
// =====================

/// Parse unified `git diff` output into a [`DiffModel`].
///
/// Handles: add/delete/modify, rename & copy headers (`-M`), similarity index,
/// binary files, `\ No newline at end of file`.
///
/// Hunk bodies are consumed by remaining-line counters, so body lines that
/// happen to start with `---` / `+++` / `@@` (e.g. removed Markdown separators)
/// are never mistaken for file headers.
pub fn parse_diff(
    output: &str,
    source: DiffSource,
    rev_range: Option<(&str, &str)>,
) -> Result<DiffModel, AppError> {
    let mut files: Vec<DiffFile> = Vec::new();
    let mut cur: Option<DiffFile> = None;
    let mut cur_hunk: Option<DiffHunk> = None;
    // Remaining (old, new) body lines of the hunk currently being consumed.
    let mut hunk_left = (0u32, 0u32);
    // Running line numbers for the hunk being consumed; hunk header fields
    // keep their original values from the `@@` header.
    let mut line_no = (0u32, 0u32);

    let flush_hunk = |hunk: &mut Option<DiffHunk>| -> Option<DiffHunk> { hunk.take() };

    for line in output.lines() {
        // Inside a hunk: every line belongs to the body.
        if hunk_left.0 > 0 || hunk_left.1 > 0 {
            let Some(hunk) = cur_hunk.as_mut() else {
                hunk_left = (0, 0);
                continue;
            };
            if let Some(rest) = line.strip_prefix(' ') {
                hunk.lines.push(DiffLine {
                    content: rest.to_string(),
                    left_no: Some(line_no.0),
                    right_no: Some(line_no.1),
                    kind: DiffLineKind::Context,
                });
                line_no = (line_no.0 + 1, line_no.1 + 1);
                hunk_left.0 = hunk_left.0.saturating_sub(1);
                hunk_left.1 = hunk_left.1.saturating_sub(1);
            } else if let Some(rest) = line.strip_prefix('+') {
                hunk.lines.push(DiffLine {
                    content: rest.to_string(),
                    left_no: None,
                    right_no: Some(line_no.1),
                    kind: DiffLineKind::Add,
                });
                line_no.1 += 1;
                hunk_left.1 = hunk_left.1.saturating_sub(1);
            } else if let Some(rest) = line.strip_prefix('-') {
                hunk.lines.push(DiffLine {
                    content: rest.to_string(),
                    left_no: Some(line_no.0),
                    right_no: None,
                    kind: DiffLineKind::Remove,
                });
                line_no.0 += 1;
                hunk_left.0 = hunk_left.0.saturating_sub(1);
            }
            // `\ No newline at end of file` and malformed lines consume nothing.
            continue;
        }

        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(h) = flush_hunk(&mut cur_hunk) {
                if let Some(f) = cur.as_mut() {
                    f.hunks.push(h);
                }
            }
            if let Some(prev) = cur.take() {
                files.push(prev);
            }
            let (op, np) = split_git_paths(rest);
            cur = Some(DiffFile {
                old_path: op,
                new_path: np,
                similarity: None,
                binary: false,
                hunks: Vec::new(),
            });
            continue;
        }
        let Some(file) = cur.as_mut() else {
            continue; // preamble lines before first "diff --git"
        };
        if let Some(sim) = line.strip_prefix("similarity index ") {
            let pct = sim.trim_end_matches('%').trim().parse().unwrap_or(0);
            file.similarity = Some(pct);
        } else if let Some(p) = line.strip_prefix("rename from ") {
            file.old_path = Some(unquote_path(p));
        } else if let Some(p) = line.strip_prefix("rename to ") {
            file.new_path = Some(unquote_path(p));
        } else if let Some(p) = line.strip_prefix("copy from ") {
            file.old_path = Some(unquote_path(p));
        } else if let Some(p) = line.strip_prefix("copy to ") {
            file.new_path = Some(unquote_path(p));
        } else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            file.binary = true;
        } else if let Some(p) = line.strip_prefix("--- ") {
            let p = p.trim();
            file.old_path = decode_ab_path(p);
        } else if let Some(p) = line.strip_prefix("+++ ") {
            let p = p.trim();
            file.new_path = decode_ab_path(p);
        } else if let Some(h) = parse_hunk_header(line) {
            hunk_left = (h.old_count, h.new_count);
            line_no = (h.old_start, h.new_start);
            if let Some(prev) = flush_hunk(&mut cur_hunk) {
                file.hunks.push(prev);
            }
            cur_hunk = Some(h);
        }
    }
    if let Some(h) = flush_hunk(&mut cur_hunk) {
        if let Some(f) = cur.as_mut() {
            f.hunks.push(h);
        }
    }
    if let Some(last) = cur.take() {
        files.push(last);
    }

    Ok(DiffModel {
        source,
        old_revision: rev_range.map(|(a, _)| a.to_string()),
        new_revision: rev_range.map(|(_, b)| b.to_string()),
        files,
    })
}

/// `a/old b/new` after `diff --git ` — only a fallback; ---/+++ lines are authoritative.
fn split_git_paths(rest: &str) -> (Option<String>, Option<String>) {
    if let Some(a) = rest.strip_prefix("a/") {
        // Find ` b/` separator at top level (paths may contain spaces).
        if let Some(idx) = a.find(" b/") {
            let old = &a[..idx];
            let new = a[idx + 3..].strip_prefix("b/").unwrap_or(&a[idx + 3..]);
            return (Some(old.to_string()), Some(new.to_string()));
        }
    }
    (None, None)
}

/// `--- a/path` / `+++ b/path` / `--- /dev/null` → Option<path>.
/// These lines are authoritative — they override the `diff --git` fallback.
fn decode_ab_path(s: &str) -> Option<String> {
    if s == "/dev/null" {
        return None;
    }
    let p = unquote_path(s);
    let p = p
        .strip_prefix("a/")
        .or_else(|| p.strip_prefix("b/"))
        .unwrap_or(&p);
    Some(p.to_string())
}

/// Undo git's C-style path quoting (`"..."` with backslash escapes / octal).
/// Octal escapes are raw UTF-8 bytes, so decode at the byte level.
fn unquote_path(p: &str) -> String {
    if !p.starts_with('"') || !p.ends_with('"') || p.len() < 2 {
        return p.to_string();
    }
    let inner = &p[1..p.len() - 1];
    let mut out: Vec<u8> = Vec::with_capacity(inner.len());
    let bytes = inner.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 1;
            match bytes[i] {
                b'n' => out.push(b'\n'),
                b't' => out.push(b'\t'),
                b'r' => out.push(b'\r'),
                b'"' => out.push(b'"'),
                b'\\' => out.push(b'\\'),
                b'0'..=b'7' => {
                    // Up to 3 octal digits.
                    let start = i;
                    let mut end = i;
                    while end < bytes.len()
                        && end < start + 3
                        && (b'0'..=b'7').contains(&bytes[end])
                    {
                        end += 1;
                    }
                    let val = u8::from_str_radix(&inner[start..end], 8).unwrap_or(b'?');
                    out.push(val);
                    i = end - 1;
                }
                c => out.push(c),
            }
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `@@ -l,c +l,c @@ ...` → DiffHunk.
fn parse_hunk_header(line: &str) -> Option<DiffHunk> {
    let rest = line.strip_prefix("@@ ")?;
    let end = rest.find(" @@")?;
    let ranges = &rest[..end];
    let header = rest[end + 3..].trim().to_string();
    let mut parts = ranges.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let (os, oc) = parse_range(old);
    let (ns, nc) = parse_range(new);
    Some(DiffHunk {
        old_start: os,
        old_count: oc,
        new_start: ns,
        new_count: nc,
        header,
        lines: Vec::new(),
    })
}

fn parse_range(s: &str) -> (u32, u32) {
    match s.split_once(',') {
        Some((a, b)) => (a.parse().unwrap_or(0), b.parse().unwrap_or(0)),
        None => (s.parse().unwrap_or(0), 1),
    }
}

// =====================
// log
// =====================

/// Parse `git log --format=%H%n%h%n%an%n%ae%n%aI%n%s%n%d%n%P` output:
/// 8 lines per commit, records are back-to-back.
pub fn parse_log(output: &str) -> Vec<CommitInfo> {
    let lines: Vec<&str> = output.lines().collect();
    let mut commits = Vec::new();
    let mut i = 0;
    while i + 8 <= lines.len() {
        let hash = lines[i].to_string();
        if hash.is_empty() {
            // Trailing garbage / blank separators — stop.
            break;
        }
        let short_hash = lines[i + 1].to_string();
        let author = lines[i + 2].to_string();
        let email = lines[i + 3].to_string();
        let date = lines[i + 4].to_string();
        let message = lines[i + 5].to_string();
        let refs = lines[i + 6]
            .trim_matches(|c| c == '(' || c == ')')
            .split(", ")
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let parents: Vec<String> = lines[i + 7]
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        commits.push(CommitInfo {
            hash,
            short_hash,
            author,
            email,
            date,
            message,
            refs,
            parents,
        });
        i += 8;
    }
    commits
}

// =====================
// branch (for-each-ref)
// =====================

/// Parse `git for-each-ref --format=%(refname)%00%(refname:short)%00%(upstream:short)%00%(upstream:track)%00%(HEAD)%00%(objectname)`.
pub fn parse_branches(output: &str) -> Vec<BranchInfo> {
    let mut out = Vec::new();
    for rec in output.lines() {
        if rec.is_empty() {
            continue;
        }
        let f: Vec<&str> = rec.split('\0').collect();
        if f.len() < 6 {
            continue;
        }
        let full_name = f[0].to_string();
        let name = f[1].to_string();
        let upstream = if f[2].is_empty() {
            None
        } else {
            Some(f[2].to_string())
        };
        let (ahead, behind) = parse_upstream_track(f[3]);
        let current = f[4] == "*";
        let hash = f[5].to_string();
        out.push(BranchInfo {
            name,
            full_name,
            upstream,
            ahead,
            behind,
            current,
            detached: hash.is_empty(),
        });
    }
    out
}

/// `[ahead 1, behind 2]` / `[ahead 2]` / `[behind 1]` / `[gone]` / ``.
fn parse_upstream_track(s: &str) -> (u32, u32) {
    let inner = s.trim_start_matches('[').trim_end_matches(']');
    let mut ahead = 0;
    let mut behind = 0;
    for part in inner.split(',') {
        let part = part.trim();
        if let Some(n) = part.strip_prefix("ahead ") {
            ahead = n.trim().parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            behind = n.trim().parse().unwrap_or(0);
        }
    }
    (ahead, behind)
}

// =====================
// tag
// =====================

/// Parse `git tag -l --format=%(refname)%00%(refname:short)%00%(objectname)%00%(taggername)%00%(creatordate:iso)%00%(subject)`.
pub fn parse_tags(output: &str) -> Vec<TagInfo> {
    let mut out = Vec::new();
    for rec in output.lines() {
        if rec.is_empty() {
            continue;
        }
        let f: Vec<&str> = rec.split('\0').collect();
        if f.len() < 6 {
            continue;
        }
        out.push(TagInfo {
            full_name: f[0].to_string(),
            name: f[1].to_string(),
            target: f[2].to_string(),
            tagger: if f[3].is_empty() {
                None
            } else {
                Some(f[3].to_string())
            },
            date: if f[4].is_empty() {
                None
            } else {
                Some(f[4].to_string())
            },
            message: if f[5].is_empty() {
                None
            } else {
                Some(f[5].to_string())
            },
        });
    }
    out
}

// =====================
// stash
// =====================

/// Parse `git stash list --format=%H%00%gd%00%gs%00%cr`.
pub fn parse_stash(output: &str) -> Vec<StashEntry> {
    let mut out = Vec::new();
    for rec in output.lines() {
        if rec.is_empty() {
            continue;
        }
        let f: Vec<&str> = rec.split('\0').collect();
        if f.len() < 4 {
            continue;
        }
        let index = f[1]
            .rsplit_once('{')
            .and_then(|(_, rest)| rest.strip_suffix('}'))
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(0);
        let (branch, message) = split_stash_subject(f[2]);
        out.push(StashEntry {
            index,
            message,
            branch,
            date: f[3].to_string(),
        });
    }
    out
}

/// `"WIP on main: abc123 msg"` / `"On main: msg"` → (Some("main"), msg).
fn split_stash_subject(subject: &str) -> (Option<String>, String) {
    if let Some(rest) = subject.strip_prefix("WIP on ") {
        if let Some((branch, msg)) = rest.split_once(": ") {
            return (Some(branch.to_string()), msg.to_string());
        }
    } else if let Some(rest) = subject.strip_prefix("On ") {
        if let Some((branch, msg)) = rest.split_once(": ") {
            return (Some(branch.to_string()), msg.to_string());
        }
    }
    (None, subject.to_string())
}

// =====================
// remote
// =====================

/// Parse `git remote -v`: `name<TAB>url (fetch|push)` — merge fetch/push per name.
pub fn parse_remotes(output: &str) -> Vec<RemoteInfo> {
    let mut out: Vec<RemoteInfo> = Vec::new();
    for line in output.lines() {
        if line.is_empty() {
            continue;
        }
        let Some((name, rest)) = line.split_once('\t') else {
            continue;
        };
        let (url, kind) = match rest.rsplit_once(' ') {
            Some((u, "(fetch)")) => (u, 0),
            Some((u, "(push)")) => (u, 1),
            _ => (rest, 2),
        };
        if let Some(r) = out.iter_mut().find(|r| r.name == name) {
            if kind == 1 {
                r.push_url = url.to_string();
            } else if kind == 0 {
                r.fetch_url = url.to_string();
            }
        } else {
            let mut info = RemoteInfo {
                name: name.to_string(),
                url: String::new(),
                fetch_url: String::new(),
                push_url: String::new(),
            };
            if kind == 1 {
                info.push_url = url.to_string();
            } else {
                info.fetch_url = url.to_string();
            }
            out.push(info);
        }
    }
    out
}

// =====================
// reflog
// =====================

/// Parse `git reflog --format=%H%00%h%00%gd%00%gs%00%cr`.
pub fn parse_reflog(output: &str) -> Vec<ReflogEntry> {
    let mut out = Vec::new();
    for rec in output.lines() {
        if rec.is_empty() {
            continue;
        }
        let f: Vec<&str> = rec.split('\0').collect();
        if f.len() < 5 {
            continue;
        }
        // selector is like `HEAD@{0}` or `main@{2}` → ref part before `@{`.
        let ref_name = f[2]
            .split_once("@{")
            .map(|(r, _)| r.to_string())
            .unwrap_or_else(|| f[2].to_string());
        out.push(ReflogEntry {
            hash: f[0].to_string(),
            short_hash: f[1].to_string(),
            ref_name,
            message: f[3].to_string(),
            date: f[4].to_string(),
            author: String::new(),
        });
    }
    out
}

// =====================
// tests
// =====================

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- status ----------

    #[test]
    fn status_v2_ordinary_modified() {
        // M. = staged modified; .M = unstaged modified
        let raw = "1 .M N... 100644 100644 100644 abc def src/main.rs\0";
        let out = parse_status(raw);
        assert_eq!(out.len(), 1);
        let f = &out[0];
        assert_eq!(f.path, "src/main.rs");
        assert_eq!(f.status, ".M");
        assert!(f.unstaged);
        assert!(!f.staged);
        assert!(!f.untracked);
        assert!(!f.conflict);
    }

    #[test]
    fn status_v2_staged_and_unstaged() {
        let raw = "1 MM N... 100644 100644 100644 abc def a.txt\0";
        let out = parse_status(raw);
        let f = &out[0];
        assert!(f.staged);
        assert!(f.unstaged);
    }

    #[test]
    fn status_v2_untracked_and_ignored() {
        let raw = "? new.txt\0! ignored.log\x001 .M N... 100644 100644 100644 a b tracked\0";
        let out = parse_status(raw);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].path, "new.txt");
        assert!(out[0].untracked);
        assert_eq!(out[1].path, "tracked");
    }

    #[test]
    fn status_v2_rename_with_orig_path() {
        // rename entries are `2 ... <path>\0<origPath>\0`
        let raw = "2 R. N... 100644 100644 100644 abc def R95 new.txt\0old.txt\0";
        let out = parse_status(raw);
        assert_eq!(out.len(), 1);
        let f = &out[0];
        assert_eq!(f.path, "new.txt");
        assert_eq!(f.orig_path.as_deref(), Some("old.txt"));
        assert_eq!(f.status, "R.");
        assert!(f.staged);
        assert!(!f.unstaged);
    }

    #[test]
    fn status_v2_unmerged_conflict() {
        let raw = "u AA N... 000000 100644 100644 000000 h1 h2 h3 conflict.txt\0";
        let out = parse_status(raw);
        let f = &out[0];
        assert_eq!(f.path, "conflict.txt");
        assert!(f.conflict);
        assert!(f.staged && f.unstaged);
    }

    #[test]
    fn status_v2_chinese_filename_raw_utf8() {
        // core.quotepath=false → raw UTF-8, no quoting
        let raw = "? 中文文件.txt\0";
        let out = parse_status(raw);
        assert_eq!(out[0].path, "中文文件.txt");
    }

    #[test]
    fn status_v2_skip_worktree() {
        let raw = "1 .S N... 100644 100644 100644 a b skip.txt\0";
        let out = parse_status(raw);
        assert!(out[0].skipped);
    }

    #[test]
    fn status_v2_submodule_states() {
        // sub field (real git output): N... = plain, S..U = untracked content
        // inside the submodule (dirty), SC.. = different checked-in commit.
        let raw = "1 .M S..U 160000 160000 160000 a b sub1\0\
                   1 .M N... 100644 100644 100644 a b file\0\
                   1 .M SC.. 160000 160000 160000 a b sub2\0";
        let out = parse_status(raw);
        assert_eq!(out.len(), 3);
        assert!(out[0].submodule && out[0].submodule_dirty);
        assert!(!out[0].submodule_commit_changed);
        assert!(!out[1].submodule && !out[1].submodule_dirty);
        assert!(out[2].submodule && out[2].submodule_commit_changed);
        assert!(!out[2].submodule_dirty);
    }

    // ---------- ls-files -s ----------

    #[test]
    fn ls_files_basic_and_unmerged() {
        let raw = "100644 1111111111111111111111111111111111111111 0\ta.txt\0\
                   100755 2222222222222222222222222222222222222222 0\tsh\0\
                   100644 3333333333333333333333333333333333333333 1\tc.txt\0\
                   100644 4444444444444444444444444444444444444444 2\tc.txt\0\
                   100644 5555555555555555555555555555555555555555 3\tc.txt\0";
        let out = parse_ls_files(raw);
        assert_eq!(out.len(), 5);
        assert_eq!(out[0].path, "a.txt");
        assert_eq!(out[0].mode, "100644");
        assert_eq!(out[0].stage, 0);
        assert_eq!(out[1].mode, "100755");
        let stages: Vec<u32> = out[2..]
            .iter()
            .filter(|e| e.path == "c.txt")
            .map(|e| e.stage)
            .collect();
        assert_eq!(stages, vec![1, 2, 3]);
    }

    // ---------- numstat -z ----------

    #[test]
    fn numstat_paths_basic_and_binary() {
        let raw = "1\t2\ta.txt\0-\t-\timg.png\x005\t0\tb.md\0";
        let out = parse_numstat_paths(raw);
        assert_eq!(out, vec!["a.txt", "img.png", "b.md"]);
    }

    #[test]
    fn numstat_paths_rename_empty_path_slot() {
        // rename layout: `1\t0\t\0orig\0new\0` (path slot empty)
        let raw = "1\t0\t\0old.txt\0new.txt\x003\t1\tok.txt\0";
        let out = parse_numstat_paths(raw);
        assert_eq!(out, vec!["new.txt", "ok.txt"]);
    }

    #[test]
    fn numstat_paths_chinese_filename_raw_utf8() {
        let raw = "1\t1\t中文.txt\0";
        let out = parse_numstat_paths(raw);
        assert_eq!(out, vec!["中文.txt"]);
    }

    // ---------- diff ----------

    #[test]
    fn diff_simple_modify() {
        // NOTE: `\x20` renders the mandatory context-line prefix space that the
        // Rust string continuation `\`+newline would otherwise strip.
        let raw = "diff --git a/a.txt b/a.txt\n\
                   index 111..222 100644\n\
                   --- a/a.txt\n\
                   +++ b/a.txt\n\
                   @@ -1,3 +1,3 @@ fn section\n\
                   \x20context line\n\
                   -old line\n\
                   +new line\n\
                   \x20another context\n";
        let model = parse_diff(raw, DiffSource::Worktree, None).unwrap();
        assert_eq!(model.files.len(), 1);
        let f = &model.files[0];
        assert_eq!(f.old_path.as_deref(), Some("a.txt"));
        assert_eq!(f.new_path.as_deref(), Some("a.txt"));
        assert_eq!(f.hunks.len(), 1);
        let h = &f.hunks[0];
        assert_eq!(h.old_start, 1);
        assert_eq!(h.new_start, 1);
        assert_eq!(h.header, "fn section");
        assert_eq!(h.lines.len(), 4);
        assert_eq!(h.lines[0].kind, DiffLineKind::Context);
        assert_eq!(h.lines[0].left_no, Some(1));
        assert_eq!(h.lines[0].right_no, Some(1));
        assert_eq!(h.lines[1].kind, DiffLineKind::Remove);
        assert_eq!(h.lines[1].left_no, Some(2));
        assert_eq!(h.lines[1].right_no, None);
        assert_eq!(h.lines[2].kind, DiffLineKind::Add);
        assert_eq!(h.lines[2].right_no, Some(2));
        assert_eq!(h.lines[3].left_no, Some(3));
    }

    #[test]
    fn diff_add_and_delete_file() {
        let raw = "diff --git a/added.txt b/added.txt\n\
                   new file mode 100644\n\
                   index 000..abc\n\
                   --- /dev/null\n\
                   +++ b/added.txt\n\
                   @@ -0,0 +1,2 @@\n\
                   +line one\n\
                   +line two\n\
                   diff --git a/deleted.txt b/deleted.txt\n\
                   deleted file mode 100644\n\
                   index abc..000\n\
                   --- a/deleted.txt\n\
                   +++ /dev/null\n\
                   @@ -1,1 +0,0 @@\n\
                   -goodbye\n";
        let model = parse_diff(raw, DiffSource::Staged, None).unwrap();
        assert_eq!(model.files.len(), 2);
        assert_eq!(model.files[0].old_path, None);
        assert_eq!(model.files[0].new_path.as_deref(), Some("added.txt"));
        assert_eq!(model.files[0].hunks[0].lines.len(), 2);
        assert_eq!(model.files[1].old_path.as_deref(), Some("deleted.txt"));
        assert_eq!(model.files[1].new_path, None);
        assert_eq!(model.files[1].hunks[0].lines[0].kind, DiffLineKind::Remove);
    }

    #[test]
    fn diff_rename_with_similarity() {
        let raw = "diff --git a/old_name.txt b/new_name.txt\n\
                   similarity index 92%\n\
                   rename from old_name.txt\n\
                   rename to new_name.txt\n\
                   index 111..222 100644\n\
                   --- a/old_name.txt\n\
                   +++ b/new_name.txt\n\
                   @@ -1 +1 @@\n\
                   -content\n\
                   +content2\n";
        let model = parse_diff(raw, DiffSource::Worktree, None).unwrap();
        let f = &model.files[0];
        assert_eq!(f.old_path.as_deref(), Some("old_name.txt"));
        assert_eq!(f.new_path.as_deref(), Some("new_name.txt"));
        assert_eq!(f.similarity, Some(92));
    }

    #[test]
    fn diff_binary_and_no_newline_marker() {
        let raw = "diff --git a/img.png b/img.png\n\
                   index 111..222 100644\n\
                   Binary files a/img.png and b/img.png differ\n\
                   diff --git a/x.txt b/x.txt\n\
                   index 333..444 100644\n\
                   --- a/x.txt\n\
                   +++ b/x.txt\n\
                   @@ -1 +1 @@\n\
                   -old\\n\n\
                   +new\n\
                   \\ No newline at end of file\n";
        let model = parse_diff(raw, DiffSource::Worktree, None).unwrap();
        assert_eq!(model.files.len(), 2);
        assert!(model.files[0].binary);
        // "\ No newline" line must not be counted as a diff line.
        assert_eq!(model.files[1].hunks[0].lines.len(), 2);
    }

    #[test]
    fn diff_body_line_starting_with_dashes_not_misparsed() {
        // A removed line whose content is "-- separator" appears as "--- separator"
        // and must stay a body line, not start a new file.
        let raw = "diff --git a/doc.md b/doc.md\n\
                   index 111..222 100644\n\
                   --- a/doc.md\n\
                   +++ b/doc.md\n\
                   @@ -1,3 +1,2 @@\n\
                   \x20title\n\
                   --- separator\n\
                   -old\n\
                   +new\n\
                   diff --git a/next.txt b/next.txt\n\
                   index 333..444 100644\n\
                   --- a/next.txt\n\
                   +++ b/next.txt\n\
                   @@ -1 +1 @@\n\
                   -a\n\
                   +b\n";
        let model = parse_diff(raw, DiffSource::Worktree, None).unwrap();
        assert_eq!(model.files.len(), 2);
        let f0 = &model.files[0];
        assert_eq!(f0.old_path.as_deref(), Some("doc.md"));
        let h = &f0.hunks[0];
        assert_eq!(h.lines.len(), 4);
        assert_eq!(h.lines[1].kind, DiffLineKind::Remove);
        assert_eq!(h.lines[1].content, "-- separator");
        assert_eq!(model.files[1].old_path.as_deref(), Some("next.txt"));
        assert_eq!(model.files[1].hunks.len(), 1);
    }

    #[test]
    fn diff_path_with_space_and_empty_context_line() {
        let raw = "diff --git a/sp ace.txt b/sp ace.txt\n\
                   index 111..222 100644\n\
                   --- a/sp ace.txt\n\
                   +++ b/sp ace.txt\n\
                   @@ -1,3 +1,3 @@\n\
                   \x20first\n\
                   \x20\n\
                   -third\n\
                   +changed\n";
        let model = parse_diff(raw, DiffSource::Worktree, None).unwrap();
        let f = &model.files[0];
        assert_eq!(f.old_path.as_deref(), Some("sp ace.txt"));
        let h = &f.hunks[0];
        // The blank context line is preserved.
        assert_eq!(h.lines[1].kind, DiffLineKind::Context);
        assert_eq!(h.lines[1].content, "");
        assert_eq!(h.lines.len(), 4);
    }

    #[test]
    fn diff_hunk_counts_advance_line_numbers() {
        let raw = "diff --git a/n.txt b/n.txt\n\
                   index 111..222 100644\n\
                   --- a/n.txt\n\
                   +++ b/n.txt\n\
                   @@ -10,3 +10,4 @@\n\
                   \x20a\n\
                   +b\n\
                   \x20c\n\
                   \x20d\n";
        let model = parse_diff(raw, DiffSource::Worktree, None).unwrap();
        let lines = &model.files[0].hunks[0].lines;
        assert_eq!(lines[0].left_no, Some(10));
        assert_eq!(lines[0].right_no, Some(10));
        assert_eq!(lines[1].right_no, Some(11));
        assert_eq!(lines[2].right_no, Some(12));
        assert_eq!(lines[2].left_no, Some(11));
    }

    #[test]
    fn diff_rev_range_recorded() {
        let model = parse_diff("", DiffSource::Commit, Some(("HEAD~1", "HEAD"))).unwrap();
        assert_eq!(model.old_revision.as_deref(), Some("HEAD~1"));
        assert_eq!(model.new_revision.as_deref(), Some("HEAD"));
    }

    #[test]
    fn diff_octal_escaped_path_unquoted() {
        // With quotepath=false non-ASCII comes raw, but quoted fallback still handled.
        // \\346\\226\\207 = UTF-8 bytes of 文.
        let raw = "diff --git a/\\346\\226\\207.txt b/\\346\\226\\207.txt\n\
                   index 111..222 100644\n\
                   --- \"a/\\346\\226\\207.txt\"\n\
                   +++ \"b/\\346\\226\\207.txt\"\n\
                   @@ -1 +1 @@\n\
                   -a\n\
                   +b\n";
        let model = parse_diff(raw, DiffSource::Worktree, None).unwrap();
        let f = &model.files[0];
        assert_eq!(f.old_path, Some("文.txt".to_string()));
    }

    // ---------- log ----------

    #[test]
    fn log_two_commits_with_parents_and_refs() {
        let raw = "aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111\n\
                   aaaa111\n\
                   Alice\n\
                   alice@example.com\n\
                   2024-01-01T10:00:00+08:00\n\
                   add feature\n\
                   (HEAD -> main, origin/main)\n\
                   \n\
                   bbbb2222bbbb2222bbbb2222bbbb2222bbbb2222\n\
                   bbbb222\n\
                   Bob\n\
                   bob@example.com\n\
                   2024-01-02T10:00:00+08:00\n\
                   initial\n\
                   \n\
                   aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111\n";
        let out = parse_log(raw);
        assert_eq!(out.len(), 2);
        let c0 = &out[0];
        assert_eq!(c0.hash, "aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111");
        assert_eq!(c0.author, "Alice");
        assert_eq!(c0.message, "add feature");
        assert_eq!(c0.refs, vec!["HEAD -> main", "origin/main"]);
        assert!(c0.parents.is_empty()); // root commit
        let c1 = &out[1];
        assert_eq!(c1.parents, vec!["aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111"]);
        assert!(c1.refs.is_empty());
    }

    // ---------- branches ----------

    #[test]
    fn branches_parse_track_and_head() {
        let raw = "refs/heads/feature\0feature\0\0\0 \0abc1234abc1234abc1234abc1234abc1234\n\
                   refs/heads/main\0main\0origin/main\0[ahead 1, behind 2]\0*\0def5678def5678def5678def5678def5678\n";
        let out = parse_branches(raw);
        assert_eq!(out.len(), 2);
        let feat = &out[0];
        assert_eq!(feat.name, "feature");
        assert_eq!(feat.full_name, "refs/heads/feature");
        assert_eq!(feat.upstream, None);
        assert_eq!((feat.ahead, feat.behind), (0, 0));
        assert!(!feat.current);
        let main = &out[1];
        assert_eq!(main.upstream.as_deref(), Some("origin/main"));
        assert_eq!((main.ahead, main.behind), (1, 2));
        assert!(main.current);
    }

    #[test]
    fn branches_parse_gone_upstream() {
        let raw = "refs/heads/dev\0dev\0origin/dev\0[gone]\0 \0abc\n";
        let out = parse_branches(raw);
        let b = &out[0];
        assert_eq!((b.ahead, b.behind), (0, 0));
        assert_eq!(b.upstream.as_deref(), Some("origin/dev"));
    }

    // ---------- tags ----------

    #[test]
    fn tags_parse() {
        let raw =
            "refs/tags/v1.0\0v1.0\0abc1234full\0Alice\x002024-01-01 10:00:00 +0800\0release one\n\
                   refs/tags/light\0light\0def5678full\0\0\0\n";
        let out = parse_tags(raw);
        assert_eq!(out.len(), 2);
        let t = &out[0];
        assert_eq!(t.name, "v1.0");
        assert_eq!(t.full_name, "refs/tags/v1.0");
        assert_eq!(t.tagger.as_deref(), Some("Alice"));
        assert_eq!(t.message.as_deref(), Some("release one"));
        let light = &out[1];
        assert_eq!(light.tagger, None::<String>);
        assert_eq!(light.message, None::<String>);
    }

    // ---------- stash ----------

    #[test]
    fn stash_parse_wip_and_on() {
        let raw = "h1\0stash@{0}\0WIP on main: abc123 some commit\x002 hours ago\n\
                   h2\0stash@{1}\0On feature: manual message\0yesterday\n";
        let out = parse_stash(raw);
        assert_eq!(out.len(), 2);
        let s0 = &out[0];
        assert_eq!(s0.index, 0);
        assert_eq!(s0.branch.as_deref(), Some("main"));
        assert_eq!(s0.message, "abc123 some commit");
        let s1 = &out[1];
        assert_eq!(s1.index, 1);
        assert_eq!(s1.branch.as_deref(), Some("feature"));
        assert_eq!(s1.message, "manual message");
    }

    // ---------- remotes ----------

    #[test]
    fn remotes_parse_fetch_and_push() {
        let raw = "origin\thttps://example.com/a.git (fetch)\n\
                   origin\thttps://example.com/a.git (push)\n\
                   upstream\tgit@github.com:b/b.git (fetch)\n";
        let out = parse_remotes(raw);
        assert_eq!(out.len(), 2);
        let o = &out[0];
        assert_eq!(o.name, "origin");
        assert_eq!(o.fetch_url, "https://example.com/a.git");
        assert_eq!(o.push_url, "https://example.com/a.git");
        let u = &out[1];
        assert_eq!(u.name, "upstream");
        assert_eq!(u.fetch_url, "git@github.com:b/b.git");
        assert_eq!(u.push_url, "");
    }

    // ---------- reflog ----------

    #[test]
    fn reflog_parse() {
        let raw = "h1\0s1\0HEAD@{0}\0commit: fix bug\x002024-01-01 10:00:00 +0800\n\
                   h2\0s2\0HEAD@{1}\0checkout: moving from main to dev\x002024-01-01 09:00:00 +0800\n";
        let out = parse_reflog(raw);
        assert_eq!(out.len(), 2);
        let r0 = &out[0];
        assert_eq!(r0.hash, "h1");
        assert_eq!(r0.ref_name, "HEAD");
        assert_eq!(r0.message, "commit: fix bug");
        assert_eq!(r0.date, "2024-01-01 10:00:00 +0800");
    }
}
