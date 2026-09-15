use crate::core::compat::GitCapabilities;
use crate::core::engine::DiffSource;
use crate::core::error::AppError;
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

#[tauri::command]
#[specta::specta]

pub async fn git_discard(
    id: RepoId,
    paths: Vec<String>,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let gate = repos.write_gate(id).await?;
    let _guard = gate.lock().await;
    let path = resolve(&repos, id).await?;
    repos.engine().discard(&path, &paths).await
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
