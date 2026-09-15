//! P1 acceptance smoke loop: status → stage → commit → log against a real
//! git repository (PLAN §P1 验收).

use ibexgit_lib::core::engine::{CliEngine, DiffOptions, DiffSource, GitEngine};
use ibexgit_lib::core::runner::{GitProcessRunner, StdinMode};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_repo() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ibexgit-smoke-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
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

#[tokio::test(flavor = "multi_thread")]
async fn status_stage_commit_log_smoke_loop() {
    let dir = temp_repo();
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["config", "user.email", "smoke@ibexgit.local"]);
    git(&dir, &["config", "user.name", "Smoke"]);

    let engine = CliEngine::new(GitProcessRunner::new(60), "git");
    let engine = Arc::new(engine);
    let path = dir.display().to_string();

    // 1. status: empty worktree
    let status = engine.status(&path).await.expect("status");
    assert!(status.is_empty(), "fresh repo should be clean");

    // 2. write a file → status shows untracked
    std::fs::write(dir.join("hello.txt"), "hello world\n").unwrap();
    let status = engine.status(&path).await.expect("status");
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].path, "hello.txt");
    assert!(status[0].untracked);
    assert!(!status[0].staged);

    // 3. stage → status shows staged
    engine
        .stage(&path, &["hello.txt".to_string()])
        .await
        .expect("stage");
    let status = engine.status(&path).await.expect("status");
    assert_eq!(status.len(), 1);
    assert!(status[0].staged);
    assert!(!status[0].untracked);

    // 4. diff (staged): the added file is visible
    let diff = engine
        .diff(&path, DiffSource::Staged, None, &[], DiffOptions::default())
        .await
        .expect("diff staged");
    assert_eq!(diff.files.len(), 1);
    assert_eq!(diff.files[0].new_path.as_deref(), Some("hello.txt"));

    // 5. commit → returns a hash
    let commit = engine
        .commit(&path, "smoke: first commit", false, false)
        .await
        .expect("commit");
    assert_eq!(commit.message, "smoke: first commit");
    assert!(commit.hash.len() >= 7, "hash: {}", commit.hash);

    // 6. status clean again
    let status = engine.status(&path).await.expect("status");
    assert!(status.is_empty(), "post-commit should be clean");

    // 7. log → exactly the commit, with matching hash and no parents
    let log = engine.log(&path, 10, 0, &[]).await.expect("log");
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].hash, commit.hash);
    assert_eq!(log[0].message, "smoke: first commit");
    assert_eq!(log[0].author, "Smoke");
    assert!(log[0].parents.is_empty(), "root commit has no parents");
    assert!(log[0].refs.iter().any(|r| r.contains("main")));

    // 8. second file + commit → log grows, parents link up
    std::fs::write(dir.join("second.txt"), "second\n").unwrap();
    engine
        .stage(&path, &["second.txt".to_string()])
        .await
        .expect("stage 2");
    let commit2 = engine
        .commit(&path, "smoke: second commit", false, false)
        .await
        .expect("commit 2");
    let log = engine.log(&path, 10, 0, &[]).await.expect("log 2");
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].hash, commit2.hash, "log is newest-first");
    assert_eq!(log[0].parents, vec![commit.hash.clone()]);

    // 9. worktree diff is empty; commit-range diff shows the second commit
    let diff = engine
        .diff(
            &path,
            DiffSource::Worktree,
            None,
            &[],
            DiffOptions::default(),
        )
        .await
        .expect("diff worktree");
    assert!(diff.files.is_empty(), "clean worktree has no diff");

    // 10. amend changes the message without adding a commit
    let amended = engine
        .commit(&path, "smoke: second commit (amended)", true, false)
        .await
        .expect("amend");
    let log = engine.log(&path, 10, 0, &[]).await.expect("log 3");
    assert_eq!(log.len(), 2, "amend must not add a commit");
    assert_eq!(log[0].message, "smoke: second commit (amended)");
    assert_ne!(log[0].hash, commit2.hash, "amend rewrites the hash");
    assert_eq!(amended.short_hash.len(), 7);

    let _ = std::fs::remove_dir_all(&dir);
    let _ = StdinMode::Null; // keep import used if assertions change
}
