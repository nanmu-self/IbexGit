//! Application-level commands (P10 设置中心): log level control, git path
//! connectivity test. These probe the environment rather than a repository,
//! so they live outside the GitEngine (same layer as `compat`).

use crate::core::error::AppError;
use serde::{Deserialize, Serialize};
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

/// 前端（webview）日志级别。serde 小写，前端拿到的是字面量联合类型。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum FrontendLogLevel {
    Error,
    Warn,
    Info,
}

/// 记录一条前端日志（window.onerror / unhandledrejection / console 转发，
/// 见 `src/lib/logging.ts`），落进与 Rust 侧相同的 tracing 管线，
/// target 固定为 `frontend` 以便区分来源。前端调用方已做限流与
/// 截断，这里再做一次 char 边界截断兜底。
#[tauri::command]
#[specta::specta]
pub fn app_log(level: FrontendLogLevel, message: String) -> Result<(), AppError> {
    const MAX_CHARS: usize = 4000;
    let message = if message.chars().count() <= MAX_CHARS {
        message
    } else {
        let mut clipped: String = message.chars().take(MAX_CHARS).collect();
        clipped.push('…');
        clipped
    };
    match level {
        FrontendLogLevel::Error => tracing::error!(target: "frontend", "{message}"),
        FrontendLogLevel::Warn => tracing::warn!(target: "frontend", "{message}"),
        FrontendLogLevel::Info => tracing::info!(target: "frontend", "{message}"),
    }
    Ok(())
}

/// Open the platform terminal at `path` (P3 工具栏：在终端打开). OS
/// integration rather than a git operation, so it lives at the app layer;
/// the actual candidate logic + tests are in `core::terminal`.
#[tauri::command]
#[specta::specta]
pub async fn app_open_terminal(path: String) -> Result<(), AppError> {
    tokio::task::spawn_blocking(move || crate::core::terminal::launch(std::path::Path::new(&path)))
        .await
        .map_err(|e| AppError::internal(format!("terminal task join failed: {e}")))?
        .map_err(AppError::from)
}

/// Open the system file manager AT `path`, entering the folder (P3 工具栏).
/// Own implementation instead of the opener plugin: on Windows
/// `plugin-opener::open_path` maps a directory to
/// `SHOpenFolderAndSelectItems` — reveal/select in the PARENT window, not
/// "enter" — and silently no-ops when that window already exists.
#[tauri::command]
#[specta::specta]
pub async fn app_open_folder(path: String) -> Result<(), AppError> {
    tokio::task::spawn_blocking(move || {
        crate::core::folder::open_in_file_manager(std::path::Path::new(&path))
    })
    .await
    .map_err(|e| AppError::internal(format!("open folder task join failed: {e}")))?
    .map_err(AppError::from)
}
