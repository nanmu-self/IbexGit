use crate::core::compat::GitCapabilities;
use crate::core::engine::patch::{self, LineOp};
use crate::core::engine::{
    CommitDetail, DiffOptions, DiffSource, FileContent, GraphFilter, GraphPage, LineSelection,
};
use crate::core::error::AppError;
use crate::core::recovery::{DiscardScope, DiscardTarget, RecoveryEntry, RecoveryManager};
use crate::core::repo::{RepoId, RepoManager, GRAPH_BATCH};
use crate::core::watcher::WatcherHub;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;

pub mod workspace;

/// Return the detected git capabilities as JSON.
#[tauri::command]
#[specta::specta]

pub fn git_version(caps: State<'_, GitCapabilities>) -> Result<String, String> {
    Ok(format!("{:?}", *caps))
}

/// Greet command (legacy from template).
#[tauri::command]
#[specta::specta]

pub fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

// =====================
// Repo lifecycle
// =====================

#[tauri::command]
#[specta::specta]

pub async fn repo_open(
    path: String,
    repos: State<'_, RepoManager>,
    hub: State<'_, WatcherHub>,
) -> Result<(RepoId, String), AppError> {
    let id = repos.open(PathBuf::from(&path)).await?;
    // RepoManager::open resolves to the worktree root (accepts any path
    // inside a worktree); watcher and frontend both use the resolved root.
    let root = repos
        .get_path(id)
        .await
        .ok_or_else(|| AppError::InvalidRepo { path: path.clone() })?;
    // Start external-change watching (state invalidation system, PLAN §4.3).
    if let Some(git_dir) = crate::core::watcher::resolve_git_dir(&root) {
        hub.add_repo(id, git_dir, root.clone()).await?;
    }
    Ok((id, root.display().to_string()))
}

#[tauri::command]
#[specta::specta]

pub async fn repo_close(
    id: RepoId,
    repos: State<'_, RepoManager>,
    hub: State<'_, WatcherHub>,
) -> Result<(), AppError> {
    hub.remove_repo(id).await;
    repos.close(id).await
}

#[tauri::command]
#[specta::specta]

pub async fn repo_list(repos: State<'_, RepoManager>) -> Result<Vec<(RepoId, String)>, AppError> {
    Ok(repos
        .list()
        .await
        .into_iter()
        .map(|(id, p)| (id, p.display().to_string()))
        .collect())
}

// =====================
// Working copy: status / stage / commit
// =====================

#[tauri::command]
#[specta::specta]

pub async fn git_status(
    id: RepoId,
    repos: State<'_, RepoManager>,
) -> Result<Vec<crate::core::engine::FileStatus>, AppError> {
    // Display-cache fast path; invalidated by the watcher on every change.
    repos.status(id).await
}

#[tauri::command]
#[specta::specta]

pub async fn git_stage(
    id: RepoId,
    paths: Vec<String>,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos.engine().stage(&path, &paths).await
}

#[tauri::command]
#[specta::specta]

pub async fn git_unstage(
    id: RepoId,
    paths: Vec<String>,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos.engine().unstage(&path, &paths).await
}

/// Discard changes for the given paths, with a recovery snapshot taken
/// beforehand (PLAN §4.7 轨道 A) so the operation is undoable.
///
/// Scope semantics per path (classification from a fresh status under the
/// write gate):
/// - untracked → physically deleted (snapshot holds a copy);
/// - unmerged (conflict) → full reset to HEAD (`restore --source=HEAD -S -W`);
/// - tracked, scope=worktree → `git restore` (staged state kept);
/// - tracked, scope=all → `git restore --source=HEAD -S -W` (staged dropped).
///
/// Returns the snapshot id for the undo entry, or `None` when nothing was
/// discardable. On failure the snapshot is rolled back.
#[tauri::command]
#[specta::specta]

pub async fn git_discard(
    id: RepoId,
    paths: Vec<String>,
    scope: String,
    repos: State<'_, RepoManager>,
    recovery: State<'_, RecoveryManager>,
) -> Result<Option<String>, AppError> {
    let scope = match scope.as_str() {
        "all" => DiscardScope::All,
        _ => DiscardScope::Worktree,
    };
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let root = resolve(&repos, id).await?;
    let engine = repos.engine();

    // Fresh classification (bypass the display cache): the snapshot must
    // reflect the state being destroyed, not a possibly-stale view.
    let status = engine.status(&root).await?;
    let mut targets: Vec<DiscardTarget> = Vec::new();
    for p in &paths {
        let Some(f) = status.iter().find(|f| &f.path == p) else {
            continue;
        };
        targets.push(DiscardTarget {
            path: p.clone(),
            untracked: f.untracked,
            conflict: f.conflict,
        });
        // A staged rename is a delete+add pair in the index: discarding it
        // must also restore the original path (restore_to_head on the new
        // name alone would leave the old name staged-deleted).
        if f.staged && !f.untracked {
            if let Some(orig) = &f.orig_path {
                if orig != &f.path && !targets.iter().any(|t| &t.path == orig) {
                    targets.push(DiscardTarget {
                        path: orig.clone(),
                        untracked: false,
                        conflict: false,
                    });
                }
            }
        }
    }
    if targets.is_empty() {
        return Ok(None);
    }

    // Snapshot → execute → on failure roll the snapshot back.
    let snap = recovery
        .snapshot_discard(&*engine, PathBuf::from(&root).as_path(), &targets, scope)
        .await?;
    let result = execute_discard(&engine, &root, &targets, scope).await;
    match result {
        Ok(()) => {
            // Lazy retention cleanup piggybacks on real activity.
            recovery.prune_expired(std::path::Path::new(&root));
            Ok(Some(snap.id))
        }
        Err(e) => {
            let _ = recovery.delete(std::path::Path::new(&root), &snap.id);
            Err(e)
        }
    }
}

async fn execute_discard(
    engine: &Arc<dyn crate::core::engine::GitEngine>,
    root: &str,
    targets: &[DiscardTarget],
    scope: DiscardScope,
) -> Result<(), AppError> {
    let mut tracked_wt: Vec<String> = Vec::new(); // scope=worktree restores
    let mut tracked_head: Vec<String> = Vec::new(); // scope=all / conflicts
    let mut untracked: Vec<String> = Vec::new();
    for t in targets {
        if t.untracked {
            untracked.push(t.path.clone());
        } else if t.conflict || scope == DiscardScope::All {
            // Conflicted paths have no meaningful "worktree-only" discard;
            // reset to HEAD regardless of scope (P8 owns conflict flows).
            tracked_head.push(t.path.clone());
        } else {
            tracked_wt.push(t.path.clone());
        }
    }
    engine.restore_worktree(root, &tracked_wt).await?;
    engine.restore_to_head(root, &tracked_head).await?;
    engine.delete_untracked(root, &untracked).await
}

#[tauri::command]
#[specta::specta]

pub async fn git_commit(
    id: RepoId,
    message: String,
    amend: bool,
    no_verify: bool,
    repos: State<'_, RepoManager>,
) -> Result<crate::core::engine::CommitResult, AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos
        .engine()
        .commit(&path, &message, amend, no_verify)
        .await
}

#[tauri::command]
#[specta::specta]

pub async fn git_log(
    id: RepoId,
    limit: u32,
    offset: u32,
    paths: Option<Vec<String>>,
    repos: State<'_, RepoManager>,
) -> Result<Vec<crate::core::engine::CommitInfo>, AppError> {
    let _permit = repos.read_permit().await?;
    let path = resolve(&repos, id).await?;
    let empty: Vec<String> = Vec::new();
    let paths = paths.as_deref().unwrap_or(&empty);
    repos.engine().log(&path, limit, offset, paths).await
}

// =====================
// Diff (P4: 管线分层 + 行级操作)
// =====================

/// Parse a diff for the given source/paths into a [`DiffModel`], cache it in
/// the RepoManager session and return it with its cache id.
///
/// Untracked worktree paths never appear in `git diff`; they are synthesized
/// as pure-addition files (P4) so line-level stage/discard works on them.
/// The model id is what line-level operations send back — Rust rebuilds the
/// patch from the same cached model (同源，杜绝双端解析漂移).
#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)]
pub async fn git_diff(
    id: RepoId,
    source: DiffSource,
    old_rev: Option<String>,
    new_rev: Option<String>,
    paths: Option<Vec<String>>,
    context_lines: Option<u32>,
    ignore_whitespace: Option<bool>,
    repos: State<'_, RepoManager>,
) -> Result<crate::core::engine::DiffModel, AppError> {
    let _permit = repos.read_permit().await?;
    let root = resolve(&repos, id).await?;
    let opts = DiffOptions {
        // Guardrail against absurd context requests.
        context_lines: context_lines.unwrap_or(3).clamp(0, 100_000),
        ignore_whitespace: ignore_whitespace.unwrap_or(false),
    };
    let rev_range = match (old_rev.as_deref(), new_rev.as_deref()) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    };
    let requested: Vec<String> = paths.unwrap_or_default();

    // Split worktree path requests into tracked (git diff) and untracked
    // (synthesized). Non-worktree sources have no untracked concept.
    let (tracked, untracked): (Vec<String>, Vec<String>) =
        if source == DiffSource::Worktree && !requested.is_empty() {
            let status = repos.status(id).await?;
            requested.into_iter().partition(|p| {
                !status
                    .iter()
                    .any(|f| &f.path == p && f.untracked && !f.conflict)
            })
        } else {
            (requested, Vec::new())
        };

    let mut model = if tracked.is_empty() && !untracked.is_empty() {
        // Only untracked paths: pure synthesis, no git diff call.
        crate::core::engine::DiffModel {
            id: 0,
            source,
            old_revision: None,
            new_revision: None,
            files: Vec::new(),
        }
    } else {
        repos
            .engine()
            .diff(&root, source, rev_range, &tracked, opts)
            .await?
    };
    for p in &untracked {
        match crate::core::engine::untracked::synthesize_untracked(std::path::Path::new(&root), p) {
            Ok(file) => model.files.push(file),
            // File vanished between status and read: skip it.
            Err(AppError::Io { .. }) => continue,
            Err(e) => return Err(e),
        }
    }

    repos.cache_diff(id, &mut model).await?;
    Ok(model)
}

/// Shared body of the three line-level operations (PLAN P4 行级暂存/丢弃/
/// 取消暂存): validate the cached model → build the patch → apply.
async fn apply_line_op(
    id: RepoId,
    model_id: u32,
    path: &str,
    selections: &[LineSelection],
    op: LineOp,
    repos: &State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let root = resolve(repos, id).await?;
    let model = repos.get_diff(id, model_id).await?;
    let file = model
        .find_file(path)
        .ok_or_else(|| AppError::parse(format!("diff model does not contain path {path:?}")))?;
    let text = patch::build_line_patch(file, path, selections, op)?;
    if text.is_empty() {
        return Ok(());
    }
    let (cached, reverse) = match op {
        LineOp::Stage => (true, false),
        LineOp::Discard => (false, true),
        LineOp::Unstage => (true, true),
    };
    repos.engine().apply(&root, &text, cached, reverse).await
}

/// Stage the selected lines of one file into the index
/// (`git apply --cached`; P4 行级暂存).
#[tauri::command]
#[specta::specta]

pub async fn git_stage_lines(
    id: RepoId,
    model_id: u32,
    path: String,
    selections: Vec<LineSelection>,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    apply_line_op(id, model_id, &path, &selections, LineOp::Stage, &repos).await
}

/// Discard the selected lines from the worktree (`git apply --reverse`),
/// with a whole-file recovery snapshot taken first (§4.7 轨道 A) so the
/// operation is undoable. Returns the snapshot id for the undo entry.
#[tauri::command]
#[specta::specta]

pub async fn git_discard_lines(
    id: RepoId,
    model_id: u32,
    path: String,
    selections: Vec<LineSelection>,
    repos: State<'_, RepoManager>,
    recovery: State<'_, RecoveryManager>,
) -> Result<Option<String>, AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let root = resolve(&repos, id).await?;
    let model = repos.get_diff(id, model_id).await?;
    let file = model
        .find_file(&path)
        .ok_or_else(|| AppError::parse(format!("diff model does not contain path {path:?}")))?;
    let text = patch::build_line_patch(file, &path, &selections, LineOp::Discard)?;
    if text.is_empty() {
        return Ok(None);
    }

    // Snapshot before destroying worktree content (untracked-ness from the
    // status view is enough for the snapshot's copy classification).
    let untracked = repos
        .status(id)
        .await
        .map(|st| {
            st.iter()
                .any(|f| f.path == path && f.untracked && !f.conflict)
        })
        .unwrap_or(false);
    let targets = [DiscardTarget {
        path: path.clone(),
        untracked,
        conflict: false,
    }];
    let snap = recovery
        .snapshot_discard(
            &*repos.engine(),
            std::path::Path::new(&root),
            &targets,
            DiscardScope::Worktree,
        )
        .await?;
    let result = repos.engine().apply(&root, &text, false, true).await;
    match result {
        Ok(()) => Ok(Some(snap.id)),
        Err(e) => {
            let _ = recovery.delete(std::path::Path::new(&root), &snap.id);
            Err(e)
        }
    }
}

/// Remove the selected lines from the index
/// (`git apply --cached --reverse`; P4 行级取消暂存).
#[tauri::command]
#[specta::specta]

pub async fn git_unstage_lines(
    id: RepoId,
    model_id: u32,
    path: String,
    selections: Vec<LineSelection>,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    apply_line_op(id, model_id, &path, &selections, LineOp::Unstage, &repos).await
}

/// Content of one file revision for the image diff: `rev = None` reads the
/// worktree file, otherwise `git cat-file blob <rev>:<path>` (e.g. `HEAD`,
/// `:0` for the index, or a commit-ish). Oversized files return `data: null`.
#[tauri::command]
#[specta::specta]

pub async fn git_file_content(
    id: RepoId,
    path: String,
    rev: Option<String>,
    repos: State<'_, RepoManager>,
) -> Result<FileContent, AppError> {
    let _permit = repos.read_permit().await?;
    let root = resolve(&repos, id).await?;
    repos
        .engine()
        .file_content(&root, &path, rev.as_deref())
        .await
}

// =====================
// Branches
// =====================

#[tauri::command]
#[specta::specta]

pub async fn git_branches(
    id: RepoId,
    repos: State<'_, RepoManager>,
) -> Result<Vec<crate::core::engine::BranchInfo>, AppError> {
    let _permit = repos.read_permit().await?;
    let path = resolve(&repos, id).await?;
    repos.engine().list_branches(&path).await
}

#[tauri::command]
#[specta::specta]

pub async fn git_checkout_branch(
    id: RepoId,
    name: String,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos.engine().checkout_branch(&path, &name).await
}

// =====================
// Recovery (PLAN §4.7 轨道 A)
// =====================

#[tauri::command]
#[specta::specta]

pub async fn recovery_list(
    id: RepoId,
    repos: State<'_, RepoManager>,
    recovery: State<'_, RecoveryManager>,
) -> Result<Vec<RecoveryEntry>, AppError> {
    let root = resolve(&repos, id).await?;
    Ok(recovery.list(std::path::Path::new(&root)))
}

#[tauri::command]
#[specta::specta]

pub async fn recovery_restore(
    id: RepoId,
    snapshot_id: String,
    repos: State<'_, RepoManager>,
    recovery: State<'_, RecoveryManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let root = resolve(&repos, id).await?;
    recovery
        .restore_discard(&*repos.engine(), std::path::Path::new(&root), &snapshot_id)
        .await
}

#[tauri::command]
#[specta::specta]

pub async fn recovery_delete(
    id: RepoId,
    snapshot_id: String,
    repos: State<'_, RepoManager>,
    recovery: State<'_, RecoveryManager>,
) -> Result<(), AppError> {
    let root = resolve(&repos, id).await?;
    recovery.delete(std::path::Path::new(&root), &snapshot_id)
}

// =====================
// Amend / ignore
// =====================

/// HEAD commit message (subject + body) for the Amend flow; `None` when
/// HEAD is unborn.
#[tauri::command]
#[specta::specta]

pub async fn git_head_message(
    id: RepoId,
    repos: State<'_, RepoManager>,
) -> Result<Option<String>, AppError> {
    let _permit = repos.read_permit().await?;
    let path = resolve(&repos, id).await?;
    repos.engine().head_message(&path).await
}

/// Append paths to `.gitignore` (right-click "ignore" action).
#[tauri::command]
#[specta::specta]

pub async fn git_ignore_paths(
    id: RepoId,
    paths: Vec<String>,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos.engine().ignore_paths(&path, &paths).await
}

// =====================
// History / commit graph (P5)
// =====================

/// First page of the commit graph (GraphQuery → GraphCache → GraphLayout;
/// the frontend renders SVG). Returns the loaded rows (≤ 500).
#[tauri::command]
#[specta::specta]
pub async fn git_graph(
    id: RepoId,
    filter: Option<GraphFilter>,
    repos: State<'_, RepoManager>,
) -> Result<GraphPage, AppError> {
    let _permit = repos.read_permit().await?;
    repos
        .graph_page(id, &filter.unwrap_or_default(), false, GRAPH_BATCH)
        .await
}

/// Next graph page (`start` = row index of the returned rows in the full
/// graph; `start = 0` means the cache was rebuilt → replace the list).
#[tauri::command]
#[specta::specta]
pub async fn git_graph_more(
    id: RepoId,
    filter: Option<GraphFilter>,
    repos: State<'_, RepoManager>,
) -> Result<GraphPage, AppError> {
    let _permit = repos.read_permit().await?;
    repos
        .graph_page(id, &filter.unwrap_or_default(), true, GRAPH_BATCH)
        .await
}

/// Commit metadata + changed files (详情面板)。Merge commits diff against
/// the first parent; root commits against the empty tree.
#[tauri::command]
#[specta::specta]
pub async fn git_commit_detail(
    id: RepoId,
    hash: String,
    repos: State<'_, RepoManager>,
) -> Result<CommitDetail, AppError> {
    let _permit = repos.read_permit().await?;
    let path = resolve(&repos, id).await?;
    repos.engine().commit_detail(&path, &hash).await
}

/// Cherry-pick the given commits onto HEAD, in list order (oldest first).
/// Conflicts surface as git errors — the visual conflict flow is P8.
#[tauri::command]
#[specta::specta]
pub async fn git_cherry_pick(
    id: RepoId,
    hashes: Vec<String>,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    if hashes.is_empty() {
        return Ok(());
    }
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos.engine().cherry_pick(&path, &hashes).await
}

/// Revert the given commits (`revert --no-edit`), in list order.
#[tauri::command]
#[specta::specta]
pub async fn git_revert(
    id: RepoId,
    hashes: Vec<String>,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    if hashes.is_empty() {
        return Ok(());
    }
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos.engine().revert(&path, &hashes).await
}

/// Squash the selected commits into one (`reset --soft <base>` + one
/// commit). v1 constraint (validated here): the selection must be exactly
/// the newest K commits of the first-parent chain from HEAD, none of them
/// a merge, and a parent must remain (the whole history cannot vanish).
/// Recovery-wise the move is visible in the reflog; the dedicated backup
/// ref flow arrives with ResetRecovery in P6.
#[tauri::command]
#[specta::specta]
pub async fn git_squash(
    id: RepoId,
    hashes: Vec<String>,
    message: String,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    if hashes.is_empty() {
        return Ok(());
    }
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    let engine = repos.engine();
    let base = validate_squash_selection(&engine, &path, &hashes).await?;
    engine.reset(&path, "soft", &base).await?;
    engine.commit(&path, &message, false, false).await?;
    Ok(())
}

/// Validate that `hashes` is exactly the newest K commits reachable from
/// HEAD via first-parent links, no merges among them; returns the base
/// (parent of the oldest selected commit) for `reset --soft`.
async fn validate_squash_selection(
    engine: &Arc<dyn crate::core::engine::GitEngine>,
    root: &str,
    hashes: &[String],
) -> Result<String, AppError> {
    let k = hashes.len();
    let log = engine.log(root, k as u32, 0, &[]).await?;
    if log.len() < k {
        return Err(AppError::parse(
            "squash: selection extends past the repository root",
        ));
    }
    let selected: HashSet<&String> = hashes.iter().collect();
    if selected.len() != k || log[..k].iter().any(|c| !selected.contains(&c.hash)) {
        return Err(AppError::parse(
            "squash: selection must be the newest consecutive commits",
        ));
    }
    for pair in log[..k].windows(2) {
        if pair[0].parents.len() != 1 || pair[0].parents[0] != pair[1].hash {
            return Err(AppError::parse(
                "squash: merge commits cannot be squashed (v1)",
            ));
        }
    }
    // The oldest selected commit must not be a merge either — resetting to
    // its first parent would fold the second parent's changes in.
    if log[k - 1].parents.len() != 1 {
        return Err(AppError::parse(
            "squash: merge commits cannot be squashed (v1)",
        ));
    }
    Ok(log[k - 1].parents[0].clone())
}

/// Restore file(s) from a revision into the worktree (P5 从历史恢复此文件
/// 版本)，after a track-A recovery snapshot so the overwrite is undoable.
/// Returns the snapshot id for the undo toast.
#[tauri::command]
#[specta::specta]
pub async fn git_restore_file_version(
    id: RepoId,
    rev: String,
    paths: Vec<String>,
    repos: State<'_, RepoManager>,
    recovery: State<'_, RecoveryManager>,
) -> Result<Option<String>, AppError> {
    if paths.is_empty() {
        return Ok(None);
    }
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let root = resolve(&repos, id).await?;
    let engine = repos.engine();

    // Snapshot the current worktree content of the paths first (轨道 A).
    let status = engine.status(&root).await?;
    let targets: Vec<DiscardTarget> = paths
        .iter()
        .map(|p| DiscardTarget {
            path: p.clone(),
            untracked: status
                .iter()
                .any(|f| &f.path == p && f.untracked && !f.conflict),
            conflict: false,
        })
        .collect();
    let snap = recovery
        .snapshot_discard(
            &*engine,
            std::path::Path::new(&root),
            &targets,
            DiscardScope::Worktree,
        )
        .await?;
    let result = engine.restore_from(&root, &rev, &paths).await;
    match result {
        Ok(()) => Ok(Some(snap.id)),
        Err(e) => {
            let _ = recovery.delete(std::path::Path::new(&root), &snap.id);
            Err(e)
        }
    }
}

/// Create a branch (used by the detached-HEAD guidance banner and later
/// the branch panel).
#[tauri::command]
#[specta::specta]
pub async fn git_create_branch(
    id: RepoId,
    name: String,
    start_point: Option<String>,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos
        .engine()
        .create_branch(&path, &name, start_point.as_deref())
        .await
}

async fn resolve(repos: &State<'_, RepoManager>, id: RepoId) -> Result<String, AppError> {
    repos
        .get_path(id)
        .await
        .map(|p| p.display().to_string())
        .ok_or_else(|| AppError::InvalidRepo {
            path: id.0.to_string(),
        })
}

// Keep Arc<RepoManager> constructible for tests without a Tauri app.
#[allow(dead_code)]
fn _assert_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Arc<RepoManager>>();
    assert_send_sync::<RepoId>();
}
