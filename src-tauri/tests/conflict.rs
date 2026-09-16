//! P8 integration tests against real git repositories: conflict models
//! (Content/AddAdd/DeleteModify/Binary/Rename), resolution flows, operation
//! state detection and the abort/continue/skip state machine — including
//! the crash-recovery acceptance path (a *fresh* engine instance must see
//! and cleanly resolve the on-disk state of an interrupted operation).

use ibexgit_lib::core::engine::conflict::ConflictType;
use ibexgit_lib::core::engine::{CliEngine, GitEngine, OperationKind};
use ibexgit_lib::core::runner::GitProcessRunner;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ibexgit-p8-{}-{}-{}",
        name,
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

/// `git init` with one retry: template copying on Windows occasionally
/// flakes with "File exists" under AV/indexer pressure.
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

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn commit_all(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
}

/// Standard two-branch divergence: base commit, ours edits `f.txt`
/// (plus `ours.txt`), theirs edits the same region (plus `theirs.txt`).
fn diverged_repo() -> (PathBuf, Arc<dyn GitEngine>) {
    let dir = temp_repo("diverged");
    git_init(&dir);
    git(&dir, &["config", "user.email", "p8@ibexgit.local"]);
    git(&dir, &["config", "user.name", "P8"]);
    git(&dir, &["config", "core.autocrlf", "false"]);
    write(&dir.join("f.txt"), "line1\nline2\nline3\n");
    write(&dir.join("keep.txt"), "keep\n");
    commit_all(&dir, "base");
    git(&dir, &["checkout", "-q", "-b", "feature"]);
    write(&dir.join("f.txt"), "line1\ntheirs line2\nline3\n");
    write(&dir.join("theirs.txt"), "theirs\n");
    commit_all(&dir, "feature change");
    git(&dir, &["checkout", "-q", "main"]);
    write(&dir.join("f.txt"), "line1\nours line2\nline3\n");
    write(&dir.join("ours.txt"), "ours\n");
    commit_all(&dir, "main change");
    (dir, engine())
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_conflict_full_flow() {
    let (dir, engine) = diverged_repo();
    let path = dir.display().to_string();

    // Merge without commit → conflict (bypass --no-edit semantics: run raw).
    let out = Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["merge", "--no-ff", "--no-commit", "feature"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "merge must conflict");

    // Operation state detected (fresh engine = "restarted app").
    let st = engine.operation_state(&path).await.unwrap().unwrap();
    assert_eq!(st.kind, OperationKind::Merge);
    assert!(st.message.is_some());

    // Conflict list: f.txt UU content, both adds present.
    let list = engine.conflict_list(&path).await.unwrap();
    let f = list
        .iter()
        .find(|c| c.path == "f.txt")
        .expect("f.txt conflicted");
    assert_eq!(f.code, "UU");
    assert_eq!(f.conflict_type, ConflictType::Content);
    assert!(f.editable);
    assert!(f.block_count >= 1);

    // Full model: three stages + block positions.
    let model = engine.conflict_model(&path, "f.txt").await.unwrap();
    assert!(model.has_base && model.has_current && model.has_incoming);
    assert!(model.editable);
    assert_eq!(model.block_count, model.blocks.len() as u32);
    let b = model.blocks[0];
    let lines: Vec<&str> = model
        .worktree_text
        .as_deref()
        .unwrap()
        .split('\n')
        .collect();
    assert!(lines[b.start as usize].starts_with("<<<<<<<"));
    assert!(lines[b.end as usize].starts_with(">>>>>>>"));

    // Resolve by editing: keep ours, validate marker guard first.
    let ours = model.current_text.clone().unwrap();
    let err = engine
        .resolve_conflict_text(&path, "f.txt", &model.worktree_text.clone().unwrap())
        .await;
    assert!(err.is_err(), "leftover markers must be rejected");

    engine
        .resolve_conflict_text(&path, "f.txt", &ours)
        .await
        .unwrap();

    // Still merging; resolved files are staged. Commit completes the merge.
    let st2 = engine.operation_state(&path).await.unwrap().unwrap();
    assert_eq!(st2.kind, OperationKind::Merge);
    let remaining = engine.conflict_list(&path).await.unwrap();
    assert!(remaining.iter().all(|c| c.path != "f.txt"));

    // Other side files (theirs.txt) are staged adds; commit the merge.
    engine
        .stage(&path, &["theirs.txt".to_string(), "ours.txt".to_string()])
        .await
        .unwrap();
    let msg = st2.message.clone().unwrap();
    engine
        .commit(&path, msg.lines().next().unwrap(), false, false)
        .await
        .unwrap();
    assert!(engine.operation_state(&path).await.unwrap().is_none());
    assert_eq!(
        git(&dir, &["rev-parse", "HEAD^2"]).trim().len(),
        40,
        "merge commit has two parents"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_conflict_side_resolution_and_abort() {
    let (dir, engine) = diverged_repo();
    let path = dir.display().to_string();

    let _ = Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["merge", "--no-ff", "--no-commit", "feature"])
        .output()
        .unwrap();

    // "theirs" for f.txt = stage 3 content lands in worktree + staged.
    engine
        .resolve_conflict_keep(&path, "f.txt", "theirs")
        .await
        .unwrap();
    let content = std::fs::read_to_string(dir.join("f.txt")).unwrap();
    assert_eq!(content, "line1\ntheirs line2\nline3\n");

    // Delete their extra file to finish resolution, then commit.
    engine
        .resolve_conflict_delete(&path, "theirs.txt")
        .await
        .unwrap();
    engine
        .stage(&path, &["ours.txt".to_string()])
        .await
        .unwrap();
    engine
        .commit(&path, "merge (theirs)", false, false)
        .await
        .unwrap();
    assert!(engine.operation_state(&path).await.unwrap().is_none());
    assert!(std::fs::read_to_string(dir.join("f.txt"))
        .unwrap()
        .contains("theirs line2"));
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_conflict_abort_restores_branch() {
    let (dir, engine) = diverged_repo();
    let path = dir.display().to_string();
    let main_before = git(&dir, &["rev-parse", "HEAD"]);

    let _ = Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["merge", "--no-ff", "--no-commit", "feature"])
        .output()
        .unwrap();
    assert!(engine.operation_state(&path).await.unwrap().is_some());

    engine.operation_abort(&path).await.unwrap();
    assert!(engine.operation_state(&path).await.unwrap().is_none());
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]).trim(), main_before.trim());
    assert!(engine.conflict_list(&path).await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn add_add_and_delete_modify_classification() {
    let dir = temp_repo("classify");
    git_init(&dir);
    git(&dir, &["config", "user.email", "p8@ibexgit.local"]);
    git(&dir, &["config", "user.name", "P8"]);
    let path = dir.display().to_string();
    write(&dir.join("base.txt"), "base\n");
    commit_all(&dir, "base");
    git(&dir, &["checkout", "-q", "-b", "feature"]);

    // AddAdd on new.txt; delete base.txt on theirs side.
    write(&dir.join("new.txt"), "theirs new\n");
    git(&dir, &["rm", "-q", "base.txt"]);
    commit_all(&dir, "feature adds+deletes");
    git(&dir, &["checkout", "-q", "main"]);
    write(&dir.join("new.txt"), "ours new\n");
    write(&dir.join("base.txt"), "base modified on main\n");
    commit_all(&dir, "main adds+modifies");

    let _ = Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["merge", "--no-ff", "--no-commit", "feature"])
        .output()
        .unwrap();

    let e0 = engine();
    let list = e0.conflict_list(&path).await.unwrap();
    let add_add = list.iter().find(|c| c.path == "new.txt").unwrap();
    assert_eq!(add_add.code, "AA");
    assert_eq!(add_add.conflict_type, ConflictType::AddAdd);

    // base.txt: modified on main, deleted on feature → UD.
    let del = list.iter().find(|c| c.path == "base.txt").unwrap();
    assert_eq!(del.code, "UD");
    assert_eq!(del.conflict_type, ConflictType::DeleteModify);
    let del_model = e0.conflict_model(&path, "base.txt").await.unwrap();
    assert!(del_model.has_base && del_model.has_current && !del_model.has_incoming);

    // Keep ours for the AddAdd file; keep the modified base.txt.
    let e = engine();
    e.resolve_conflict_keep(&path, "new.txt", "ours")
        .await
        .unwrap();
    e.resolve_conflict_keep(&path, "base.txt", "ours")
        .await
        .unwrap();
    assert!(e.conflict_list(&path).await.unwrap().is_empty());
    e.commit(&path, "resolved", false, false).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn binary_conflict_is_choose_side() {
    let dir = temp_repo("binary");
    git_init(&dir);
    git(&dir, &["config", "user.email", "p8@ibexgit.local"]);
    git(&dir, &["config", "user.name", "P8"]);
    let path = dir.display().to_string();
    write(&dir.join("b.bin"), "same\n");
    commit_all(&dir, "base");
    git(&dir, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(dir.join("b.bin"), b"theirs\x00bin\n").unwrap();
    commit_all(&dir, "theirs bin");
    git(&dir, &["checkout", "-q", "main"]);
    std::fs::write(dir.join("b.bin"), b"ours\x00bin\n").unwrap();
    commit_all(&dir, "ours bin");

    let _ = Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["merge", "--no-ff", "--no-commit", "feature"])
        .output()
        .unwrap();

    let e = engine();
    let list = e.conflict_list(&path).await.unwrap();
    let b = list.iter().find(|c| c.path == "b.bin").unwrap();
    assert_eq!(b.conflict_type, ConflictType::Binary);
    assert!(b.binary);
    assert!(!b.editable);

    e.resolve_conflict_keep(&path, "b.bin", "theirs")
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(dir.join("b.bin")).unwrap(),
        b"theirs\x00bin\n"
    );
    e.commit(&path, "merge binary", false, false).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn rename_rename_pair_detected() {
    let dir = temp_repo("rename");
    git_init(&dir);
    git(&dir, &["config", "user.email", "p8@ibexgit.local"]);
    git(&dir, &["config", "user.name", "P8"]);
    let path = dir.display().to_string();
    write(&dir.join("old.txt"), "content\n");
    commit_all(&dir, "base");
    git(&dir, &["checkout", "-q", "-b", "feature"]);
    git(&dir, &["mv", "old.txt", "theirs_name.txt"]);
    commit_all(&dir, "theirs rename");
    git(&dir, &["checkout", "-q", "main"]);
    git(&dir, &["mv", "old.txt", "ours_name.txt"]);
    commit_all(&dir, "ours rename");

    let _ = Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["merge", "--no-ff", "--no-commit", "feature"])
        .output()
        .unwrap();

    let e = engine();
    let list = e.conflict_list(&path).await.unwrap();
    let ours = list.iter().find(|c| c.path == "ours_name.txt").unwrap();
    let theirs = list.iter().find(|c| c.path == "theirs_name.txt").unwrap();
    assert_eq!(
        ours.conflict_type,
        ConflictType::Rename,
        "UD member of the 1to2 pair"
    );
    assert_eq!(
        theirs.conflict_type,
        ConflictType::Rename,
        "DU member of the 1to2 pair"
    );

    // Choose ours: keep ours_name.txt, drop theirs_name.txt and the
    // rename origin (old.txt, DD: index-only after both sides renamed it).
    e.resolve_conflict_keep(&path, "ours_name.txt", "ours")
        .await
        .unwrap();
    e.resolve_conflict_delete(&path, "theirs_name.txt")
        .await
        .unwrap();
    e.resolve_conflict_delete(&path, "old.txt").await.unwrap();
    assert!(e.conflict_list(&path).await.unwrap().is_empty());
    e.commit(&path, "ours rename wins", false, false)
        .await
        .unwrap();
    assert!(dir.join("ours_name.txt").exists());
    assert!(!dir.join("theirs_name.txt").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn rebase_conflict_state_machine() {
    let (dir, engine) = diverged_repo();
    let path = dir.display().to_string();
    let main_head = git(&dir, &["rev-parse", "HEAD"]).trim().to_string();

    // A second feature commit that applies cleanly after the conflicted one.
    git(&dir, &["checkout", "-q", "feature"]);
    write(
        &dir.join("extra.txt"),
        "extra
",
    );
    commit_all(&dir, "feature extra");

    let out = Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["rebase", "main"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "rebase must conflict");

    // Step bookkeeping + kind (fresh engine = restart resilience).
    let st = engine.operation_state(&path).await.unwrap().unwrap();
    assert_eq!(st.kind, OperationKind::Rebase);
    assert_eq!(st.total, Some(2));
    assert!(st.step.is_some());
    assert_eq!(st.onto.as_deref(), Some(main_head.as_str()));

    engine
        .resolve_conflict_text(&path, "f.txt", "line1\nrebased line2\nline3\n")
        .await
        .unwrap();

    // Skip drops the conflicted commit; the clean second step still lands
    // (replayed → new sha, but on top of main).
    engine.operation_skip(&path).await.unwrap();
    assert!(engine.operation_state(&path).await.unwrap().is_none());
    assert_eq!(
        git(&dir, &["rev-parse", "HEAD~"]).trim(),
        main_head.trim(),
        "skipped rebase lands the replayed extra commit on main"
    );
    assert!(dir.join("extra.txt").exists());
    assert!(std::fs::read_to_string(dir.join("f.txt"))
        .unwrap()
        .contains("ours line2"));
    let _ = out; // silence unused when assertions change
}

#[tokio::test(flavor = "multi_thread")]
async fn rebase_conflict_abort_restores_branch() {
    let (dir, engine) = diverged_repo();
    let path = dir.display().to_string();
    git(&dir, &["checkout", "-q", "feature"]);
    let feature_head = git(&dir, &["rev-parse", "HEAD"]).trim().to_string();

    let _ = Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["rebase", "main"])
        .output()
        .unwrap();
    engine.operation_abort(&path).await.unwrap();
    assert!(engine.operation_state(&path).await.unwrap().is_none());
    assert_eq!(
        git(&dir, &["rev-parse", "HEAD"]).trim(),
        feature_head.trim()
    );
    assert!(engine.conflict_list(&path).await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn cherry_pick_conflict_and_abort() {
    let (dir, engine) = diverged_repo();
    let path = dir.display().to_string();
    let main_head = git(&dir, &["rev-parse", "HEAD"]).trim().to_string();

    let feature_change = git(&dir, &["rev-parse", "feature"]);
    let _ = Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["cherry-pick", feature_change.trim()])
        .output()
        .unwrap();

    let st = engine.operation_state(&path).await.unwrap().unwrap();
    assert_eq!(st.kind, OperationKind::CherryPick);
    assert_eq!(st.onto.as_deref(), Some(feature_change.trim()));

    let list = engine.conflict_list(&path).await.unwrap();
    assert!(list.iter().any(|c| c.path == "f.txt"));
    engine.operation_abort(&path).await.unwrap();
    assert!(engine.operation_state(&path).await.unwrap().is_none());
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]).trim(), main_head.trim());
}

#[tokio::test(flavor = "multi_thread")]
async fn pull_conflict_resolves_like_merge() {
    // pull(merge) = fetch + merge; conflict → operation state Merge →
    // resolve → continue (== commit). Exercises the P8×P6 integration.
    let (dir, engine) = diverged_repo();
    let path = dir.display().to_string();
    let feature_head = git(&dir, &["rev-parse", "feature"]);

    let res = engine
        .pull(&path, Some("."), Some("feature"), None)
        .await
        .unwrap();
    assert!(!res.success, "pull must conflict");

    let st = engine.operation_state(&path).await.unwrap().unwrap();
    assert_eq!(st.kind, OperationKind::Merge);

    engine
        .resolve_conflict_text(&path, "f.txt", "line1\nmerged line2\nline3\n")
        .await
        .unwrap();
    engine
        .stage(&path, &["theirs.txt".to_string(), "ours.txt".to_string()])
        .await
        .unwrap();
    // merge --continue == commit with MERGE_MSG (editor no-ops via env).
    engine.operation_continue(&path).await.unwrap();
    assert!(engine.operation_state(&path).await.unwrap().is_none());
    assert_eq!(
        git(&dir, &["rev-parse", "HEAD^2"]).trim(),
        feature_head.trim()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn directory_file_conflict_detected() {
    let dir = temp_repo("dirfile");
    git_init(&dir);
    git(&dir, &["config", "user.email", "p8@ibexgit.local"]);
    git(&dir, &["config", "user.name", "P8"]);
    let path = dir.display().to_string();
    write(&dir.join("x.txt"), "x\n");
    commit_all(&dir, "base");
    git(&dir, &["checkout", "-q", "-b", "feature"]);
    git(&dir, &["rm", "-q", "x.txt"]);
    write(&dir.join("x.txt/inner.txt"), "dir content\n");
    commit_all(&dir, "feature turns x into dir");
    git(&dir, &["checkout", "-q", "main"]);
    write(&dir.join("x.txt"), "modified file x\n");
    commit_all(&dir, "main modifies x");

    let _ = Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["merge", "--no-ff", "--no-commit", "feature"])
        .output()
        .unwrap();

    let e = engine();
    let list = e.conflict_list(&path).await.unwrap();
    // git parks the losing file version at `x.txt~HEAD` while the
    // directory occupies the plain path.
    let x = list
        .iter()
        .find(|c| c.path.starts_with("x.txt~"))
        .expect("suffixed file/directory conflict entry");
    assert_eq!(x.conflict_type, ConflictType::DirectoryFile);
    assert!(x.directory);
    assert!(!x.editable);

    // keep theirs (the directory) = drop the suffixed file entry.
    e.resolve_conflict_delete(&path, &x.path).await.unwrap();
    assert!(e
        .conflict_list(&path)
        .await
        .unwrap()
        .iter()
        .all(|c| !c.path.starts_with("x.txt~")));
    e.stage(&path, &["x.txt/inner.txt".to_string()])
        .await
        .unwrap();
    e.commit(&path, "dir wins", false, false).await.unwrap();
    assert!(dir.join("x.txt").is_dir());
}
