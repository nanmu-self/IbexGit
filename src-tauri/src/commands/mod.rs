use crate::core::compat::GitCapabilities;
use crate::core::engine::conflict::{ConflictModel, ConflictSummary};
use crate::core::engine::patch::{self, LineOp};
use crate::core::engine::{
    BackupRef, CommitDetail, CommitInfo, DiffOptions, DiffSource, FileContent, GraphFilter,
    GraphPage, LineSelection, OperationState, ResetUndo,
};
use crate::core::error::AppError;
use crate::core::recovery::{DiscardScope, DiscardTarget, RecoveryEntry, RecoveryManager};
use crate::core::repo::{RepoId, RepoManager, GRAPH_BATCH};
use crate::core::watcher::WatcherHub;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;

pub mod ai;
pub mod app;
pub mod net;
pub mod ssh;
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

// =====================
// P6: branches / refs panel
// =====================

#[tauri::command]
#[specta::specta]
pub async fn git_delete_branch(
    id: RepoId,
    name: String,
    force: bool,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos.engine().delete_branch(&path, &name, force).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_rename_branch(
    id: RepoId,
    old_name: String,
    new_name: String,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos
        .engine()
        .rename_branch(&path, &old_name, &new_name)
        .await
}

/// Set (or clear with `None`) the upstream tracking of a branch.
#[tauri::command]
#[specta::specta]
pub async fn git_set_upstream(
    id: RepoId,
    branch: String,
    upstream: Option<String>,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos
        .engine()
        .set_branch_upstream(&path, &branch, upstream.as_deref())
        .await
}

// =====================
// P6: tags
// =====================

#[tauri::command]
#[specta::specta]
pub async fn git_tags(
    id: RepoId,
    repos: State<'_, RepoManager>,
) -> Result<Vec<crate::core::engine::TagInfo>, AppError> {
    let _permit = repos.read_permit().await?;
    let path = resolve(&repos, id).await?;
    repos.engine().list_tags(&path).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_create_tag(
    id: RepoId,
    name: String,
    message: Option<String>,
    target: String,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos
        .engine()
        .create_tag(&path, &name, message.as_deref(), &target)
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn git_delete_tag(
    id: RepoId,
    name: String,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos.engine().delete_tag(&path, &name).await
}

// =====================
// P6: stash
// =====================

#[tauri::command]
#[specta::specta]
pub async fn git_stash_list(
    id: RepoId,
    repos: State<'_, RepoManager>,
) -> Result<Vec<crate::core::engine::StashEntry>, AppError> {
    let _permit = repos.read_permit().await?;
    let path = resolve(&repos, id).await?;
    repos.engine().list_stash(&path).await
}

/// Stash all changes (including untracked, matching the discard snapshot
/// philosophy) and return the new entry's index.
#[tauri::command]
#[specta::specta]
pub async fn git_stash_push(
    id: RepoId,
    message: Option<String>,
    repos: State<'_, RepoManager>,
) -> Result<u32, AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos
        .engine()
        .stash_push(&path, message.as_deref())
        .await
        .map(|i| i as u32)
}

#[tauri::command]
#[specta::specta]
pub async fn git_stash_apply(
    id: RepoId,
    index: u32,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos.engine().stash_apply(&path, index as usize).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_stash_pop(
    id: RepoId,
    index: u32,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos.engine().stash_pop(&path, index as usize).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_stash_drop(
    id: RepoId,
    index: u32,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos.engine().stash_drop(&path, index as usize).await
}

// =====================
// P6: remotes
// =====================

#[tauri::command]
#[specta::specta]
pub async fn git_remotes(
    id: RepoId,
    repos: State<'_, RepoManager>,
) -> Result<Vec<crate::core::engine::RemoteInfo>, AppError> {
    let _permit = repos.read_permit().await?;
    let path = resolve(&repos, id).await?;
    repos.engine().list_remotes(&path).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_add_remote(
    id: RepoId,
    name: String,
    url: String,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos.engine().add_remote(&path, &name, &url).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_remove_remote(
    id: RepoId,
    name: String,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos.engine().remove_remote(&path, &name).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_set_remote_url(
    id: RepoId,
    name: String,
    url: String,
    push: bool,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos
        .engine()
        .set_remote_url(&path, &name, &url, push)
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn git_prune_remote(
    id: RepoId,
    name: String,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos.engine().prune_remote(&path, &name).await
}

// =====================
// P6: fetch / pull / push / merge / rebase
// =====================

#[tauri::command]
#[specta::specta]
pub async fn git_fetch(
    id: RepoId,
    remote: Option<String>,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos.engine().fetch(&path, remote.as_deref()).await
}

/// Pull with an explicit strategy: merge (default) / rebase / ff_only.
#[tauri::command]
#[specta::specta]
pub async fn git_pull(
    id: RepoId,
    remote: Option<String>,
    branch: Option<String>,
    mode: Option<String>,
    repos: State<'_, RepoManager>,
) -> Result<crate::core::engine::PullResult, AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos
        .engine()
        .pull(&path, remote.as_deref(), branch.as_deref(), mode.as_deref())
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn git_push(
    id: RepoId,
    remote: String,
    branch: String,
    force_with_lease: bool,
    set_upstream: bool,
    tags: bool,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos
        .engine()
        .push(
            &path,
            &remote,
            &branch,
            force_with_lease,
            set_upstream,
            tags,
        )
        .await
}

/// Merge `target` into the current branch. Conflicts surface as git errors
/// (toast) — the visual conflict flow is P8.
#[tauri::command]
#[specta::specta]
pub async fn git_merge(
    id: RepoId,
    target: String,
    ff_only: bool,
    repos: State<'_, RepoManager>,
) -> Result<crate::core::engine::MergeResult, AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos.engine().merge(&path, &target, ff_only).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_rebase(
    id: RepoId,
    target: String,
    repos: State<'_, RepoManager>,
) -> Result<crate::core::engine::RebaseState, AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos.engine().rebase(&path, &target, &[]).await
}

// =====================
// P6: reset (soft/mixed/hard) with recovery + undo
// =====================

fn valid_reset_mode(mode: &str) -> bool {
    matches!(mode, "soft" | "mixed" | "hard")
}

/// Reset the current branch to `target` with full recovery support:
///
/// - every mode first creates a track-B backup ref at HEAD (refs/ibexgit/
///   backups/reset-<ts>) so the pre-reset commit stays reachable;
/// - `mixed`/`hard` additionally take a track-A snapshot of every path with
///   staged/unstaged/conflicted changes (mixed rewrites the index; hard also
///   rewrites the worktree). Untracked files are untouched by reset.
///
/// Returns the undo anchors: the backup ref plus (for mixed/hard) the
/// snapshot id. Undo = `reset --<mode> <backup_ref>` + snapshot restore.
#[tauri::command]
#[specta::specta]
pub async fn git_reset(
    id: RepoId,
    mode: String,
    target: String,
    repos: State<'_, RepoManager>,
    recovery: State<'_, RecoveryManager>,
) -> Result<ResetUndo, AppError> {
    if !valid_reset_mode(&mode) {
        return Err(AppError::parse(format!(
            "reset: invalid mode {mode:?} (soft|mixed|hard)"
        )));
    }
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let root = resolve(&repos, id).await?;
    let engine = repos.engine();

    // Track B: backup ref at the pre-reset HEAD (fails naturally on an
    // unborn HEAD, which reset cannot serve anyway).
    let backup_ref = recovery
        .create_backup(&*engine, std::path::Path::new(&root), "reset", "HEAD")
        .await?;

    // Track A: snapshot every path mixed/hard will destroy (index and/or
    // worktree content). Untracked paths are not reset targets.
    let mut snapshot_id = None;
    if mode != "soft" {
        let status = engine.status(&root).await?;
        let targets: Vec<DiscardTarget> = status
            .iter()
            .filter(|f| (f.staged || f.unstaged || f.conflict) && !f.untracked)
            .map(|f| DiscardTarget {
                path: f.path.clone(),
                untracked: false,
                conflict: f.conflict,
            })
            .collect();
        if !targets.is_empty() {
            let snap = recovery
                .snapshot_discard(
                    &*engine,
                    std::path::Path::new(&root),
                    &targets,
                    DiscardScope::All,
                )
                .await?;
            snapshot_id = Some(snap.id);
        }
    }

    match engine.reset(&root, &mode, &target).await {
        Ok(()) => Ok(ResetUndo {
            backup_ref,
            snapshot_id,
        }),
        Err(e) => {
            // Roll back the anchors so a failed reset leaves no litter.
            if let Some(snap) = &snapshot_id {
                let _ = recovery.delete(std::path::Path::new(&root), snap);
            }
            let _ = recovery
                .delete_backups(&*engine, std::path::Path::new(&root), &[backup_ref])
                .await;
            Err(e)
        }
    }
}

/// Undo one reset (PLAN P6 一键撤销): move the branch back to the backup
/// ref with the same mode, then (mixed/hard) restore the track-A snapshot
/// to rebuild the exact pre-reset index + worktree.
#[tauri::command]
#[specta::specta]
pub async fn git_undo_reset(
    id: RepoId,
    backup_ref: String,
    mode: String,
    snapshot_id: Option<String>,
    repos: State<'_, RepoManager>,
    recovery: State<'_, RecoveryManager>,
) -> Result<(), AppError> {
    if !valid_reset_mode(&mode) {
        return Err(AppError::parse(format!(
            "undo reset: invalid mode {mode:?}"
        )));
    }
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let root = resolve(&repos, id).await?;
    let engine = repos.engine();
    engine.reset(&root, &mode, &backup_ref).await?;
    if let Some(snap) = snapshot_id {
        recovery
            .restore_discard(&*engine, std::path::Path::new(&root), &snap)
            .await?;
    }
    Ok(())
}

// =====================
// P6: clean (preview → confirm → delete)
// =====================

#[tauri::command]
#[specta::specta]
pub async fn git_clean_list(
    id: RepoId,
    repos: State<'_, RepoManager>,
) -> Result<Vec<String>, AppError> {
    let _permit = repos.read_permit().await?;
    let path = resolve(&repos, id).await?;
    repos.engine().clean_list(&path).await
}

/// Delete the confirmed untracked paths. A track-A snapshot is taken first
/// so the removal stays undoable; returns the snapshot id for the toast.
#[tauri::command]
#[specta::specta]
pub async fn git_clean(
    id: RepoId,
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

    // Snapshot the doomed untracked paths (physical copies) first.
    let targets: Vec<DiscardTarget> = paths
        .iter()
        .map(|p| DiscardTarget {
            path: p.trim_end_matches('/').to_string(),
            untracked: true,
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
    match engine.clean(&root, &paths).await {
        Ok(()) => Ok(Some(snap.id)),
        Err(e) => {
            let _ = recovery.delete(std::path::Path::new(&root), &snap.id);
            Err(e)
        }
    }
}

// =====================
// P6: reflog browser
// =====================

#[tauri::command]
#[specta::specta]
pub async fn git_reflog(
    id: RepoId,
    ref_name: Option<String>,
    repos: State<'_, RepoManager>,
) -> Result<Vec<crate::core::engine::ReflogEntry>, AppError> {
    let _permit = repos.read_permit().await?;
    let path = resolve(&repos, id).await?;
    repos.engine().reflog(&path, ref_name.as_deref()).await
}

// =====================
// P9: file trace (单文件历史 + Blame)
// =====================

/// Single-file history with rename following (`git log --follow`),
/// newest first. `start` = last hash of the previous page (cursor
/// pagination; see the engine doc). Read-only.
#[tauri::command]
#[specta::specta]
pub async fn git_file_history(
    id: RepoId,
    path: String,
    limit: u32,
    start: Option<String>,
    repos: State<'_, RepoManager>,
) -> Result<Vec<crate::core::engine::FileCommit>, AppError> {
    let _permit = repos.read_permit().await?;
    let path_root = resolve(&repos, id).await?;
    repos
        .engine()
        .file_history(&path_root, &path, limit, start.as_deref())
        .await
}

/// Blame the current worktree version of a path (`git blame --porcelain`).
#[tauri::command]
#[specta::specta]
pub async fn git_blame(
    id: RepoId,
    path: String,
    repos: State<'_, RepoManager>,
) -> Result<crate::core::engine::BlameResult, AppError> {
    let _permit = repos.read_permit().await?;
    let path_root = resolve(&repos, id).await?;
    repos.engine().blame(&path_root, &path).await
}

// =====================
// P6: branch compare + merge/rebase previews
// =====================

/// Ahead/behind + merge base of two revs (branch compare view).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct BranchCompare {
    /// Commits in `left` not in `right`.
    pub ahead: u32,
    /// Commits in `right` not in `left`.
    pub behind: u32,
    pub merge_base: Option<String>,
}

#[tauri::command]
#[specta::specta]
pub async fn git_branch_compare(
    id: RepoId,
    left: String,
    right: String,
    repos: State<'_, RepoManager>,
) -> Result<BranchCompare, AppError> {
    let _permit = repos.read_permit().await?;
    let path = resolve(&repos, id).await?;
    let engine = repos.engine();
    let (ahead, behind) = engine.range_count(&path, &left, &right).await?;
    let merge_base = engine.merge_base(&path, &left, &right).await?;
    Ok(BranchCompare {
        ahead,
        behind,
        merge_base,
    })
}

/// Commits in a rev range — merge/rebase previews and the compare view's
/// commit list.
#[tauri::command]
#[specta::specta]
pub async fn git_rev_list(
    id: RepoId,
    range: String,
    limit: u32,
    offset: u32,
    repos: State<'_, RepoManager>,
) -> Result<Vec<CommitInfo>, AppError> {
    let _permit = repos.read_permit().await?;
    let path = resolve(&repos, id).await?;
    repos.engine().rev_list(&path, &range, limit, offset).await
}

// =====================
// P6: backup refs (孤儿备份清理)
// =====================

#[tauri::command]
#[specta::specta]
pub async fn git_backup_list(
    id: RepoId,
    repos: State<'_, RepoManager>,
    recovery: State<'_, RecoveryManager>,
) -> Result<Vec<BackupRef>, AppError> {
    let root = resolve(&repos, id).await?;
    recovery
        .list_backups(&*repos.engine(), std::path::Path::new(&root))
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn git_backup_delete(
    id: RepoId,
    names: Vec<String>,
    repos: State<'_, RepoManager>,
    recovery: State<'_, RecoveryManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let root = resolve(&repos, id).await?;
    recovery
        .delete_backups(&*repos.engine(), std::path::Path::new(&root), &names)
        .await
}

// =====================
// P8: conflicts & operation state
// =====================

/// All conflicted paths with lightweight classification (conflict file list).
#[tauri::command]
#[specta::specta]
pub async fn git_conflict_list(
    id: RepoId,
    repos: State<'_, RepoManager>,
) -> Result<Vec<ConflictSummary>, AppError> {
    let _permit = repos.read_permit().await?;
    let path = resolve(&repos, id).await?;
    repos.engine().conflict_list(&path).await
}

/// Full model for one conflicted path (editor input).
#[tauri::command]
#[specta::specta]
pub async fn git_conflict_model(
    id: RepoId,
    path: String,
    repos: State<'_, RepoManager>,
) -> Result<ConflictModel, AppError> {
    let _permit = repos.read_permit().await?;
    let root = resolve(&repos, id).await?;
    repos.engine().conflict_model(&root, &path).await
}

/// Write the resolved document back and stage the path. Rejects text that
/// still contains conflict markers (Rust is the single marker authority).
#[tauri::command]
#[specta::specta]
pub async fn git_resolve_conflict_text(
    id: RepoId,
    path: String,
    text: String,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    if text.len() > 32 * 1024 * 1024 {
        return Err(AppError::parse("resolved document too large to write back"));
    }
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let root = resolve(&repos, id).await?;
    repos
        .engine()
        .resolve_conflict_text(&root, &path, &text)
        .await
}

/// Resolve one conflict by side: `ours` / `theirs` (stage blob back into
/// the worktree + `git add`) or `delete` (`git rm -f`).
#[tauri::command]
#[specta::specta]
pub async fn git_resolve_conflict_side(
    id: RepoId,
    path: String,
    action: String,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let root = resolve(&repos, id).await?;
    match action.as_str() {
        "ours" | "theirs" => {
            repos
                .engine()
                .resolve_conflict_keep(&root, &path, &action)
                .await
        }
        "delete" => repos.engine().resolve_conflict_delete(&root, &path).await,
        other => Err(AppError::parse(format!(
            "unknown conflict resolution {other:?}"
        ))),
    }
}

/// Detect the in-progress operation (merge/rebase/cherry-pick/…), if any.
/// The frontend polls it alongside status for the guidance banner.
#[tauri::command]
#[specta::specta]
pub async fn git_operation_state(
    id: RepoId,
    repos: State<'_, RepoManager>,
) -> Result<Option<OperationState>, AppError> {
    let path = resolve(&repos, id).await?;
    repos.engine().operation_state(&path).await
}

/// Abort the in-progress operation (merge --abort / rebase --abort / …).
#[tauri::command]
#[specta::specta]
pub async fn git_operation_abort(
    id: RepoId,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos.engine().operation_abort(&path).await
}

/// Continue the in-progress operation after resolutions are staged.
#[tauri::command]
#[specta::specta]
pub async fn git_operation_continue(
    id: RepoId,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos.engine().operation_continue(&path).await
}

/// Skip the current commit of a rebase/cherry-pick/revert sequence.
#[tauri::command]
#[specta::specta]
pub async fn git_operation_skip(id: RepoId, repos: State<'_, RepoManager>) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos.engine().operation_skip(&path).await
}

/// Open an external merge tool for one path. `tool` = git's built-in name
/// (meld/kdiff3/…); `cmd` = custom command template (uses $MERGED etc.) —
/// both configured via `-c` only, nothing persisted. The tool's exit code
/// is trusted and the run may take minutes (GUI wait).
#[tauri::command]
#[specta::specta]
pub async fn git_mergetool(
    id: RepoId,
    path: String,
    tool: Option<String>,
    cmd: Option<String>,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let root = resolve(&repos, id).await?;
    repos
        .engine()
        .mergetool(&root, &path, tool.as_deref(), cmd.as_deref())
        .await
}

// =====================
// P10: Git 配置查看器 / 提交模板（只读）
// =====================

/// List the user-level git config (P10 设置中心 → Git 配置）。
#[tauri::command]
#[specta::specta]
pub async fn git_config_global(
    repos: State<'_, RepoManager>,
) -> Result<Vec<crate::core::engine::ConfigEntry>, AppError> {
    let _permit = repos.read_permit().await?;
    repos.engine().config_global().await
}

/// List the repository-level git config (P10 设置中心 → Git 配置）。
#[tauri::command]
#[specta::specta]
pub async fn git_config_local(
    id: RepoId,
    repos: State<'_, RepoManager>,
) -> Result<Vec<crate::core::engine::ConfigEntry>, AppError> {
    let _permit = repos.read_permit().await?;
    let root = resolve(&repos, id).await?;
    repos.engine().config_local(&root).await
}

/// Set/unset a user-level git config key (P10 设置中心 → 常用配置编辑）。
/// 全局作用域写 `~/.gitconfig`，不涉及仓库 index.lock；read_permit 仅作
/// git 子进程全局并发上限使用。
#[tauri::command]
#[specta::specta]
pub async fn git_config_set_global(
    key: String,
    value: Option<String>,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let _permit = repos.read_permit().await?;
    repos
        .engine()
        .config_set_global(&key, value.as_deref())
        .await
}

// =====================
// P10: 仓库级常用配置（仓库设置对话框，写入 .git/config）
// =====================

/// The common repo-level config keys surfaced by the per-repo settings
/// dialog（身份 / 代理 / 拉取推送 / 换行）。Frontend renders them in this
/// order; `git_repo_config_values` resolves exactly these keys.
const REPO_CONFIG_KEYS: &[&str] = &[
    "user.name",
    "user.email",
    "http.proxy",
    "https.proxy",
    "core.autocrlf",
    "pull.rebase",
    "fetch.prune",
    "push.autoSetupRemote",
];

/// Resolve the common repo config keys: repo-local value + effective
/// (merged) value for each（P10 仓库设置）。两次 `git config --list -z`
/// 读完后在命令层拼装 DTO，engine 保持通用。
#[tauri::command]
#[specta::specta]
pub async fn git_repo_config_values(
    id: RepoId,
    repos: State<'_, RepoManager>,
) -> Result<Vec<crate::core::engine::RepoConfigValue>, AppError> {
    let _permit = repos.read_permit().await?;
    let root = resolve(&repos, id).await?;
    // 同名键以最后一次出现为准（parse::config_last_wins，git --get 语义）。
    let local = crate::core::engine::parse::config_last_wins(
        repos
            .engine()
            .config_local(&root)
            .await?
            .into_iter()
            .map(|e| (e.key, e.value))
            .collect(),
    );
    let merged = crate::core::engine::parse::config_last_wins(
        repos
            .engine()
            .config_merged(&root)
            .await?
            .into_iter()
            .map(|e| (e.key, e.value))
            .collect(),
    );
    Ok(REPO_CONFIG_KEYS
        .iter()
        .map(|&key| crate::core::engine::RepoConfigValue {
            key: key.to_string(),
            local: local.get(key).cloned(),
            effective: merged.get(key).cloned(),
        })
        .collect())
}

/// Set/unset a repository-local config key (`value === None` → unset；
/// P10 仓库设置）。写 `.git/config`（config.lock）而非 index.lock，但
/// 变更类操作按红线统一持有 per-repo WriteGate。
#[tauri::command]
#[specta::specta]
pub async fn git_repo_config_set(
    id: RepoId,
    key: String,
    value: Option<String>,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let root = resolve(&repos, id).await?;
    repos
        .engine()
        .config_set_local(&root, &key, value.as_deref())
        .await
}

/// Read the global gitignore (`core.excludesFile` or default path).
#[tauri::command]
#[specta::specta]
pub async fn git_gitignore_global(
    repos: State<'_, RepoManager>,
) -> Result<Option<crate::core::engine::GitignoreFile>, AppError> {
    let _permit = repos.read_permit().await?;
    repos.engine().global_gitignore().await
}

/// Read the commit message template (`commit.template`), if configured.
#[tauri::command]
#[specta::specta]
pub async fn git_commit_template(
    id: RepoId,
    repos: State<'_, RepoManager>,
) -> Result<Option<crate::core::engine::CommitTemplate>, AppError> {
    let _permit = repos.read_permit().await?;
    let root = resolve(&repos, id).await?;
    repos.engine().commit_template(&root).await
}

// Keep Arc<RepoManager> constructible for tests without a Tauri app.
#[allow(dead_code)]
fn _assert_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Arc<RepoManager>>();
    assert_send_sync::<RepoId>();
}
