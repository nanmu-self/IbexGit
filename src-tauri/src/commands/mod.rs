use crate::core::compat::GitCapabilities;
use crate::core::engine::DiffSource;
use crate::core::error::AppError;
use crate::core::recovery::{DiscardScope, DiscardTarget, RecoveryEntry, RecoveryManager};
use crate::core::repo::{RepoId, RepoManager};
use crate::core::watcher::WatcherHub;
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
// Diff
// =====================

#[tauri::command]
#[specta::specta]

pub async fn git_diff(
    id: RepoId,
    source: String,
    old_rev: Option<String>,
    new_rev: Option<String>,
    paths: Option<Vec<String>>,
    repos: State<'_, RepoManager>,
) -> Result<crate::core::engine::DiffModel, AppError> {
    let _permit = repos.read_permit().await?;
    let path = resolve(&repos, id).await?;
    let diff_source = match source.as_str() {
        "staged" => DiffSource::Staged,
        "commit" => DiffSource::Commit,
        "stash" => DiffSource::Stash,
        _ => DiffSource::Worktree,
    };
    let rev_range = match (old_rev.as_deref(), new_rev.as_deref()) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    };
    let empty: Vec<String> = Vec::new();
    let paths = paths.as_deref().unwrap_or(&empty);
    repos
        .engine()
        .diff(&path, diff_source, rev_range, paths)
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
