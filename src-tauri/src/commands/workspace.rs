//! Workspace commands: recent repositories + per-repo UI state (PLAN §4.4)
//! and repo groups/stars (PLAN P3.5).

use crate::core::error::AppError;
use crate::core::workspace::{
    self, GroupsFile, RecentRepo, RepoGroup, RepoMeta, RepoUiState, WorkspaceDir,
};
use tauri::State;

/// Emitted to the frontend when a second app instance was launched with
/// repository paths on its command line (single-instance plugin, P2).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type, tauri_specta::Event)]
pub struct AppOpenPaths {
    pub paths: Vec<String>,
}

#[tauri::command]
#[specta::specta]
pub async fn workspace_recents(dir: State<'_, WorkspaceDir>) -> Result<Vec<RecentRepo>, AppError> {
    workspace::load_recents(&dir.0)
}

#[tauri::command]
#[specta::specta]
pub async fn workspace_touch_recent(
    dir: State<'_, WorkspaceDir>,
    path: String,
    name: String,
) -> Result<(), AppError> {
    workspace::touch_recent(&dir.0, &path, &name)
}

#[tauri::command]
#[specta::specta]
pub async fn workspace_forget_recent(
    dir: State<'_, WorkspaceDir>,
    path: String,
) -> Result<(), AppError> {
    workspace::forget_recent(&dir.0, &path)
}

#[tauri::command]
#[specta::specta]
pub async fn workspace_load_state(
    dir: State<'_, WorkspaceDir>,
    repo_path: String,
) -> Result<Option<RepoUiState>, AppError> {
    workspace::load_state(&dir.0, &repo_path)
}

#[tauri::command]
#[specta::specta]
pub async fn workspace_save_state(
    dir: State<'_, WorkspaceDir>,
    repo_path: String,
    state: RepoUiState,
) -> Result<(), AppError> {
    workspace::save_state(&dir.0, &repo_path, &state)
}

// ===================== repo groups + stars (PLAN P3.5) =====================

/// Load the full groups file (groups + per-repo metadata).
#[tauri::command]
#[specta::specta]
pub async fn workspace_groups(dir: State<'_, WorkspaceDir>) -> Result<GroupsFile, AppError> {
    workspace::load_groups(&dir.0)
}

/// Create a group (`id: None`) or rename an existing one (`id: Some`).
#[tauri::command]
#[specta::specta]
pub async fn workspace_upsert_group(
    dir: State<'_, WorkspaceDir>,
    id: Option<String>,
    name: String,
) -> Result<RepoGroup, AppError> {
    workspace::upsert_group(&dir.0, id, name)
}

/// Delete a group; member repos fall back to ungrouped (stars are kept).
#[tauri::command]
#[specta::specta]
pub async fn workspace_delete_group(
    dir: State<'_, WorkspaceDir>,
    id: String,
) -> Result<(), AppError> {
    workspace::delete_group(&dir.0, &id)
}

/// Set the organizational metadata of one repo (group membership and/or
/// star; full replace).
#[tauri::command]
#[specta::specta]
pub async fn workspace_update_repo(
    dir: State<'_, WorkspaceDir>,
    path: String,
    meta: RepoMeta,
) -> Result<(), AppError> {
    workspace::update_repo(&dir.0, &path, meta)
}
