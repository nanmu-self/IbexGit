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
use std::collections::BTreeMap;
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
    /// Workspace list mode: `list` (flat sections) or `tree` (path tree).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view_mode: Option<String>,
    /// Collapsed directory ids in tree mode.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tree_collapsed: Vec<String>,
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

// ===================== repo groups + stars (PLAN P3.5) =====================

/// A named repository group (flat, v1 — no nesting; PLAN P3.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RepoGroup {
    /// Stable id (generated at creation, survives renames).
    pub id: String,
    pub name: String,
    /// Display order (creation order); the UI sorts groups by this.
    pub order: u32,
}

/// Per-repo organizational metadata (PLAN P3.5). Entries exist only while
/// meaningful: [`update_repo`] drops an entry when it becomes the default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type, Default)]
pub struct RepoMeta {
    /// Group the repo belongs to (`None` = ungrouped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    /// Colorful bookmark (palette id, e.g. `red`/`blue`); `None` = unmarked.
    /// The palette itself is a frontend display concern.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bookmark: Option<String>,
}

impl RepoMeta {
    fn is_meaningful(&self) -> bool {
        self.group_id.is_some() || self.bookmark.is_some()
    }
}

/// `workspaces/groups.json` — the long-lived organizational layer over the
/// repo collection. Unlike `recents.json` (bounded recently-opened list),
/// group membership and stars must survive recents truncation, hence a
/// separate file (PLAN P3.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type, Default)]
pub struct GroupsFile {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub groups: Vec<RepoGroup>,
    /// Resolved repo path → metadata. Keys use the resolved worktree root
    /// (same normalization as `RepoId`) to avoid Windows case ambiguity.
    #[serde(default)]
    pub repos: BTreeMap<String, RepoMeta>,
}

fn groups_file(dir: &Path) -> Result<PathBuf, AppError> {
    Ok(ensure_dir(dir)?.join("groups.json"))
}

/// Load groups.json (empty file when it doesn't exist yet).
pub fn load_groups(dir: &Path) -> Result<GroupsFile, AppError> {
    Ok(read_json::<GroupsFile>(&groups_file(dir)?)?.unwrap_or_default())
}

/// Monotonic-enough id for UI keys: timestamp + process-local counter.
fn new_group_id() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("g_{ts:x}_{n:02x}")
}

/// Create a group (`id: None`) or rename an existing one (`id: Some`).
/// Returns the stored group; new groups get the next display order.
pub fn upsert_group(dir: &Path, id: Option<String>, name: String) -> Result<RepoGroup, AppError> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::internal("group name cannot be empty"));
    }
    let path = groups_file(dir)?;
    let mut file = read_json::<GroupsFile>(&path)?.unwrap_or_default();
    let next_order = file.groups.iter().map(|g| g.order).max().unwrap_or(0) + 1;
    let group = match id {
        Some(id) => {
            if let Some(existing) = file.groups.iter_mut().find(|g| g.id == id) {
                existing.name = name.clone();
                existing.clone()
            } else {
                // Unknown id: idempotent create with it.
                let g = RepoGroup {
                    id: id.clone(),
                    name: name.clone(),
                    order: next_order,
                };
                file.groups.push(g.clone());
                g
            }
        }
        None => {
            let g = RepoGroup {
                id: new_group_id(),
                name: name.clone(),
                order: next_order,
            };
            file.groups.push(g.clone());
            g
        }
    };
    write_json(&path, &file)?;
    Ok(group)
}

/// Delete a group; member repos fall back to ungrouped (bookmarks are kept);
/// metadata entries that become meaningless are dropped. No repo data lost.
pub fn delete_group(dir: &Path, id: &str) -> Result<(), AppError> {
    let path = groups_file(dir)?;
    let mut file = read_json::<GroupsFile>(&path)?.unwrap_or_default();
    file.groups.retain(|g| g.id != id);
    for meta in file.repos.values_mut() {
        if meta.group_id.as_deref() == Some(id) {
            meta.group_id = None;
        }
    }
    file.repos.retain(|_, meta| meta.is_meaningful());
    write_json(&path, &file)
}

/// Set the organizational metadata of one repo (full replace; PLAN P3.5
/// shares one setter for group membership and star).
pub fn update_repo(dir: &Path, repo_path: &str, meta: RepoMeta) -> Result<(), AppError> {
    let path = groups_file(dir)?;
    let mut file = read_json::<GroupsFile>(&path)?.unwrap_or_default();
    if meta.is_meaningful() {
        file.repos.insert(repo_path.to_string(), meta);
    } else {
        file.repos.remove(repo_path);
    }
    write_json(&path, &file)
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

    // ===================== groups + stars (PLAN P3.5) =====================

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ibexgit-groups-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn groups_file_missing_treated_as_empty() {
        let dir = temp_dir("missing");
        let file = load_groups(&dir).unwrap();
        assert!(file.groups.is_empty());
        assert!(file.repos.is_empty());
    }

    #[test]
    fn upsert_group_creates_with_increasing_order_and_trims_name() {
        let dir = temp_dir("upsert");
        let g1 = upsert_group(&dir, None, "  工作  ".into()).unwrap();
        assert_eq!(g1.name, "工作");
        assert_eq!(g1.order, 1);
        let g2 = upsert_group(&dir, None, "开源".into()).unwrap();
        assert_eq!(g2.order, 2);
        assert_ne!(g1.id, g2.id);
        assert_eq!(load_groups(&dir).unwrap().groups.len(), 2);
    }

    #[test]
    fn upsert_group_with_known_id_renames_and_keeps_order() {
        let dir = temp_dir("rename");
        let g = upsert_group(&dir, None, "工作".into()).unwrap();
        let renamed = upsert_group(&dir, Some(g.id.clone()), "工作项目".into()).unwrap();
        assert_eq!(renamed.id, g.id);
        assert_eq!(renamed.order, g.order);
        assert_eq!(renamed.name, "工作项目");
        let file = load_groups(&dir).unwrap();
        assert_eq!(file.groups.len(), 1);
    }

    #[test]
    fn upsert_group_rejects_empty_name() {
        let dir = temp_dir("empty-name");
        assert!(upsert_group(&dir, None, "   ".into()).is_err());
    }

    #[test]
    fn delete_group_ungroups_members_keeps_bookmarks_drops_meaningless() {
        let dir = temp_dir("delete");
        let g = upsert_group(&dir, None, "工作".into()).unwrap();
        update_repo(
            &dir,
            "C:/repos/a",
            RepoMeta {
                group_id: Some(g.id.clone()),
                bookmark: None,
            },
        )
        .unwrap();
        update_repo(
            &dir,
            "C:/repos/b",
            RepoMeta {
                group_id: Some(g.id.clone()),
                bookmark: Some("red".into()),
            },
        )
        .unwrap();
        delete_group(&dir, &g.id).unwrap();
        let file = load_groups(&dir).unwrap();
        assert!(file.groups.is_empty());
        // Became meaningless (ungrouped + unbookmarked) → entry dropped.
        assert!(!file.repos.contains_key("C:/repos/a"));
        // Bookmark kept, group cleared.
        assert_eq!(
            file.repos.get("C:/repos/b"),
            Some(&RepoMeta {
                group_id: None,
                bookmark: Some("red".into()),
            })
        );
    }

    #[test]
    fn update_repo_upserts_and_default_meta_removes_entry() {
        let dir = temp_dir("update-repo");
        update_repo(
            &dir,
            "C:/repos/a",
            RepoMeta {
                group_id: None,
                bookmark: Some("blue".into()),
            },
        )
        .unwrap();
        assert_eq!(
            load_groups(&dir).unwrap().repos.get("C:/repos/a"),
            Some(&RepoMeta {
                group_id: None,
                bookmark: Some("blue".into())
            })
        );
        // Full replace back to default → entry removed.
        update_repo(&dir, "C:/repos/a", RepoMeta::default()).unwrap();
        assert!(load_groups(&dir).unwrap().repos.is_empty());
    }

    #[test]
    fn update_repo_unknown_group_reference_is_stored_verbatim() {
        // The backend stores metadata verbatim; a dangling group id (e.g.
        // raced delete) simply renders as ungrouped in the UI.
        let dir = temp_dir("dangling");
        update_repo(
            &dir,
            "C:/repos/a",
            RepoMeta {
                group_id: Some("g_missing".into()),
                bookmark: None,
            },
        )
        .unwrap();
        assert_eq!(
            load_groups(&dir).unwrap().repos.get("C:/repos/a"),
            Some(&RepoMeta {
                group_id: Some("g_missing".into()),
                bookmark: None
            })
        );
    }
}
