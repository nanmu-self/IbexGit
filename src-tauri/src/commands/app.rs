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

/// Read the system accent color as `#RRGGBB` — the "颜色/Colors" pick in
/// Windows 个性化 / macOS 系统设置, which native apps use for focus &
/// selection highlights. Selection rows in the UI tint with this color so
/// they match the OS instead of a hardcoded app blue.
///
/// - Windows: `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Accent`
///   value `Accent` (stored as 0xAARRGGBB).
/// - macOS: `NSColor.controlAccentColor`.
/// - Linux: no standard API → None, frontend falls back to the theme blue.
///
/// Returns None when the platform has no readable value (or the registry
/// read fails); the frontend then keeps its CSS fallback.
#[tauri::command]
#[specta::specta]
pub fn app_system_accent_color() -> Option<String> {
    let color = read_system_accent().map(|(r, g, b)| format!("#{:02X}{:02X}{:02X}", r, g, b));
    tracing::debug!(accent = ?color, "system accent color resolved");
    color
}

#[cfg(target_os = "windows")]
fn read_system_accent() -> Option<(u8, u8, u8)> {
    // Explicit user pick (HKCU Explorer\Accent value `Accent`, ARGB).
    if let Some(v) = read_accent_registry_value() {
        return Some(v);
    }
    // Wallpaper-derived mode: the `Accent` value is absent and Windows
    // computes the palette from the wallpaper — fall back to the DWM
    // colorization color, which is what the shell tints UI with.
    read_accent_dwm()
}

#[cfg(target_os = "windows")]
fn read_accent_registry_value() -> Option<(u8, u8, u8)> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent")
        .ok()?;
    let value: u32 = key.get_value("Accent").ok()?;
    // The Accent DWORD is ARGB (alpha first, always 0xFF).
    Some((
        ((value >> 16) & 0xFF) as u8,
        ((value >> 8) & 0xFF) as u8,
        (value & 0xFF) as u8,
    ))
}

#[cfg(target_os = "windows")]
fn read_accent_dwm() -> Option<(u8, u8, u8)> {
    use std::os::raw::c_long;
    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmGetColorizationColor(
            colorization_color: *mut u32,
            colorization_color_balance: *mut u32,
        ) -> c_long;
    }
    let mut color: u32 = 0;
    let mut balance: u32 = 0;
    let hr = unsafe { DwmGetColorizationColor(&mut color, &mut balance) };
    if hr == 0 {
        // S_OK
        Some((
            ((color >> 16) & 0xFF) as u8,
            ((color >> 8) & 0xFF) as u8,
            (color & 0xFF) as u8,
        ))
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
fn read_system_accent() -> Option<(u8, u8, u8)> {
    use objc::msg_send;
    use objc::runtime::{Class, Object};
    // + (NSColor *)controlAccentColor — class method.
    let color: *mut Object = unsafe { msg_send![Class::get("NSColor")?, controlAccentColor] };
    if color.is_null() {
        return None;
    }
    // redComponent / greenComponent / blueComponent — CGFloat (f64), 0..1.
    let r: f64 = unsafe { msg_send![color, redComponent] };
    let g: f64 = unsafe { msg_send![color, greenComponent] };
    let b: f64 = unsafe { msg_send![color, blueComponent] };
    let to_byte = |v: f64| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    Some((to_byte(r), to_byte(g), to_byte(b)))
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn read_system_accent() -> Option<(u8, u8, u8)> {
    None
}
