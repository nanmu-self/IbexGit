//! P4 acceptance: line-level stage / discard / unstage across 20+ edge-case
//! fixtures against a real git repository (PLAN P4 验收: 行级暂存/丢弃全部
//! 通过). Every case goes through the full pipeline: engine.diff → cached
//! model → patch builder → `git apply`.
//!
//! Line-selection semantics follow the standard (VSCode-style) rules:
//! staging a `+` line inserts it into the index at its worktree position;
//! unselected `-` lines stay in the index (staging) / stay removed
//! (discarding). Marker hunks (`\ No newline at end of file`) are
//! all-or-nothing.

use ibexgit_lib::core::engine::patch::{build_line_patch, LineOp};
use ibexgit_lib::core::engine::untracked::synthesize_untracked;
use ibexgit_lib::core::engine::{
    CliEngine, DiffLineKind, DiffOptions, DiffSource, GitEngine, LineSelection,
};
use ibexgit_lib::core::runner::GitProcessRunner;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

struct TempRepo {
    dir: PathBuf,
}

impl TempRepo {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "ibexgit-lineops-{}-{}-{}",
            tag,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
                * 1000
                + {
                    use std::sync::atomic::Ordering;
                    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                    N.fetch_add(1, Ordering::Relaxed) as u128
                }
        ));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "user.email", "t@t"]);
        git(&dir, &["config", "user.name", "t"]);
        // Deterministic byte-level content: no smudge/clean filters.
        git(&dir, &["config", "core.autocrlf", "false"]);
        TempRepo { dir }
    }

    fn write(&self, rel: &str, content: &str) {
        std::fs::write(self.dir.join(rel), content).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.dir.join(rel)).unwrap()
    }

    fn commit_all(&self, msg: &str) {
        git(&self.dir, &["add", "-A"]);
        git(&self.dir, &["commit", "-q", "-m", msg]);
    }

    fn path(&self) -> String {
        self.dir.display().to_string()
    }

    async fn worktree_diff(&self, paths: &[&str]) -> ibexgit_lib::core::engine::DiffModel {
        self.engine()
            .diff(
                &self.path(),
                DiffSource::Worktree,
                None,
                &paths.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                DiffOptions::default(),
            )
            .await
            .expect("diff")
    }

    async fn staged_diff(&self, paths: &[&str]) -> ibexgit_lib::core::engine::DiffModel {
        self.engine()
            .diff(
                &self.path(),
                DiffSource::Staged,
                None,
                &paths.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                DiffOptions::default(),
            )
            .await
            .expect("diff")
    }

    fn engine(&self) -> Arc<dyn GitEngine> {
        Arc::new(CliEngine::new(GitProcessRunner::new(60), "git"))
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn git(dir: &PathBuf, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git available");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn sels(hunk: u32, lines: &[u32]) -> Vec<LineSelection> {
    lines
        .iter()
        .map(|&line| LineSelection { hunk, line })
        .collect()
}

async fn apply(repo: &TempRepo, patch: &str, cached: bool, reverse: bool) {
    if patch.is_empty() {
        return;
    }
    repo.engine()
        .apply(&repo.path(), patch, cached, reverse)
        .await
        .expect("git apply must succeed");
}

/// Index content of a file (`git show :path`), empty string if absent.
fn index_content(repo: &TempRepo, rel: &str) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(&repo.dir)
        .args(["show", &format!(":{rel}")])
        .output()
        .expect("git show");
    if out.status.success() {
        String::from_utf8(out.stdout).unwrap()
    } else {
        String::new()
    }
}

/// Line kinds of hunk 0 as (kind char, content) — debugging helper.
fn layout(file: &ibexgit_lib::core::engine::DiffFile) -> Vec<(char, &str)> {
    file.hunks[0]
        .lines
        .iter()
        .map(|l| {
            (
                match l.kind {
                    DiffLineKind::Context => ' ',
                    DiffLineKind::Add => '+',
                    DiffLineKind::Remove => '-',
                    DiffLineKind::Header => '@',
                },
                l.content.as_str(),
            )
        })
        .collect()
}

// =====================
// 1. simple mid-file modify
// =====================

#[tokio::test]
async fn stage_single_line_of_multi_line_change() {
    let repo = TempRepo::new("stage-single");
    repo.write("f.txt", "a\nb\nc\nd\ne\n");
    repo.commit_all("init");
    repo.write("f.txt", "a\nB1\nB2\nd\ne\n");

    let model = repo.worktree_diff(&["f.txt"]).await;
    let file = &model.files[0];
    // lines: ctx a / -b / -c / +B1 / +B2 / ctx d / ctx e
    assert_eq!(
        layout(file),
        vec![
            (' ', "a"),
            ('-', "b"),
            ('-', "c"),
            ('+', "B1"),
            ('+', "B2"),
            (' ', "d"),
            (' ', "e")
        ]
    );
    // Staging just the +B1 line inserts it into the index after c.
    let patch = build_line_patch(file, "f.txt", &sels(0, &[3]), LineOp::Stage).unwrap();
    apply(&repo, &patch, true, false).await;
    assert_eq!(index_content(&repo, "f.txt"), "a\nb\nc\nB1\nd\ne\n");
    // Worktree untouched.
    assert_eq!(repo.read("f.txt"), "a\nB1\nB2\nd\ne\n");
}

#[tokio::test]
async fn stage_full_replacement_line_wise() {
    let repo = TempRepo::new("stage-replace");
    repo.write("f.txt", "a\nb\nc\nd\ne\n");
    repo.commit_all("init");
    repo.write("f.txt", "a\nB1\nB2\nd\ne\n");

    let model = repo.worktree_diff(&["f.txt"]).await;
    let file = &model.files[0];
    // Select every changed line (-b, -c, +B1, +B2) → index matches worktree.
    let patch = build_line_patch(file, "f.txt", &sels(0, &[1, 2, 3, 4]), LineOp::Stage).unwrap();
    apply(&repo, &patch, true, false).await;
    assert_eq!(index_content(&repo, "f.txt"), "a\nB1\nB2\nd\ne\n");
}

#[tokio::test]
async fn discard_single_added_line_from_worktree() {
    let repo = TempRepo::new("discard-add");
    repo.write("f.txt", "a\nb\nc\n");
    repo.commit_all("init");
    repo.write("f.txt", "a\nX\nb\nc\n");

    let model = repo.worktree_diff(&["f.txt"]).await;
    let file = &model.files[0];
    // lines: ctx a / +X / ctx b / ctx c
    let patch = build_line_patch(file, "f.txt", &sels(0, &[1]), LineOp::Discard).unwrap();
    apply(&repo, &patch, false, true).await;
    assert_eq!(repo.read("f.txt"), "a\nb\nc\n");
}

#[tokio::test]
async fn discard_restores_removed_line_into_worktree() {
    let repo = TempRepo::new("discard-remove");
    repo.write("f.txt", "a\nb\nc\n");
    repo.commit_all("init");
    repo.write("f.txt", "a\nc\n");

    let model = repo.worktree_diff(&["f.txt"]).await;
    let file = &model.files[0];
    // lines: ctx a / -b / ctx c → discarding the removal restores b.
    let patch = build_line_patch(file, "f.txt", &sels(0, &[1]), LineOp::Discard).unwrap();
    apply(&repo, &patch, false, true).await;
    assert_eq!(repo.read("f.txt"), "a\nb\nc\n");
}

// =====================
// 2. multi-hunk selection
// =====================

#[tokio::test]
async fn stage_lines_across_multiple_hunks() {
    let repo = TempRepo::new("multi-hunk");
    let mut base = String::new();
    for i in 1..=40 {
        base.push_str(&format!("line{i}\n"));
    }
    repo.write("f.txt", &base);
    repo.commit_all("init");
    let modified =
        base.replacen("line5\n", "line5-EDIT\n", 1)
            .replacen("line30\n", "line30-EDIT\n", 1);
    repo.write("f.txt", &modified);

    let model = repo.worktree_diff(&["f.txt"]).await;
    let file = &model.files[0];
    assert_eq!(file.hunks.len(), 2);
    // Each hunk: 3 ctx / -line / +line-EDIT / 3 ctx → changed at idx 3, 4.
    // Stage hunk 0 fully, leave hunk 1 unstaged.
    let patch = build_line_patch(file, "f.txt", &sels(0, &[3, 4]), LineOp::Stage).unwrap();
    apply(&repo, &patch, true, false).await;
    let idx = index_content(&repo, "f.txt");
    assert!(idx.contains("line5-EDIT"), "{idx}");
    assert!(!idx.contains("line30-EDIT"), "{idx}");
}

#[tokio::test]
async fn discard_second_hunk_only() {
    let repo = TempRepo::new("discard-hunk2");
    let mut base = String::new();
    for i in 1..=40 {
        base.push_str(&format!("line{i}\n"));
    }
    repo.write("f.txt", &base);
    repo.commit_all("init");
    let modified =
        base.replacen("line5\n", "line5-EDIT\n", 1)
            .replacen("line30\n", "line30-EDIT\n", 1);
    repo.write("f.txt", &modified);

    let model = repo.worktree_diff(&["f.txt"]).await;
    let file = &model.files[0];
    let patch = build_line_patch(file, "f.txt", &sels(1, &[3, 4]), LineOp::Discard).unwrap();
    apply(&repo, &patch, false, true).await;
    let wt = repo.read("f.txt");
    assert!(wt.contains("line5-EDIT"), "{wt}");
    assert!(!wt.contains("line30-EDIT"), "{wt}");
    assert!(
        wt.contains("line30\n"),
        "the original line must be restored"
    );
}

// =====================
// 3. no-newline-at-EOF cases
// =====================

#[tokio::test]
async fn no_eol_stage_entire_hunk_appends_lines() {
    let repo = TempRepo::new("noeol-append");
    repo.write("f.txt", "last");
    repo.commit_all("init");
    repo.write("f.txt", "last\nnew\n");

    let model = repo.worktree_diff(&["f.txt"]).await;
    let file = &model.files[0];
    // Partial selection in a marker hunk is refused.
    assert!(build_line_patch(file, "f.txt", &sels(0, &[2]), LineOp::Stage).is_err());
    let patch = build_line_patch(file, "f.txt", &sels(0, &[0, 1, 2]), LineOp::Stage).unwrap();
    apply(&repo, &patch, true, false).await;
    assert_eq!(index_content(&repo, "f.txt"), "last\nnew\n");
}

#[tokio::test]
async fn no_eol_discard_all_restores_index_bytes() {
    let repo = TempRepo::new("noeol-discard");
    repo.write("f.txt", "a\n");
    repo.commit_all("init");
    // Worktree removes the trailing newline of the only line.
    std::fs::write(repo.dir.join("f.txt"), "a").unwrap();

    let model = repo.worktree_diff(&["f.txt"]).await;
    let file = &model.files[0];
    let patch = build_line_patch(file, "f.txt", &sels(0, &[0, 1]), LineOp::Discard).unwrap();
    apply(&repo, &patch, false, true).await;
    assert_eq!(std::fs::read(repo.dir.join("f.txt")).unwrap(), b"a\n");
}

#[tokio::test]
async fn no_eol_unstage_all_restores_head() {
    let repo = TempRepo::new("noeol-unstage");
    repo.write("f.txt", "keep\n");
    repo.commit_all("init");
    // Stage the no-EOL removal of the trailing newline.
    std::fs::write(repo.dir.join("f.txt"), "keep").unwrap();
    git(&repo.dir, &["add", "f.txt"]);

    let model = repo.staged_diff(&["f.txt"]).await;
    let file = &model.files[0];
    let patch = build_line_patch(file, "f.txt", &sels(0, &[0, 1]), LineOp::Unstage).unwrap();
    apply(&repo, &patch, true, true).await;
    // Unstaging restores HEAD's trailing newline.
    assert_eq!(index_content(&repo, "f.txt"), "keep\n");
}

#[tokio::test]
async fn no_eol_context_marker_partial_ops_allowed() {
    // File ends with an UNCHANGED no-EOL line; a change above it can still
    // be staged line-wise (the context marker terminates both sides).
    let repo = TempRepo::new("noeol-ctx");
    repo.write("f.txt", "first\nend");
    repo.commit_all("init");
    repo.write("f.txt", "first\nmiddle\nend");

    let model = repo.worktree_diff(&["f.txt"]).await;
    let file = &model.files[0];
    let patch = build_line_patch(file, "f.txt", &sels(0, &[1]), LineOp::Stage).unwrap();
    apply(&repo, &patch, true, false).await;
    assert_eq!(index_content(&repo, "f.txt"), "first\nmiddle\nend");
}

// =====================
// 4. empty files / add / delete
// =====================

#[tokio::test]
async fn stage_into_previously_empty_file() {
    let repo = TempRepo::new("empty-to-content");
    repo.write("f.txt", "");
    repo.commit_all("init");
    repo.write("f.txt", "one\ntwo\n");

    let model = repo.worktree_diff(&["f.txt"]).await;
    let file = &model.files[0];
    let patch = build_line_patch(file, "f.txt", &sels(0, &[0]), LineOp::Stage).unwrap();
    apply(&repo, &patch, true, false).await;
    assert_eq!(index_content(&repo, "f.txt"), "one\n");
}

#[tokio::test]
async fn stage_deletion_partially_keeps_remaining_lines() {
    let repo = TempRepo::new("partial-delete");
    repo.write("f.txt", "a\nb\n");
    repo.commit_all("init");
    std::fs::remove_file(repo.dir.join("f.txt")).unwrap();

    let model = repo.worktree_diff(&["f.txt"]).await;
    let file = &model.files[0];
    // Stage only the deletion of "a": index keeps "b".
    let patch = build_line_patch(file, "f.txt", &sels(0, &[0]), LineOp::Stage).unwrap();
    apply(&repo, &patch, true, false).await;
    assert_eq!(index_content(&repo, "f.txt"), "b\n");
    assert!(!repo.dir.join("f.txt").exists());
}

#[tokio::test]
async fn unstage_part_of_staged_new_file() {
    let repo = TempRepo::new("unstage-new-file");
    repo.write("f.txt", "one\ntwo\nthree\n");
    git(&repo.dir, &["add", "f.txt"]);

    let model = repo.staged_diff(&["f.txt"]).await;
    let file = &model.files[0];
    let patch = build_line_patch(file, "f.txt", &sels(0, &[1]), LineOp::Unstage).unwrap();
    apply(&repo, &patch, true, true).await;
    assert_eq!(index_content(&repo, "f.txt"), "one\nthree\n");
}

#[tokio::test]
async fn unstage_all_lines_of_staged_new_file_removes_entry() {
    let repo = TempRepo::new("unstage-new-file-all");
    repo.write("f.txt", "one\ntwo\n");
    git(&repo.dir, &["add", "f.txt"]);

    let model = repo.staged_diff(&["f.txt"]).await;
    let file = &model.files[0];
    let patch = build_line_patch(file, "f.txt", &sels(0, &[0, 1]), LineOp::Unstage).unwrap();
    apply(&repo, &patch, true, true).await;
    assert_eq!(index_content(&repo, "f.txt"), "");
    assert_eq!(repo.read("f.txt"), "one\ntwo\n");
}

// =====================
// 5. untracked files (synthetic model)
// =====================

#[tokio::test]
async fn untracked_stage_lines_creates_partial_index_entry() {
    let repo = TempRepo::new("untracked-stage");
    repo.write("new.txt", "one\ntwo\nthree\n");

    let file = synthesize_untracked(&repo.dir, "new.txt").unwrap();
    let patch = build_line_patch(&file, "new.txt", &sels(0, &[0, 2]), LineOp::Stage).unwrap();
    apply(&repo, &patch, true, false).await;
    assert_eq!(index_content(&repo, "new.txt"), "one\nthree\n");
    assert_eq!(repo.read("new.txt"), "one\ntwo\nthree\n");
}

#[tokio::test]
async fn untracked_discard_all_lines_deletes_file() {
    let repo = TempRepo::new("untracked-discard-all");
    repo.write("new.txt", "one\ntwo\n");

    let file = synthesize_untracked(&repo.dir, "new.txt").unwrap();
    let patch = build_line_patch(&file, "new.txt", &sels(0, &[0, 1]), LineOp::Discard).unwrap();
    apply(&repo, &patch, false, true).await;
    assert!(!repo.dir.join("new.txt").exists());
}

#[tokio::test]
async fn untracked_discard_some_lines_trims_file() {
    let repo = TempRepo::new("untracked-discard-some");
    repo.write("new.txt", "one\ntwo\nthree\n");

    let file = synthesize_untracked(&repo.dir, "new.txt").unwrap();
    let patch = build_line_patch(&file, "new.txt", &sels(0, &[2]), LineOp::Discard).unwrap();
    apply(&repo, &patch, false, true).await;
    assert_eq!(repo.read("new.txt"), "one\ntwo\n");
}

#[tokio::test]
async fn untracked_no_trailing_newline_roundtrip() {
    let repo = TempRepo::new("untracked-noeol");
    repo.write("new.txt", "solo");

    let file = synthesize_untracked(&repo.dir, "new.txt").unwrap();
    assert!(file.hunks[0].lines[0].no_eol);
    let patch = build_line_patch(&file, "new.txt", &sels(0, &[0]), LineOp::Stage).unwrap();
    apply(&repo, &patch, true, false).await;
    assert_eq!(index_content(&repo, "new.txt"), "solo");
}

// =====================
// 6. encodings / CRLF / whitespace mode
// =====================

#[tokio::test]
async fn chinese_filename_line_stage() {
    let repo = TempRepo::new("chinese-name");
    repo.write("中文文件.txt", "一\n二\n三\n");
    repo.commit_all("init");
    repo.write("中文文件.txt", "一\n二改\n三\n");

    let model = repo.worktree_diff(&["中文文件.txt"]).await;
    let file = &model.files[0];
    // lines: ctx 一 / -二 / +二改 / ctx 三 — stage the full replacement.
    let patch = build_line_patch(file, "中文文件.txt", &sels(0, &[1, 2]), LineOp::Stage).unwrap();
    apply(&repo, &patch, true, false).await;
    assert_eq!(index_content(&repo, "中文文件.txt"), "一\n二改\n三\n");
}

#[tokio::test]
async fn crlf_content_line_ops() {
    let repo = TempRepo::new("crlf");
    repo.write("f.txt", "a\r\nb\r\nc\r\n");
    repo.commit_all("init");
    repo.write("f.txt", "a\r\nB\r\nc\r\n");

    let model = repo.worktree_diff(&["f.txt"]).await;
    let file = &model.files[0];
    // lines: ctx a / -b / +B / ctx c — stage the full replacement.
    let patch = build_line_patch(file, "f.txt", &sels(0, &[1, 2]), LineOp::Stage).unwrap();
    apply(&repo, &patch, true, false).await;
    assert_eq!(index_content(&repo, "f.txt"), "a\r\nB\r\nc\r\n");
}

#[tokio::test]
async fn ignore_whitespace_diff_roundtrip() {
    let repo = TempRepo::new("ignore-ws");
    repo.write("f.txt", "alpha\n  beta\n");
    repo.commit_all("init");
    repo.write("f.txt", "alpha\nbeta\n");

    // With -w the indent-only change produces no hunk → nothing to select.
    let opts = DiffOptions {
        context_lines: 3,
        ignore_whitespace: true,
    };
    let engine = repo.engine();
    let model = engine
        .diff(
            &repo.path(),
            DiffSource::Worktree,
            None,
            &[String::from("f.txt")],
            opts,
        )
        .await
        .unwrap();
    assert!(model.files.is_empty() || model.files[0].hunks.is_empty());
}

#[tokio::test]
async fn expanded_context_refetch_still_applies() {
    let repo = TempRepo::new("expanded-context");
    let mut base = String::new();
    for i in 1..=30 {
        base.push_str(&format!("l{i}\n"));
    }
    repo.write("f.txt", &base);
    repo.commit_all("init");
    repo.write("f.txt", &base.replacen("l15\n", "L15\n", 1));

    // Refetch with a bigger context like the viewer's expand action.
    let opts = DiffOptions {
        context_lines: 12,
        ignore_whitespace: false,
    };
    let engine = repo.engine();
    let model = engine
        .diff(
            &repo.path(),
            DiffSource::Worktree,
            None,
            &[String::from("f.txt")],
            opts,
        )
        .await
        .unwrap();
    let file = &model.files[0];
    let idx = file.hunks[0]
        .lines
        .iter()
        .position(|l| l.kind == DiffLineKind::Add)
        .unwrap() as u32;
    let patch = build_line_patch(file, "f.txt", &sels(0, &[idx]), LineOp::Stage).unwrap();
    apply(&repo, &patch, true, false).await;
    assert!(index_content(&repo, "f.txt").contains("L15"));
}

// =====================
// 7. guards: binary, rename, no-change
// =====================

#[tokio::test]
async fn binary_file_line_ops_rejected() {
    let repo = TempRepo::new("binary");
    repo.write("img.bin", "a\nb\n");
    repo.commit_all("init");
    std::fs::write(repo.dir.join("img.bin"), b"\x00\x01binary").unwrap();

    let model = repo.worktree_diff(&["img.bin"]).await;
    let file = &model.files[0];
    assert!(file.binary);
    assert!(build_line_patch(file, "img.bin", &sels(0, &[0]), LineOp::Stage).is_err());
}

#[tokio::test]
async fn renamed_file_line_ops_rejected() {
    let repo = TempRepo::new("rename");
    repo.write("old.txt", "a\nb\nc\nd\ne\nf\ng\n");
    repo.commit_all("init");
    repo.write("new.txt", "a\nb-changed\nc\nd\ne\nf\ng\n");
    std::fs::remove_file(repo.dir.join("old.txt")).unwrap();
    git(&repo.dir, &["add", "-A"]);

    // Request BOTH paths so git can pair them into a rename entry (a
    // pathspec limited to the new path reports a plain add instead).
    let engine = repo.engine();
    let model = engine
        .diff(
            &repo.path(),
            DiffSource::Staged,
            None,
            &[String::from("old.txt"), String::from("new.txt")],
            DiffOptions::default(),
        )
        .await
        .unwrap();
    let file = model
        .files
        .iter()
        .find(|f| f.new_path.as_deref() == Some("new.txt"))
        .expect("rename entry");
    assert_ne!(file.old_path.as_deref(), file.new_path.as_deref());
    assert!(build_line_patch(file, "new.txt", &sels(0, &[1]), LineOp::Unstage).is_err());
}

#[tokio::test]
async fn no_selection_is_noop() {
    let repo = TempRepo::new("noop");
    repo.write("f.txt", "a\nb\n");
    repo.commit_all("init");
    repo.write("f.txt", "a\nB\n");

    let model = repo.worktree_diff(&["f.txt"]).await;
    let file = &model.files[0];
    let patch = build_line_patch(file, "f.txt", &[], LineOp::Stage).unwrap();
    assert_eq!(patch, "");
    // Full selection (ctx a / -b / +B / ctx... wait: a,b → a,B: 1 ctx, -b, +B).
    let patch = build_line_patch(file, "f.txt", &sels(0, &[0, 1, 2]), LineOp::Stage).unwrap();
    apply(&repo, &patch, true, false).await;
    assert_eq!(index_content(&repo, "f.txt"), "a\nB\n");
}

// =====================
// 8. end-to-end consistency
// =====================

#[tokio::test]
async fn staged_model_unstage_roundtrip() {
    let repo = TempRepo::new("roundtrip");
    repo.write("f.txt", "one\ntwo\nthree\n");
    repo.commit_all("init");
    repo.write("f.txt", "one\nTWO\nthree\n");
    git(&repo.dir, &["add", "f.txt"]);

    let model = repo.staged_diff(&["f.txt"]).await;
    let file = &model.files[0];
    // lines: ctx one / -two / +TWO / ctx three — unstage the addition.
    let patch = build_line_patch(file, "f.txt", &sels(0, &[2]), LineOp::Unstage).unwrap();
    apply(&repo, &patch, true, true).await;
    assert_eq!(index_content(&repo, "f.txt"), "one\nthree\n");
    assert_eq!(repo.read("f.txt"), "one\nTWO\nthree\n");
}

#[tokio::test]
async fn numstat_status_consistent_after_full_line_stage() {
    let repo = TempRepo::new("numstat-consistency");
    repo.write("f.txt", "a\nb\nc\n");
    repo.commit_all("init");
    repo.write("f.txt", "a\nB\nc\n");

    let model = repo.worktree_diff(&["f.txt"]).await;
    let file = &model.files[0];
    let patch = build_line_patch(file, "f.txt", &sels(0, &[0, 1, 2]), LineOp::Stage).unwrap();
    apply(&repo, &patch, true, false).await;

    let engine = repo.engine();
    let after = engine
        .diff(
            &repo.path(),
            DiffSource::Worktree,
            None,
            &[String::from("f.txt")],
            DiffOptions::default(),
        )
        .await
        .unwrap();
    assert!(after.files.is_empty() || after.files[0].hunks.is_empty());
    let status = engine.status(&repo.path()).await.unwrap();
    let f = status.iter().find(|f| f.path == "f.txt").unwrap();
    assert!(f.staged && !f.unstaged);
}

// =====================
// 9. performance smoke: 100k-line diff (基准机目标: 解析层 < 500ms)
// =====================

#[tokio::test]
async fn parse_100k_line_diff_under_budget() {
    let repo = TempRepo::new("perf");
    let mut base = String::new();
    for i in 0..50000 {
        base.push_str(&format!("original line {i} with some padding text\n"));
    }
    repo.write("big.txt", &base);
    repo.commit_all("init");
    let mut modified = String::new();
    for i in 0..50000 {
        modified.push_str(&format!("CHANGED line {i} with some padding text\n"));
    }
    repo.write("big.txt", &modified);

    let started = std::time::Instant::now();
    let engine = repo.engine();
    let model = engine
        .diff(
            &repo.path(),
            DiffSource::Worktree,
            None,
            &[String::from("big.txt")],
            DiffOptions::default(),
        )
        .await
        .unwrap();
    let elapsed = started.elapsed();
    let total: usize = model
        .files
        .iter()
        .map(|f| f.hunks.iter().map(|h| h.lines.len()).sum::<usize>())
        .sum();
    assert!(total >= 100_000, "expected >=100k diff lines, got {total}");
    // Loose CI-safe bound; the 基准机 target (<500ms) is measured locally.
    assert!(
        elapsed.as_millis() < 5000,
        "100k-line diff pipeline took {}ms",
        elapsed.as_millis()
    );
    tracing::info!("100k-line diff end-to-end: {}ms", elapsed.as_millis());
}
