//! SSH 密钥管理命令（设置中心）：`~/.ssh` 列出 / 生成 / 删除。
//!
//! 非 git 能力：不持 RepoManager，也不走 WriteGate（不触碰仓库与
//! index.lock）。生成（尤其 RSA）可能秒级耗时，统一丢 spawn_blocking。

use crate::core::error::AppError;
use crate::core::sshkeys::{self, SshKeyGenerateRequest, SshKeyInfo};

/// 列出 `~/.ssh` 下可识别的成对密钥（目录不存在 = 空列表）。
#[tauri::command]
#[specta::specta]
pub async fn ssh_key_list() -> Result<Vec<SshKeyInfo>, AppError> {
    let dir = sshkeys::ssh_dir()?;
    tokio::task::spawn_blocking(move || sshkeys::list_keys(&dir))
        .await
        .map_err(|e| AppError::io_with_detail(e.to_string(), "ssh_key_list join"))?
}

/// 生成密钥对（Ed25519 / RSA），可选口令加密私钥。
#[tauri::command]
#[specta::specta]
pub async fn ssh_key_generate(req: SshKeyGenerateRequest) -> Result<SshKeyInfo, AppError> {
    let dir = sshkeys::ssh_dir()?;
    tokio::task::spawn_blocking(move || sshkeys::generate_key(&dir, &req))
        .await
        .map_err(|e| AppError::io_with_detail(e.to_string(), "ssh_key_generate join"))?
}

/// 删除密钥（私钥 + `.pub`；路径必须位于 `~/.ssh` 内）。
#[tauri::command]
#[specta::specta]
pub async fn ssh_key_delete(path: String) -> Result<(), AppError> {
    let dir = sshkeys::ssh_dir()?;
    tokio::task::spawn_blocking(move || sshkeys::delete_key(&dir, &path))
        .await
        .map_err(|e| AppError::io_with_detail(e.to_string(), "ssh_key_delete join"))?
}
