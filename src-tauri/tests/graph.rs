//! P5 integration tests: commit-graph pipeline (GraphQuery → GraphCache →
//! GraphLayout), history operations (cherry-pick / revert / squash /
//! restore file version) against real git repositories.
//!
//! Fixture repos are built programmatically; timeouts are generous (PLAN:
//! git must be on PATH for integration tests).

use ibexgit_lib::core::engine::{CliEngine, GitEngine, GraphFilter};
use ibexgit_lib::core::graph::LayoutState;
use ibexgit_lib::core::repo::RepoManager;
use ibexgit_lib::core::runner::GitProcessRunner;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_repo(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ibexgit-graph-{}-{}-{}",
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
    dir
}

/// `git init` without hook templates (avoids the Windows template-copy
/// race between parallel test fns) and with a deterministic EOL setup:
/// the global `core.autocrlf` must not flip fixture content to CRLF.
fn init_repo(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main", "--template="]);
    git(dir, &["config", "core.autocrlf", "false"]);
}

fn git(dir: &Path, args: &[&str]) {
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

fn git_out(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn engine() -> Arc<CliEngine> {
    Arc::new(CliEngine::new(GitProcessRunner::new(60), "git"))
}

// =====================\n// layout invariants over engine output
// =====================

/// Verify rendering invariants over laid-out rows: every edge crossing a
/// row boundary continues on the next row at the same lane.
fn assert_connected(rows: &[ibexgit_lib::core::graph::GraphRow]) {
    for w in rows.windows(2) {
        let (lower, upper) = (&w[0], &w[1]);
        for e in &lower.edges {
            let arrive = if e.from_node {
                Some(e.to)
            } else if e.to_node {
                None
            } else {
                Some(e.from)
            };
            let Some(arrive) = arrive else { continue };
            let found = upper.edges.iter().any(|u| match (u.from_node, u.to_node) {
                (_, true) => u.from == arrive && upper.lane == arrive,
                (false, false) => u.from == arrive,
                (true, false) => false,
            });
            assert!(
                found,
                "edge to lane {arrive} below {} must continue on {}",
                lower.commit.hash, upper.commit.hash
            );
        }
    }
}

/// Fixture: main line with N feature branches, each forking at HEAD-ward
/// points, accumulating 2 commits and merging back (merge commits with 2
/// parents). Returns the dir.
fn build_merged_repo(n_branches: usize) -> PathBuf {
    let dir = temp_repo("branches");
    init_repo(&dir);
    git(&dir, &["config", "user.email", "graph@ibexgit.local"]);
    git(&dir, &["config", "user.name", "Graph"]);
    for i in 0..n_branches {
        std::fs::write(dir.join(format!("f{i}.txt")), format!("base {i}\n")).unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", &format!("base {i}")]);
    }
    for i in 0..n_branches {
        let name = format!("feat{i}");
        git(&dir, &["checkout", "-q", "-b", &name]);
        for j in 0..2 {
            std::fs::write(dir.join(format!("feat{i}-{j}.txt")), format!("{i}-{j}\n")).unwrap();
            git(&dir, &["add", "."]);
            git(&dir, &["commit", "-q", "-m", &format!("feat {i} work {j}")]);
        }
        git(&dir, &["checkout", "-q", "main"]);
        git(
            &dir,
            &[
                "merge",
                "-q",
                "--no-ff",
                "-m",
                &format!("merge {name}"),
                &name,
            ],
        );
    }
    dir
}

#[tokio::test(flavor = "multi_thread")]
async fn graph_pages_lay_out_merged_history() {
    let dir = build_merged_repo(6);
    let mgr = RepoManager::new(engine());
    let id = mgr.open(dir.clone()).await.unwrap();

    // Page 1: full fixture is < 500 → complete, all rows present.
    let page = mgr
        .graph_page(id, &GraphFilter::default(), false, 500)
        .await
        .unwrap();
    assert!(page.complete);
    assert_eq!(page.start, 0);
    let expected = 6 /* base */ + 6 * (2 /* branch */ + 1 /* merge */);
    assert_eq!(page.rows.len(), expected);
    assert_connected(&page.rows);
    // Merges actually merge: some node receives 2+ inflow edges over the
    // graph (diagonal merge-ins land at row boundaries; the merge commit
    // itself has 2 parents and its lane fans out).
    assert!(page.rows.iter().any(|r| r.commit.parents.len() == 2));
    assert!(page.width >= 2, "merged branches must use >1 lane");

    // `more` on a complete graph: empty delta, same start.
    let more = mgr
        .graph_page(id, &GraphFilter::default(), true, 500)
        .await
        .unwrap();
    assert!(more.complete);
    assert!(more.rows.is_empty());
    assert_eq!(more.start as usize, expected);

    // Filter: only commits of one branch file.
    let f = GraphFilter {
        paths: vec!["feat3-1.txt".into()],
        ..Default::default()
    };
    let page = mgr.graph_page(id, &f, false, 500).await.unwrap();
    assert!(!page.rows.is_empty());
    assert!(page
        .rows
        .iter()
        .all(|r| r.commit.message.contains("feat 3") || r.commit.message.contains("base")));

    // Cache invalidation: a new commit bumps generation → rebuild.
    std::fs::write(dir.join("after.txt"), "after\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "after"]);
    let page2 = mgr
        .graph_page(id, &GraphFilter::default(), false, 500)
        .await
        .unwrap();
    assert_eq!(page2.rows.len(), expected + 1);
    assert_eq!(page2.rows[0].commit.message, "after");
    assert_connected(&page2.rows);

    mgr.close(id).await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// 回归：首页请求（more=false）在缓存有效且 complete 时必须返回缓存 rows，
/// 而不是 `more` 专用的空 delta —— 否则每次点刷新/切分支后（缓存未失效、
/// 前端 reload 首页）列表会被空页清空（历史"消失"）。
#[tokio::test(flavor = "multi_thread")]
async fn graph_first_page_on_fresh_cache_returns_rows() {
    let dir = build_merged_repo(3);
    let mgr = RepoManager::new(engine());
    let id = mgr.open(dir.clone()).await.unwrap();

    let expected = 3 /* base */ + 3 * (2 /* branch */ + 1 /* merge */);
    let first = mgr
        .graph_page(id, &GraphFilter::default(), false, 500)
        .await
        .unwrap();
    assert_eq!(first.rows.len(), expected);
    assert!(first.complete);

    // 模拟前端刷新（F5 / watcher 事件后的 reload）：缓存有效 → 再次首页请求。
    for _ in 0..3 {
        let again = mgr
            .graph_page(id, &GraphFilter::default(), false, 500)
            .await
            .unwrap();
        assert_eq!(again.start, 0);
        assert_eq!(
            again.rows.len(),
            expected,
            "first page must never be an empty delta"
        );
        assert_eq!(
            again.rows.first().unwrap().commit.hash,
            first.rows.first().unwrap().commit.hash
        );
    }

    // `more` 语义保持不变：complete 缓存上的翻页仍是空 delta。
    let more = mgr
        .graph_page(id, &GraphFilter::default(), true, 500)
        .await
        .unwrap();
    assert!(more.rows.is_empty());
    assert_eq!(more.start as usize, expected);

    mgr.close(id).await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// PLAN P5 验收：50+ 分支合并的图形正确（连通性不变量 + lane 上限合理）。
#[tokio::test(flavor = "multi_thread")]
async fn graph_50_branches_merge_correctly() {
    let dir = build_merged_repo(52);
    let mgr = RepoManager::new(engine());
    let id = mgr.open(dir.clone()).await.unwrap();
    let page = mgr
        .graph_page(id, &GraphFilter::default(), false, 500)
        .await
        .unwrap();
    assert!(page.complete);
    let expected = 52 + 52 * 3;
    assert_eq!(page.rows.len(), expected);
    assert_connected(&page.rows);
    // Free-list reuse keeps concurrency bounded: never wider than
    // main + all 52 branch chains being alive at once is impossible here
    // (2-commit chains) — sanity bound only.
    assert!(
        page.width <= 8,
        "width {} blew past sane concurrency",
        page.width
    );
    mgr.close(id).await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// 分批加载：batch=3 逐页取数，累计结果与一次性取数一致（lane 连续）。
#[tokio::test(flavor = "multi_thread")]
async fn graph_incremental_batches_match_single_pass() {
    let dir = build_merged_repo(5);
    let mgr = RepoManager::new(engine());
    let id = mgr.open(dir.clone()).await.unwrap();

    // Accumulate via `more` with a tiny batch.
    let mut accumulated: Vec<ibexgit_lib::core::graph::GraphRow> = Vec::new();
    let filter = GraphFilter::default();
    let mut first = true;
    loop {
        let page = mgr.graph_page(id, &filter, !first, 3).await.unwrap();
        if page.start == 0 {
            accumulated = page.rows.clone();
        } else {
            assert_eq!(page.start as usize, accumulated.len(), "delta must append");
            accumulated.extend(page.rows);
        }
        if page.complete {
            break;
        }
        first = false;
    }

    // Single pass over the same commits must equal the accumulated layout.
    let engine = mgr.engine();
    let path = mgr.get_path(id).await.unwrap();
    let all = engine
        .graph(&path.display().to_string(), 0, 10_000, &filter)
        .await
        .unwrap();
    let mut state = LayoutState::new();
    let mut rows = Vec::new();
    state.layout(&all, &mut rows);
    assert_eq!(rows.len(), accumulated.len());
    for (a, b) in rows.iter().zip(accumulated.iter()) {
        assert_eq!(a.commit.hash, b.commit.hash);
        assert_eq!(a.lane, b.lane);
        assert_eq!(a.edges, b.edges);
    }
    mgr.close(id).await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

// =====================\n// history operations
// =====================\n

fn simple_repo() -> (PathBuf, Arc<CliEngine>, String) {
    let dir = temp_repo("ops");
    init_repo(&dir);
    git(&dir, &["config", "user.email", "graph@ibexgit.local"]);
    git(&dir, &["config", "user.name", "Graph"]);
    let e = engine();
    let p = dir.display().to_string();
    (dir, e, p)
}

fn write_commit(dir: &Path, file: &str, content: &str, msg: &str) -> String {
    std::fs::write(dir.join(file), content).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "-q", "-m", msg]);
    git_out(dir, &["rev-parse", "HEAD"])
}

#[tokio::test(flavor = "multi_thread")]
async fn commit_detail_lists_files_and_handles_merge_root() {
    let (dir, e, p) = simple_repo();

    // Root commit.
    let root = write_commit(&dir, "a.txt", "one\n", "root");
    let detail = e.commit_detail(&p, &root).await.unwrap();
    assert_eq!(detail.parent, None, "root has no parent");
    assert_eq!(detail.files.len(), 1);
    assert_eq!(detail.files[0].status, "A");
    assert_eq!(detail.files[0].path, "a.txt");

    let second = write_commit(&dir, "a.txt", "two\n", "second");
    let detail = e.commit_detail(&p, &second).await.unwrap();
    assert_eq!(detail.parent.as_deref(), Some(root.as_str()));
    assert_eq!(detail.files[0].status, "M");

    // Rename.
    git(&dir, &["mv", "a.txt", "b.txt"]);
    git(&dir, &["commit", "-q", "-m", "rename"]);
    let head = git_out(&dir, &["rev-parse", "HEAD"]);
    let detail = e.commit_detail(&p, &head).await.unwrap();
    assert_eq!(detail.files.len(), 1);
    assert_eq!(detail.files[0].status, "R");
    assert_eq!(detail.files[0].path, "b.txt");
    assert_eq!(detail.files[0].orig_path.as_deref(), Some("a.txt"));

    // Merge commit: first-parent diff.
    git(&dir, &["checkout", "-q", "-b", "side", "HEAD~2"]);

    std::fs::write(dir.join("side.txt"), "side\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "side work"]);
    git(&dir, &["checkout", "-q", "main"]);
    git(
        &dir,
        &["merge", "-q", "--no-ff", "-m", "merge side", "side"],
    );
    let merge = git_out(&dir, &["rev-parse", "HEAD"]);
    let detail = e.commit_detail(&p, &merge).await.unwrap();
    // Diff against first parent (main before merge): side.txt added.
    assert_eq!(detail.files.len(), 1);
    assert_eq!(detail.files[0].path, "side.txt");
    assert_eq!(detail.files[0].status, "A");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn cherry_pick_then_revert_roundtrip() {
    let (dir, e, p) = simple_repo();
    write_commit(&dir, "base.txt", "base\n", "base");

    // Feature branch with one commit to pick.
    git(&dir, &["checkout", "-q", "-b", "feat"]);
    let feat_commit = write_commit(&dir, "feat.txt", "feature\n", "feat work");
    git(&dir, &["checkout", "-q", "main"]);
    let before = git_out(&dir, &["rev-parse", "HEAD"]);

    e.cherry_pick(&p, std::slice::from_ref(&feat_commit))
        .await
        .unwrap();
    let after = git_out(&dir, &["rev-parse", "HEAD"]);
    assert_ne!(before, after, "cherry-pick must create a commit");
    assert!(dir.join("feat.txt").exists());

    // Revert undoes it.
    e.revert(&p, &[after]).await.unwrap();
    assert!(!dir.join("feat.txt").exists(), "revert removes the file");
    let _ = feat_commit;

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn squash_collapses_top_commits_and_validates() {
    let (dir, e, p) = simple_repo();
    write_commit(&dir, "base.txt", "base\n", "base");
    let h1 = write_commit(&dir, "x.txt", "1\n", "one");
    let h2 = write_commit(&dir, "y.txt", "2\n", "two");
    let head = git_out(&dir, &["rev-parse", "HEAD"]);
    assert_eq!(head, h2);
    let tree_before = git_out(&dir, &["rev-parse", "HEAD^{tree}"]);

    // Squash the top two (newest-first list from the UI).
    e.reset(&p, "soft", &h1).await.unwrap();
    // (validate via command-level helper is covered by the UI contract;
    // here we exercise the same reset+commit the command performs)
    e.commit(&p, "squashed", false, false).await.unwrap();
    let tree_after = git_out(&dir, &["rev-parse", "HEAD^{tree}"]);
    assert_eq!(tree_before, tree_after, "squash must preserve the tree");
    assert_eq!(git_out(&dir, &["log", "-1", "--format=%s"]), "squashed");
    // Base survived as parent.
    let parent = git_out(&dir, &["rev-parse", "HEAD~1"]);
    assert_eq!(parent, git_out(&dir, &["rev-parse", "main~1"]));

    let _ = h2;
    let _ = std::fs::remove_dir_all(&dir);
}

/// 恢复此文件版本：worktree-only restore + track-A snapshot undo.
#[tokio::test(flavor = "multi_thread")]
async fn restore_file_version_with_snapshot_undo() {
    use ibexgit_lib::core::recovery::RecoveryManager;

    let dir = temp_repo("restore");
    init_repo(&dir);
    git(&dir, &["config", "user.email", "graph@ibexgit.local"]);
    git(&dir, &["config", "user.name", "Graph"]);
    let e = engine();
    let p = dir.display().to_string();

    let old = write_commit(&dir, "f.txt", "old content\n", "old");
    write_commit(&dir, "f.txt", "new content\n", "new");
    // Local uncommitted change on top.
    std::fs::write(dir.join("f.txt"), "dirty content\n").unwrap();

    // Restore the old version into the worktree (as the command does:
    // snapshot first, then restore).
    let recovery = RecoveryManager::new(std::env::temp_dir().join("ibexgit-restore-recovery"));
    let targets = vec![ibexgit_lib::core::recovery::DiscardTarget {
        path: "f.txt".into(),
        untracked: false,
        conflict: false,
    }];
    let snap = recovery
        .snapshot_discard(
            &*e,
            &dir,
            &targets,
            ibexgit_lib::core::recovery::DiscardScope::Worktree,
        )
        .await
        .unwrap();
    e.restore_from(&p, &old, &["f.txt".to_string()])
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("f.txt")).unwrap(),
        "old content\n"
    );
    // Index untouched (worktree-only restore): no staged diff.
    let status = e.status(&p).await.unwrap();
    let f = status.iter().find(|f| f.path == "f.txt").expect("tracked");
    assert!(f.unstaged, "worktree differs from index");

    // Undo via the snapshot.
    recovery.restore_discard(&*e, &dir, &snap.id).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("f.txt")).unwrap(),
        "dirty content\n",
        "snapshot restore brings back the dirty worktree"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// 搜索：hash 前缀跳转 + 消息 grep + 作者筛选。
#[tokio::test(flavor = "multi_thread")]
async fn graph_filter_hash_jump_and_grep() {
    let (dir, e, p) = simple_repo();
    write_commit(&dir, "a.txt", "1\n", "alpha commit");
    let target = write_commit(&dir, "b.txt", "2\n", "beta commit");
    write_commit(&dir, "c.txt", "3\n", "gamma commit");

    // Hash jump: graph starts AT the resolved commit and covers its
    // ancestors only (target + the base commit).
    let f = GraphFilter {
        text: Some(target[..8].to_string()),
        ..Default::default()
    };
    let rows = e.graph(&p, 0, 100, &f).await.unwrap();
    assert!(!rows.is_empty());
    assert_eq!(rows[0].hash, target);
    assert_eq!(rows.len(), 2);

    // Message grep.
    let f = GraphFilter {
        text: Some("gamma".into()),
        ..Default::default()
    };
    let rows = e.graph(&p, 0, 100, &f).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].message, "gamma commit");

    // Author filter (different author for one commit).
    git(&dir, &["config", "user.name", "Other"]);
    write_commit(&dir, "d.txt", "4\n", "delta commit");
    let f = GraphFilter {
        author: Some("Other".into()),
        ..Default::default()
    };
    let rows = e.graph(&p, 0, 100, &f).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].author, "Other");

    let _ = std::fs::remove_dir_all(&dir);
}

/// 空仓库（unborn HEAD）：graph 返回空而非报错。
#[tokio::test(flavor = "multi_thread")]
async fn graph_on_unborn_head_is_empty() {
    let dir = temp_repo("unborn");
    init_repo(&dir);
    git(&dir, &["config", "user.email", "graph@ibexgit.local"]);
    git(&dir, &["config", "user.name", "Graph"]);
    let e = engine();
    let p = dir.display().to_string();
    let rows = e.graph(&p, 0, 500, &GraphFilter::default()).await.unwrap();
    assert!(rows.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
