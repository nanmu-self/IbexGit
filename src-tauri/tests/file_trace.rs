//! P9 integration tests against real git repositories: single-file history
//! with rename following (`log --follow`) and worktree blame
//! (`blame --porcelain`), including uncommitted lines and pagination.

use ibexgit_lib::core::engine::{CliEngine, GitEngine};
use ibexgit_lib::core::runner::GitProcessRunner;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_repo() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ibexgit-p9-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// `git init` with one retry (Windows template flake under AV pressure).
fn git_init(dir: &Path) {
    let args = ["init", "-q", "-b", "main"];
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    if !out.status.success() {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let retry = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            retry.status.success(),
            "git init failed: {}",
            String::from_utf8_lossy(&retry.stderr)
        );
    }
}

fn engine() -> Arc<dyn GitEngine> {
    Arc::new(CliEngine::new(GitProcessRunner::new(60), "git"))
}

/// c1: f.txt = "a1\na2\n" → c2: append "a3\n" → rename to d/g.txt
/// → c4: append "b4\n". Returns (dir, [c1, c2, rename, c4] hashes).
fn setup_history() -> (PathBuf, Vec<String>) {
    let dir = temp_repo();
    git_init(&dir);
    git(&dir, &["config", "user.email", "p9@ibexgit.local"]);
    git(&dir, &["config", "user.name", "P9"]);
    git(&dir, &["config", "core.autocrlf", "false"]);
    std::fs::write(dir.join("f.txt"), "a1\na2\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "c1"]);
    std::fs::write(dir.join("f.txt"), "a1\na2\na3\n").unwrap();
    git(&dir, &["commit", "-qam", "c2"]);
    std::fs::create_dir_all(dir.join("d")).unwrap();
    git(&dir, &["mv", "f.txt", "d/g.txt"]);
    git(&dir, &["commit", "-q", "-m", "rename"]);
    std::fs::write(dir.join("d/g.txt"), "a1\na2\na3\nb4\n").unwrap();
    git(&dir, &["commit", "-qam", "c4"]);
    let commits = ["c1", "c2", "rename", "c4"]
        .iter()
        .map(|m| {
            let out = git(&dir, &["log", "--format=%H", "--grep", m, "-n1"]);
            out.trim().to_string()
        })
        .collect();
    (dir, commits)
}

#[tokio::test(flavor = "multi_thread")]
async fn file_history_follows_rename() {
    let (dir, commits) = setup_history();
    let path = dir.display().to_string();
    let engine = engine();

    let history = engine
        .file_history(&path, "d/g.txt", 100, None)
        .await
        .unwrap();
    assert_eq!(history.len(), 4, "follow must reach pre-rename commits");
    // Newest first.
    assert_eq!(history[0].hash, commits[3], "c4");
    assert_eq!(history[0].path, "d/g.txt");
    assert_eq!(history[0].status, "M");
    assert_eq!(history[1].hash, commits[2], "rename commit");
    assert_eq!(history[1].status, "R");
    assert_eq!(history[1].orig_path.as_deref(), Some("f.txt"));
    assert_eq!(history[1].path, "d/g.txt");
    // Pre-rename entries keep the old path (porcelain filename semantics).
    assert_eq!(history[2].hash, commits[1]);
    assert_eq!(history[2].path, "f.txt");
    assert_eq!(history[3].hash, commits[0]);
    assert_eq!(history[3].path, "f.txt");
    // First-parent chain is intact for diff construction.
    assert_eq!(history[0].parents, vec![commits[2].clone()]);
    assert!(history[3].parents.is_empty(), "root commit");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn file_history_paginates_by_cursor_across_rename() {
    let (dir, _commits) = setup_history();
    let path = dir.display().to_string();
    let engine = engine();

    // Page 1 → cursor → page 2 must continue past the rename boundary.
    let page1 = engine
        .file_history(&path, "d/g.txt", 2, None)
        .await
        .unwrap();
    assert_eq!(page1.len(), 2);
    let cursor = page1[1].hash.clone();
    let page2 = engine
        .file_history(&path, "d/g.txt", 2, Some(&cursor))
        .await
        .unwrap();
    assert_eq!(page2.len(), 2);
    // Pages are disjoint slices of the full list, in order.
    let full = engine
        .file_history(&path, "d/g.txt", 100, None)
        .await
        .unwrap();
    assert_eq!(page1[0].hash, full[0].hash);
    assert_eq!(page1[1].hash, full[1].hash);
    assert_eq!(page2[0].hash, full[2].hash);
    assert_eq!(page2[1].hash, full[3].hash);
    // Cursor at the rename commit: the next page still reaches f.txt.
    assert_eq!(page1[1].status, "R");
    assert_eq!(page2[0].path, "f.txt");

    // Last page cursor (root commit) → empty page = complete.
    let cursor3 = full[3].hash.clone();
    let page3 = engine
        .file_history(&path, "d/g.txt", 2, Some(&cursor3))
        .await
        .unwrap();
    assert!(page3.is_empty(), "cursor at root commit → complete");

    // Path that never existed → empty history, no error.
    let empty = engine
        .file_history(&path, "no/such/file.bin", 100, None)
        .await
        .unwrap();
    assert!(empty.is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn blame_attributes_lines_and_marks_uncommitted() {
    let (dir, commits) = setup_history();
    let path = dir.display().to_string();
    let engine = engine();

    // Clean worktree: every line attributed to a commit.
    let blame = engine.blame(&path, "d/g.txt").await.unwrap();
    assert_eq!(blame.lines.len(), 4);
    // The rename commit contributed no surviving lines → not in the list.
    assert_eq!(blame.commits.len(), 3);
    for (i, line) in blame.lines.iter().enumerate() {
        assert_eq!(line.final_no, (i + 1) as u32);
        assert!(!blame.commits[line.commit as usize].uncommitted);
    }
    assert_eq!(blame.lines[0].content, "a1");
    assert_eq!(
        blame.commits[blame.lines[0].commit as usize].hash, commits[0],
        "line 1 comes from c1"
    );
    assert_eq!(blame.lines[3].content, "b4");
    assert_eq!(
        blame.commits[blame.lines[3].commit as usize].hash, commits[3],
        "line 4 comes from c4"
    );
    // Lines predating the rename report the old path (porcelain filename).
    assert_eq!(blame.commits[blame.lines[0].commit as usize].path, "f.txt");
    assert_eq!(
        blame.commits[blame.lines[3].commit as usize].path,
        "d/g.txt"
    );
    // Metadata sanity: author + ISO date with the commit-timezone offset.
    let c = &blame.commits[blame.lines[0].commit as usize];
    assert_eq!(c.author, "P9");
    assert_eq!(c.email, "p9@ibexgit.local");
    assert!(c.date.contains('T'), "ISO date: {}", c.date);
    assert!(c.summary.starts_with('c'));

    // Dirty worktree: changed line + appended line → uncommitted marks.
    std::fs::write(dir.join("d/g.txt"), "a1\nXX\na3\nb4\nNEW\n").unwrap();
    let blame = engine.blame(&path, "d/g.txt").await.unwrap();
    assert_eq!(blame.lines.len(), 5);
    let uncommitted: Vec<_> = blame
        .lines
        .iter()
        .filter(|l| blame.commits[l.commit as usize].uncommitted)
        .collect();
    assert_eq!(uncommitted.len(), 2, "changed line 2 + appended line 5");
    assert_eq!(uncommitted[0].final_no, 2);
    assert_eq!(uncommitted[1].final_no, 5);
    assert_eq!(uncommitted[1].content, "NEW");
    // Committed lines keep their attribution across the dirty state.
    assert_eq!(
        blame.commits[blame.lines[0].commit as usize].hash,
        commits[0]
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn blame_empty_file_yields_empty_result() {
    let dir = temp_repo();
    git_init(&dir);
    git(&dir, &["config", "user.email", "p9@ibexgit.local"]);
    git(&dir, &["config", "user.name", "P9"]);
    std::fs::write(dir.join("empty.txt"), "").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "c1"]);
    let engine = engine();
    let blame = engine
        .blame(&dir.display().to_string(), "empty.txt")
        .await
        .unwrap();
    assert!(blame.commits.is_empty());
    assert!(blame.lines.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn file_history_unborn_repo_is_empty_not_error() {
    let dir = temp_repo();
    git_init(&dir);
    let engine = engine();
    let history = engine
        .file_history(&dir.display().to_string(), "any.txt", 100, None)
        .await
        .unwrap();
    assert!(history.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
