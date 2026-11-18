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
            * 1000
            + {
                use std::sync::atomic::Ordering;
                static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                N.fetch_add(1, Ordering::Relaxed) as u128
            }
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

    // 2b. fully-untracked directory expands into per-file entries (no
    // collapsed `dir/` record — flat list / per-file diff / discard depend
    // on it).
    std::fs::create_dir_all(dir.join("newdir")).unwrap();
    std::fs::write(dir.join("newdir/a.txt"), "a\n").unwrap();
    std::fs::write(dir.join("newdir/b.txt"), "b\n").unwrap();
    let status = engine.status(&path).await.expect("status");
    let mut untracked_paths: Vec<&str> = status
        .iter()
        .filter(|f| f.untracked)
        .map(|f| f.path.as_str())
        .collect();
    untracked_paths.sort_unstable();
    assert_eq!(
        untracked_paths,
        vec!["hello.txt", "newdir/a.txt", "newdir/b.txt"],
        "untracked directory must expand to per-file entries"
    );
    std::fs::remove_dir_all(dir.join("newdir")).unwrap();

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

/// 固定 committer 日期的提交（统计分桶的确定性来源）。
fn git_commit_at(dir: &PathBuf, name: &str, email: &str, date: &str, msg: &str) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            &format!("user.name={name}"),
            "-c",
            &format!("user.email={email}"),
            "commit",
            "-m",
            msg,
        ])
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .expect("git available");
    assert!(
        out.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn commit_stats_empty_repo_and_two_authors() {
    let engine = CliEngine::new(GitProcessRunner::new(60), "git");

    // 1. unborn repo: stats is an empty set, not an error
    let dir = temp_repo();
    git(&dir, &["init", "-q", "-b", "main"]);
    let path = dir.display().to_string();
    let empty = engine
        .commit_stats(&path, "HEAD")
        .await
        .expect("stats empty");
    assert_eq!(empty.all.total, 0);
    assert!(empty.all.contributors.is_empty());
    assert!(empty.months.is_empty());
    let _ = std::fs::remove_dir_all(&dir);

    // 2. two authors across three months (fixed committer dates)
    let dir = temp_repo();
    git(&dir, &["init", "-q", "-b", "main"]);
    let path = dir.display().to_string();
    std::fs::write(dir.join("a.txt"), "a\n").unwrap();
    git(&dir, &["add", "."]);
    git_commit_at(
        &dir,
        "Alice",
        "alice@x",
        "2026-01-10T10:00:00+08:00",
        "s: c1",
    );
    std::fs::write(dir.join("b.txt"), "b\n").unwrap();
    git(&dir, &["add", "."]);
    git_commit_at(&dir, "Bob", "bob@x", "2026-01-20T10:00:00+08:00", "s: c2");
    std::fs::write(dir.join("c.txt"), "c\n").unwrap();
    git(&dir, &["add", "."]);
    git_commit_at(
        &dir,
        "Alice",
        "alice@x",
        "2026-03-05T10:00:00+08:00",
        "s: c3",
    );

    let dto = engine.commit_stats(&path, "main").await.expect("stats");
    assert_eq!(dto.all.total, 3);
    assert_eq!(dto.all.contributors.len(), 2);
    assert_eq!(dto.all.contributors[0].name, "Alice");
    assert_eq!(dto.all.contributors[0].count, 2);
    assert_eq!(dto.all.contributors[1].email, "bob@x");
    assert_eq!(dto.all.contributors[1].count, 1);
    // 固定提交日期全在过去：当前周期（本月/本周/本日）合计为 0。
    assert_eq!(dto.month.total, 0);
    assert_eq!(dto.week.total, 0);
    assert_eq!(dto.today.total, 0);

    // 月轴从首个提交月铺起、零填充；历史提交不进当前周期的桶。
    assert_eq!(dto.months[0].key, "2026-01");
    assert_eq!(dto.months[0].count, 2);
    assert_eq!(dto.months[1].key, "2026-02");
    assert_eq!(dto.months[1].count, 0);
    assert_eq!(dto.months[2].key, "2026-03");
    assert_eq!(dto.months[2].count, 1);
    assert!(dto.months.len() >= 3);
    assert!(dto.today_hours.iter().all(|b| b.count == 0));

    let _ = std::fs::remove_dir_all(&dir);
}

/// EOL 探针（`mark_eol_only`）：只改行尾的文件标 `eol_only`，真改内容的不标。
///
/// 该探针是 status 读取里最贵的一步（两次全树 `git diff`，实测 2.5k
/// untracked 时占 345ms 中的 188ms），现在按候选路径限定并并发执行。这个
/// 用例同时守住"限定 pathspec 不改变判定"：候选里既有只改行尾的文件，也有
/// 真改内容的文件，还有一个带子目录路径（pathspec 走 stdin，不经命令行）。
#[tokio::test(flavor = "multi_thread")]
async fn status_flags_eol_only_changes() {
    let dir = temp_repo();
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["config", "user.email", "smoke@ibexgit.local"]);
    git(&dir, &["config", "user.name", "Smoke"]);
    // 关掉自动换行转换：blob 里的 LF 必须原样留在工作区，否则没有"只改行尾"。
    git(&dir, &["config", "core.autocrlf", "false"]);

    std::fs::write(dir.join("crlf.txt"), "one\ntwo\n").unwrap();
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub/real.txt"), "alpha\n").unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-q", "-m", "base"]);

    // 行尾变更（内容相同）+ 真实内容变更 + 未跟踪文件。
    std::fs::write(dir.join("crlf.txt"), "one\r\ntwo\r\n").unwrap();
    std::fs::write(dir.join("sub/real.txt"), "alpha\nbeta\n").unwrap();
    std::fs::write(dir.join("untracked.txt"), "u\n").unwrap();

    let engine = Arc::new(CliEngine::new(GitProcessRunner::new(60), "git"));
    let status = engine
        .status(&dir.display().to_string())
        .await
        .expect("status");

    let get = |p: &str| {
        status
            .iter()
            .find(|f| f.path == p)
            .unwrap_or_else(|| panic!("missing status entry for {p}: {status:?}"))
    };
    assert!(get("crlf.txt").eol_only, "CRLF-only change must be flagged");
    assert!(
        !get("sub/real.txt").eol_only,
        "real content change is not EOL-only"
    );
    assert!(
        !get("untracked.txt").eol_only,
        "untracked entries are never EOL-only"
    );

    // 同一份工作区再来一次：路径限定分支已被缓存/索引状态考验过一遍。
    let again = engine
        .status(&dir.display().to_string())
        .await
        .expect("status");
    assert!(
        again
            .iter()
            .find(|f| f.path == "crlf.txt")
            .unwrap()
            .eol_only
    );

    let _ = std::fs::remove_dir_all(&dir);
}
