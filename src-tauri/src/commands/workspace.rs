//! Workspace commands: recent repositories + per-repo UI state (PLAN §4.4).

use crate::core::error::AppError;
use crate::core::workspace::{self, RecentRepo, RepoUiState, WorkspaceDir};
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
