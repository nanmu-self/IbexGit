//! Workspace persistence (PLAN §4.4): recent repositories and per-repo UI
//! state live under `{appData}/workspaces/` — never inside user repositories.
//!
//! Files:
//! - `workspaces/recents.json` — bounded list of recently opened repositories
//! - `workspaces/{hash(path)}.json` — opaque per-repo UI state (JSON blob the
//!   frontend owns; the backend treats it as opaque)
//!
//! The hash reuses the RepoId FNV-1a of the worktree path so a repo always
//! maps to the same file.

use crate::core::error::AppError;
use crate::core::repo::RepoId;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::{Path, PathBuf};

/// Bounded recent list; beyond this the oldest entries fall off.
const MAX_RECENTS: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct RecentRepo {
    pub path: String,
    pub name: String,
    /// Unix timestamp (seconds) of the last open. f64 for JS-safe export.
    pub last_opened: f64,
}

/// Managed state: the `{appData}/workspaces` directory, resolved once at
/// startup. Commands receive it via `State` so they stay runtime-generic
/// (tauri-specta needs concrete runtimes for `AppHandle` parameters).
#[derive(Debug, Clone)]
pub struct WorkspaceDir(pub PathBuf);

fn ensure_dir(dir: &Path) -> Result<PathBuf, AppError> {
    std::fs::create_dir_all(dir)
        .map_err(|e| AppError::io_with_detail("create workspaces dir", e.to_string()))?;
    Ok(dir.to_path_buf())
}

fn state_file(dir: &Path, repo_path: &str) -> Result<PathBuf, AppError> {
    let hash = format!("{:016x}", RepoId::new(Path::new(repo_path)).0);
    Ok(ensure_dir(dir)?.join(format!("{hash}.json")))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>, AppError> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| AppError::io_with_detail("parse workspace json", e.to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(AppError::io_with_detail(
            "read workspace json",
            e.to_string(),
        )),
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), AppError> {
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| AppError::io_with_detail("serialize workspace json", e.to_string()))?;
    // Write-then-rename so a crash never leaves a truncated file behind.
    std::fs::write(&tmp, bytes)
        .map_err(|e| AppError::io_with_detail("write workspace json", e.to_string()))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| AppError::io_with_detail("commit workspace json", e.to_string()))?;
    Ok(())
}

/// Selected diff target in the workspace view (frontend-owned UI state).
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct SelectedFile {
    pub path: String,
    /// `worktree` or `staged` — which diff to show for the file.
    pub source: String,
}

/// Per-repo UI state (PLAN §4.4). The frontend owns the contents; this is a
/// typed contract so the wire format stays stable. Extend with new
/// `#[serde(default)]` fields only — never remove/rename existing ones.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct RepoUiState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub filter: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sidebar_collapsed: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_file: Option<SelectedFile>,
}

/// Recent repositories, newest first.
pub fn load_recents(dir: &Path) -> Result<Vec<RecentRepo>, AppError> {
    let path = ensure_dir(dir)?.join("recents.json");
    Ok(read_json::<Vec<RecentRepo>>(&path)?.unwrap_or_default())
}

/// Upsert one repo in the recent list (dedupe by path, bump timestamp,
/// truncate to [`MAX_RECENTS`]).
pub fn touch_recent(dir: &Path, path: &str, name: &str) -> Result<(), AppError> {
    let list_path = ensure_dir(dir)?.join("recents.json");
    let mut list = read_json::<Vec<RecentRepo>>(&list_path)?.unwrap_or_default();
    list.retain(|r| r.path != path);
    list.insert(
        0,
        RecentRepo {
            path: path.to_string(),
            name: name.to_string(),
            last_opened: chrono::Utc::now().timestamp() as f64,
        },
    );
    list.truncate(MAX_RECENTS);
    write_json(&list_path, &list)
}

/// Remove one repo from the recent list (no-op when absent).
pub fn forget_recent(dir: &Path, path: &str) -> Result<(), AppError> {
    let list_path = ensure_dir(dir)?.join("recents.json");
    let Some(mut list) = read_json::<Vec<RecentRepo>>(&list_path)? else {
        return Ok(());
    };
    list.retain(|r| r.path != path);
    write_json(&list_path, &list)
}

/// Load the per-repo UI state (None when the repo was never opened).
pub fn load_state(dir: &Path, repo_path: &str) -> Result<Option<RepoUiState>, AppError> {
    read_json(&state_file(dir, repo_path)?)
}

/// Persist the per-repo UI state.
pub fn save_state(dir: &Path, repo_path: &str, state: &RepoUiState) -> Result<(), AppError> {
    write_json(&state_file(dir, repo_path)?, state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_file_uses_fnv1a_of_path() {
        // Same path → same file name; different paths → different files.
        let p1 = state_file_name("C:/repos/demo");
        let p2 = state_file_name("C:/repos/demo");
        let p3 = state_file_name("C:/repos/other");
        assert_eq!(p1, p2);
        assert_ne!(p1, p3);
        assert!(p1.ends_with(".json"));
    }

    fn state_file_name(path: &str) -> String {
        format!("{:016x}.json", RepoId::new(Path::new(path)).0)
    }
}
