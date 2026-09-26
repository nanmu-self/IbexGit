//! 任务中心命令面（P12）：TaskManager 的只读 / 取消 / 清理入口。
//!
//! 进度不走命令拉取：TaskManager 每次变更推全量快照进 sink 通道，
//! `lib.rs` 的转发循环包成 [`TaskEvent`] 发给前端（`tasks.svelte.ts`
//! upsert）。命令只在挂载时拉一次初始列表。

use crate::core::error::AppError;
use crate::core::task::{Task, TaskId, TaskManager};
use serde::{Deserialize, Serialize};
use tauri::State;
/// 后台任务快照事件：TaskManager 每次生命周期变更推送一条。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct TaskEvent {
    pub task: Task,
}

/// 当前全部任务（旧 → 新；面板自行排序展示）。
#[tauri::command]
#[specta::specta]
pub async fn task_list(manager: State<'_, TaskManager>) -> Result<Vec<Task>, AppError> {
    Ok(manager.list().await)
}

/// 取消一个任务：置位其 CancelToken（若有）并标记 Cancelling。
#[tauri::command]
#[specta::specta]
pub async fn task_cancel(id: TaskId, manager: State<'_, TaskManager>) -> Result<(), AppError> {
    manager.cancel(id).await
}

/// 清理全部已终结任务，返回删除数。
#[tauri::command]
#[specta::specta]
pub async fn task_clear_finished(manager: State<'_, TaskManager>) -> Result<u32, AppError> {
    Ok(manager.clear_finished().await)
}
