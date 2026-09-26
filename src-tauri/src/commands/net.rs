//! Network / credential commands (P7)：克隆（可取消 + 实时进度）、新建仓库、
//! 凭据管理、host key 信任、代理与 SSH 配置。
//!
//! 克隆任务：`git_clone` 返回 taskId 并在后台执行；进度经 `CloneEvent`
//! 事件推送（`clone://progress` 语义）；`clone_cancel` 经 CancelToken 整树
//! 终止并清理半成品目录。PLAN P7 取消语义：凭据取消 / 用户取消 →
//! `cancelled`（而非 failed）。

use crate::core::credential::{CredentialBroker, CredentialEntry, CredentialReply, KnownHost};
use crate::core::engine::{parse, CloneOptions};
use crate::core::error::AppError;
use crate::core::repo::RepoManager;
use crate::core::runner::{CancelToken, ProxyMode, SharedNetConfig};
use crate::core::task::TaskManager;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;
use tokio::sync::Mutex;

/// AppHandle 托管态：命令内发事件不能直接拿 `AppHandle`（会把
/// `collect_commands!` 钉死在 Wry，破坏 tests/bindings 的 MockRuntime 泛型），
/// 经 state 绕开。仅运行 Wry，运行期具体化是安全的。
pub struct AppEmitter(pub tauri::AppHandle);

/// 克隆任务进度事件（tauri-specta 事件 `CloneEvent`）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct CloneEvent {
    pub task_id: u32,
    /// `start` | `progress` | `done` | `cancelled` | `failed`
    pub phase: String,
    /// 进度阶段：receiving / resolving / updating / counting / …
    pub stage: Option<String>,
    pub percent: Option<u32>,
    pub message: String,
    pub error: Option<AppError>,
}

/// 克隆任务注册表：taskId → CancelToken（完成/取消后移除）。
/// taskId 与任务中心的 TaskManager id 同源（见 `git_clone`）。
#[derive(Default)]
pub struct CloneTasks(Arc<Mutex<HashMap<u32, CancelToken>>>);

fn validate_url(url: &str) -> Result<(), AppError> {
    let ok = ["https://", "http://", "ssh://", "git://", "file://"]
        .iter()
        .any(|p| url.starts_with(p))
        // scp-like shorthand: git@github.com:owner/repo.git
        || {
            url.contains('@')
                && url.contains(':')
                && !url.contains("://")
                && !url.starts_with('-')
                && !url.starts_with('/')
        }
        // local path
        || (url.contains('/') || url.contains('\\') || url.contains(':'))
            && !url.starts_with('-');
    if ok && !url.contains(char::is_whitespace) {
        Ok(())
    } else {
        Err(AppError::parse(format!("invalid repository url: {url:?}")))
    }
}

/// 克隆请求参数（命令层打包，避免 8 参数命令）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct CloneRequest {
    pub url: String,
    pub dest: String,
    pub depth: Option<u32>,
    pub single_branch: bool,
    pub recurse_submodules: bool,
}

#[tauri::command]
#[specta::specta]
pub async fn git_clone(
    request: CloneRequest,
    emitter: State<'_, AppEmitter>,
    repos: State<'_, RepoManager>,
    tasks: State<'_, CloneTasks>,
    manager: State<'_, TaskManager>,
) -> Result<u32, AppError> {
    let CloneRequest {
        url,
        dest,
        depth,
        single_branch,
        recurse_submodules,
    } = request;
    let url = url.trim().to_string();
    let dest = dest.trim().to_string();
    if url.is_empty() {
        return Err(AppError::parse("clone: url is required"));
    }
    if dest.is_empty() {
        return Err(AppError::parse("clone: destination path is required"));
    }
    validate_url(&url)?;
    if depth.is_some_and(|d| d == 0 || d > 100_000) {
        return Err(AppError::parse("clone: depth must be 1..=100000"));
    }

    let desc = format!("clone {url}");
    let opts = CloneOptions {
        url,
        dest,
        depth,
        single_branch,
        recurse_submodules,
    };

    let token = CancelToken::new();
    // 任务中心（P12）：克隆是用户可见长任务；TaskManager 持有同一个
    // CancelToken（task_cancel 与 clone_cancel 双通道等价），其任务 id
    // 兼作 CloneEvent 的 task_id，两条事件流同源不漂移。
    let tm_task = manager
        .create("clone", None, desc, true, Some(token.clone()))
        .await;
    manager.start(tm_task).await;
    let task_id = u32::try_from(tm_task.0).unwrap_or(u32::MAX);
    tasks.0.lock().await.insert(task_id, token.clone());

    let engine = repos.engine();
    let registry = tasks.inner().0.clone();
    let tm = manager.inner().clone();
    let app = emitter.inner().0.clone();

    tauri::async_runtime::spawn(async move {
        use tauri_specta::Event as _;
        let app_for_emit = app.clone();
        let emit = |phase: &str,
                    stage: Option<String>,
                    percent: Option<u32>,
                    message: String,
                    error: Option<AppError>| {
            let _ = CloneEvent {
                task_id,
                phase: phase.to_string(),
                stage,
                percent,
                message,
                error,
            }
            .emit(&app_for_emit);
        };
        emit("start", None, None, format!("clone {}", opts.url), None);

        let app_for_lines = app_for_emit.clone();
        let tm_lines = tm.clone();
        let on_line: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |line: String| {
            let p = parse::parse_clone_progress(&line);
            tm_lines.try_update_progress(tm_task, p.percent, line.clone());
            let _ = CloneEvent {
                task_id,
                phase: "progress".into(),
                stage: Some(p.phase),
                percent: p.percent,
                message: line,
                error: None,
            }
            .emit(&app_for_lines);
        });

        let result = engine.clone_repo(&opts, Some(&token), Some(on_line)).await;
        registry.lock().await.remove(&task_id);
        match result {
            Ok(()) => {
                tm.complete(tm_task, None).await;
                emit(
                    "done",
                    None,
                    Some(100),
                    format!("cloned into {}", opts.dest),
                    None,
                )
            }
            Err(AppError::OperationCancelled | AppError::CredentialCancelled) => {
                tm.mark_cancelled(tm_task).await;
                emit("cancelled", None, None, "clone cancelled".to_string(), None)
            }
            Err(e) => {
                tm.complete(tm_task, Some(e.clone())).await;
                emit("failed", None, None, e.to_string(), Some(e))
            }
        }
    });

    Ok(task_id)
}

#[tauri::command]
#[specta::specta]
pub async fn clone_cancel(task_id: u32, tasks: State<'_, CloneTasks>) -> Result<(), AppError> {
    let token = tasks.0.lock().await.remove(&task_id);
    match token {
        Some(t) => {
            t.cancel();
            Ok(())
        }
        None => Err(AppError::parse(format!("unknown clone task {task_id}"))),
    }
}

// =====================
// 新建空仓库（P7）
// =====================

#[tauri::command]
#[specta::specta]
pub async fn repo_init(
    path: String,
    readme: bool,
    gitignore: bool,
    repos: State<'_, RepoManager>,
) -> Result<(), AppError> {
    let dir = PathBuf::from(path.trim());
    if dir.as_os_str().is_empty() {
        return Err(AppError::parse("init: path is required"));
    }
    if dir.exists() {
        let non_empty = std::fs::read_dir(&dir)
            .map(|mut it| it.next().is_some())
            .unwrap_or(false);
        if non_empty {
            return Err(AppError::parse(format!(
                "path '{}' already exists and is not empty",
                dir.display()
            )));
        }
    } else {
        std::fs::create_dir_all(&dir)
            .map_err(|e| AppError::io_with_detail(e.to_string(), "create directory"))?;
    }
    repos.engine().init_repo(&dir.to_string_lossy()).await?;
    // 模板文件不自动提交（避免替用户伪造 author 身份），留给提交框。
    if readme {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Project".to_string());
        std::fs::write(dir.join("README.md"), format!("# {name}\n"))
            .map_err(|e| AppError::io(e.to_string()))?;
    }
    if gitignore {
        std::fs::write(
            dir.join(".gitignore"),
            "# Build artifacts\n/target/\nbuild/\ndist/\n*.log\n.DS_Store\n",
        )
        .map_err(|e| AppError::io(e.to_string()))?;
    }
    Ok(())
}

// =====================
// 凭据管理（P7）
// =====================

#[tauri::command]
#[specta::specta]
pub async fn credential_list(
    broker: State<'_, Arc<CredentialBroker>>,
) -> Result<Vec<CredentialEntry>, AppError> {
    Ok(broker.list_index())
}

#[tauri::command]
#[specta::specta]
pub async fn credential_delete(
    key: String,
    broker: State<'_, Arc<CredentialBroker>>,
) -> Result<(), AppError> {
    broker.delete_credential(&key)
}

#[tauri::command]
#[specta::specta]
pub async fn credential_respond(
    request_id: String,
    reply: CredentialReply,
    broker: State<'_, Arc<CredentialBroker>>,
) -> Result<(), AppError> {
    broker.respond(&request_id, reply)
}

#[tauri::command]
#[specta::specta]
pub async fn known_hosts_list(
    broker: State<'_, Arc<CredentialBroker>>,
) -> Result<Vec<KnownHost>, AppError> {
    Ok(broker.list_known_hosts())
}

#[tauri::command]
#[specta::specta]
pub async fn known_hosts_remove(
    host: String,
    broker: State<'_, Arc<CredentialBroker>>,
) -> Result<(), AppError> {
    broker.remove_known_host(&host)
}

// =====================
// 网络/SSH 配置（P7）
// =====================

/// 前端下发的网络配置（settings.json 持久化在前端 store）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct NetConfig {
    /// `inherit` | `none` | `custom`
    pub proxy_mode: String,
    pub proxy_url: Option<String>,
    pub ssh_key_path: Option<String>,
}

#[tauri::command]
#[specta::specta]
pub async fn app_set_net_config(
    config: NetConfig,
    net: State<'_, SharedNetConfig>,
) -> Result<(), AppError> {
    let proxy_mode = ProxyMode::parse(&config.proxy_mode);
    let proxy_url = config.proxy_url.filter(|s| !s.trim().is_empty());
    let ssh_key_path = config.ssh_key_path.filter(|s| !s.trim().is_empty());
    net.update(|c| {
        c.proxy_mode = proxy_mode;
        c.proxy_url = proxy_url;
        c.ssh_key_path = ssh_key_path;
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_url;

    #[test]
    fn accepts_common_urls() {
        for u in [
            "https://github.com/o/r.git",
            "http://example.com/r.git",
            "ssh://git@github.com/o/r.git",
            "git@github.com:o/r.git",
            "git://example.com/r",
            "file:///tmp/repo",
            "/local/path/repo",
            "C:\\repos\\demo",
            "./relative/repo",
        ] {
            assert!(validate_url(u).is_ok(), "{u} must be accepted");
        }
    }

    #[test]
    fn rejects_garbage_and_flag_injection() {
        for u in ["", "   ", "-c credential.helper=evil", "justaword"] {
            assert!(validate_url(u).is_err(), "{u:?} must be rejected");
        }
    }
}
