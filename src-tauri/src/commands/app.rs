//! Application-level commands (P10 设置中心): log level control, git path
//! connectivity test. These probe the environment rather than a repository,
//! so they live outside the GitEngine (same layer as `compat`).

use crate::core::error::AppError;
use tauri::State;

pub type Registry = tracing_subscriber::Registry;
pub type FilterHandle = tracing_subscriber::reload::Handle<tracing_subscriber::EnvFilter, Registry>;

/// Reload handle produced by `lib.rs::init_tracing`; managed in Tauri state
/// so commands can adjust logging at runtime (P10 高级：日志级别).
pub struct LogFilter(pub FilterHandle);

/// Adjust the tracing filter at runtime (P10 高级：日志级别）。
#[tauri::command]
#[specta::specta]
pub fn app_set_log_level(level: String, log: State<'_, LogFilter>) -> Result<(), AppError> {
    let filter = tracing_subscriber::EnvFilter::try_new(&level)
        .map_err(|e| AppError::parse(format!("invalid log filter {level:?}: {e}")))?;
    log.0
        .modify(|l| *l = filter)
        .map_err(|e| AppError::internal(format!("failed to reload log filter: {e}")))?;
    tracing::info!(%level, "log level changed");
    Ok(())
}

/// Run `<path> --version` to validate a user-configured git executable
/// (P10 设置中心：git 路径自定义). Returns the trimmed version string.
#[tauri::command]
#[specta::specta]
pub fn app_check_git_path(path: String) -> Result<String, AppError> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(AppError::parse("git path is empty"));
    }
    crate::core::compat::GitCapabilities::version_of(trimmed)
}
