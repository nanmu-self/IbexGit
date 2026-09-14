use crate::core::compat::GitCapabilities;
use tauri::State;

/// Return the detected git capabilities as JSON.
#[tauri::command]
pub fn git_version(caps: State<'_, GitCapabilities>) -> Result<String, String> {
    Ok(format!("{:?}", *caps))
}

/// Greet command (legacy from template).
#[tauri::command]
pub fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}
