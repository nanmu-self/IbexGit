//! 任务中心数据层（P12）：用户可见长任务的生命周期追踪。
//!
//! TaskManager 以「全量快照」为同步协议：每次变更都把整个 [`Task`]
//! 克隆推入 sink 通道（`lib.rs` 的转发循环把它包成 `TaskEvent` 发给
//! 前端），前端按 id upsert，不需要增量补丁语义。核心层不感知
//! AppHandle / Wry（MockRuntime 测试可无 sink 运行）。
//!
//! 取消语义：`create` 可携带一个 [`CancelToken`]（克隆任务与 git 子进程
//! 共享同一 token），`cancel` 只负责置位 token 并把状态标为 Cancelling，
//! 真正的终止由执行方轮询 token 完成。无 token 的任务（fetch/pull/push，
//! engine 暂不支持中断）`cancellable = false`，面板不出现取消按钮。

use crate::core::error::AppError;
use crate::core::repo::u64_as_string;
use crate::core::runner::CancelToken;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

/// 内存上限：超过后优先淘汰最早的已终结任务，没有则淘汰最早的任务。
const MAX_TASKS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Running,
    Cancelling,
    Cancelled,
    Success,
    Failed,
}

impl TaskStatus {
    /// 终结态：不会再变更，可被 `clear_finished` 清理。
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Cancelled | Self::Success | Self::Failed)
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, specta::Type,
)]
pub struct TaskId(
    /// 字符串序列化（u64 超 JS 安全整数，同 RepoId 约定）。
    #[serde(with = "u64_as_string")]
    #[specta(type = String)]
    pub u64,
);

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct Task {
    pub id: TaskId,
    pub kind: String,
    pub repo_id: Option<String>,
    pub status: TaskStatus,
    pub progress: u32,
    pub message: String,
    /// 毫秒时间戳（f64 同 RecoveryEntry.created_at_ms 约定，JS number 安全）。
    pub started_at: Option<f64>,
    pub finished_at: Option<f64>,
    pub cancellable: bool,
    pub error: Option<AppError>,
}

impl Task {
    pub fn new(id: TaskId, kind: impl Into<String>) -> Self {
        Self {
            id,
            kind: kind.into(),
            repo_id: None,
            status: TaskStatus::Queued,
            progress: 0,
            message: String::new(),
            started_at: None,
            finished_at: None,
            cancellable: false,
            error: None,
        }
    }
}

/// TaskManager tracks user-facing tasks and their lifecycle. Cheap to clone
/// (all state behind Arc); every mutation pushes the full task snapshot
/// through the sink channel (see module doc).
#[derive(Clone)]
pub struct TaskManager {
    next_id: Arc<AtomicU64>,
    tasks: Arc<RwLock<HashMap<TaskId, Task>>>,
    cancel_tokens: Arc<RwLock<HashMap<TaskId, CancelToken>>>,
    events: Option<mpsc::UnboundedSender<Task>>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self::with_sink(None)
    }

    pub fn with_sink(events: Option<mpsc::UnboundedSender<Task>>) -> Self {
        Self {
            next_id: Arc::new(AtomicU64::new(1)),
            tasks: Arc::new(RwLock::new(HashMap::new())),
            cancel_tokens: Arc::new(RwLock::new(HashMap::new())),
            events,
        }
    }

    fn emit(&self, task: &Task) {
        if let Some(tx) = &self.events {
            let _ = tx.send(task.clone());
        }
    }

    /// Register a queued task. `token` (when given) is fired by [`Self::cancel`]
    /// and must be the same token the executor polls.
    pub async fn create(
        &self,
        kind: impl Into<String>,
        repo_id: Option<String>,
        message: impl Into<String>,
        cancellable: bool,
        token: Option<CancelToken>,
    ) -> TaskId {
        let id = TaskId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let mut task = Task::new(id, kind);
        task.repo_id = repo_id;
        task.message = message.into();
        task.cancellable = cancellable;
        self.tasks.write().await.insert(id, task.clone());
        if let Some(t) = token {
            self.cancel_tokens.write().await.insert(id, t);
        }
        self.prune_cap().await;
        self.emit(&task);
        id
    }

    pub async fn start(&self, id: TaskId) {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.get_mut(&id) {
            task.status = TaskStatus::Running;
            task.started_at = Some(chrono::Utc::now().timestamp_millis() as f64);
            self.emit(task);
        }
    }

    /// Update the progress / message; `progress = None` keeps the value
    /// (line-driven stages often have no percentage).
    pub async fn update_progress(
        &self,
        id: TaskId,
        progress: Option<u32>,
        message: impl Into<String>,
    ) {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.get_mut(&id) {
            if let Some(p) = progress {
                task.progress = p;
            }
            task.message = message.into();
            self.emit(task);
        }
    }

    /// Fire-and-forget variant for sync callbacks (git stdout pumps): skips
    /// the update when the lock is momentarily contended.
    pub fn try_update_progress(
        &self,
        id: TaskId,
        progress: Option<u32>,
        message: impl Into<String>,
    ) {
        if let Ok(mut tasks) = self.tasks.try_write() {
            if let Some(task) = tasks.get_mut(&id) {
                if let Some(p) = progress {
                    task.progress = p;
                }
                task.message = message.into();
                self.emit(task);
            }
        }
    }

    /// Terminal transition: success (`error = None`) or failed.
    pub async fn complete(&self, id: TaskId, error: Option<AppError>) {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.get_mut(&id) {
            task.status = if error.is_some() {
                TaskStatus::Failed
            } else {
                TaskStatus::Success
            };
            task.error = error;
            task.finished_at = Some(chrono::Utc::now().timestamp_millis() as f64);
            task.progress = 100;
            self.cancel_tokens.write().await.remove(&id);
            self.emit(task);
        }
    }

    /// Mark a task cancelled after its CancelToken fired (executor observed
    /// the signal and aborted cleanly). Unknown / already-terminal: no-op.
    pub async fn mark_cancelled(&self, id: TaskId) {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.get_mut(&id) {
            if task.status.is_terminal() {
                return;
            }
            task.status = TaskStatus::Cancelled;
            task.finished_at = Some(chrono::Utc::now().timestamp_millis() as f64);
            self.cancel_tokens.write().await.remove(&id);
            self.emit(task);
        }
    }

    /// User-requested cancel: mark Cancelling and fire the task's token.
    /// Tasks without a token can be signalled but will run to completion.
    pub async fn cancel(&self, id: TaskId) -> Result<(), AppError> {
        let mut tasks = self.tasks.write().await;
        let Some(task) = tasks.get_mut(&id) else {
            return Err(AppError::parse(format!("unknown task {id:?}")));
        };
        if task.status.is_terminal() {
            return Ok(());
        }
        task.status = TaskStatus::Cancelling;
        let snapshot = task.clone();
        if let Some(token) = self.cancel_tokens.read().await.get(&id) {
            token.cancel();
        }
        self.emit(&snapshot);
        Ok(())
    }

    pub async fn get(&self, id: TaskId) -> Option<Task> {
        self.tasks.read().await.get(&id).cloned()
    }

    /// All tasks, oldest first (the panel re-sorts for display).
    pub async fn list(&self) -> Vec<Task> {
        let mut tasks = self
            .tasks
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        tasks.sort_by_key(|t| t.id);
        tasks
    }

    /// Drop every terminal task; returns how many were removed.
    pub async fn clear_finished(&self) -> u32 {
        let mut tasks = self.tasks.write().await;
        let before = tasks.len();
        tasks.retain(|_, t| !t.status.is_terminal());
        let removed = (before - tasks.len()) as u32;
        if removed > 0 {
            self.cancel_tokens
                .write()
                .await
                .retain(|id, _| tasks.contains_key(id));
        }
        removed
    }

    pub async fn remove(&self, id: TaskId) {
        self.tasks.write().await.remove(&id);
        self.cancel_tokens.write().await.remove(&id);
    }

    /// Enforce [`MAX_TASKS`]: evict oldest terminal tasks first, then the
    /// oldest task overall.
    async fn prune_cap(&self) {
        let mut tasks = self.tasks.write().await;
        while tasks.len() >= MAX_TASKS {
            let oldest_terminal = tasks
                .values()
                .filter(|t| t.status.is_terminal())
                .min_by_key(|t| t.id.0)
                .map(|t| t.id);
            let victim =
                oldest_terminal.or_else(|| tasks.values().min_by_key(|t| t.id.0).map(|t| t.id));
            match victim {
                Some(id) => {
                    tasks.remove(&id);
                    self.cancel_tokens.write().await.remove(&id);
                }
                None => break,
            }
        }
    }
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

    fn manager_with_sink() -> (TaskManager, mpsc::UnboundedReceiver<Task>) {
        let (tx, rx) = unbounded_channel();
        (TaskManager::with_sink(Some(tx)), rx)
    }

    #[tokio::test]
    async fn lifecycle_emits_full_snapshots() {
        let (tm, mut rx) = manager_with_sink();
        let id = tm
            .create("fetch", Some("repo".into()), "fetch origin", false, None)
            .await;
        tm.start(id).await;
        tm.update_progress(id, Some(42), "halfway").await;
        tm.complete(id, None).await;

        let statuses: Vec<TaskStatus> = {
            let mut v = Vec::new();
            while let Ok(t) = rx.try_recv() {
                v.push(t.status);
            }
            v
        };
        assert_eq!(
            statuses,
            vec![
                TaskStatus::Queued,
                TaskStatus::Running,
                TaskStatus::Running,
                TaskStatus::Success
            ]
        );
        let done = tm.get(id).await.unwrap();
        assert_eq!(done.message, "halfway");
        assert_eq!(done.progress, 100);
        assert!(done.finished_at.is_some());
    }

    #[tokio::test]
    async fn progress_none_keeps_percent() {
        let (tm, _rx) = manager_with_sink();
        let id = tm.create("clone", None, "clone url", true, None).await;
        tm.start(id).await;
        tm.update_progress(id, Some(30), "receiving").await;
        tm.update_progress(id, None, "resolving").await;
        let t = tm.get(id).await.unwrap();
        assert_eq!(t.progress, 30);
        assert_eq!(t.message, "resolving");
    }

    #[tokio::test]
    async fn cancel_fires_token_and_marks_cancelling() {
        let (tm, _rx) = manager_with_sink();
        let token = CancelToken::new();
        let id = tm
            .create("clone", None, "clone url", true, Some(token.clone()))
            .await;
        tm.cancel(id).await.unwrap();
        assert!(token.is_cancelled());
        assert_eq!(tm.get(id).await.unwrap().status, TaskStatus::Cancelling);
        // Executor observes the token and reports a clean abort.
        tm.mark_cancelled(id).await;
        assert_eq!(tm.get(id).await.unwrap().status, TaskStatus::Cancelled);
        assert!(tm.get(id).await.unwrap().finished_at.is_some());
    }

    #[tokio::test]
    async fn cancel_unknown_task_errors() {
        let (tm, _rx) = manager_with_sink();
        assert!(tm.cancel(TaskId(999)).await.is_err());
    }

    #[tokio::test]
    async fn clear_finished_keeps_running() {
        let (tm, _rx) = manager_with_sink();
        let a = tm.create("fetch", None, "a", false, None).await;
        let b = tm.create("push", None, "b", false, None).await;
        let c = tm.create("pull", None, "c", false, None).await;
        tm.complete(a, None).await;
        tm.complete(b, Some(AppError::parse("boom"))).await;
        tm.start(c).await;
        assert_eq!(tm.clear_finished().await, 2);
        let left = tm.list().await;
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, c);
        assert_eq!(left[0].status, TaskStatus::Running);
    }

    #[tokio::test]
    async fn cap_evicts_oldest_finished_first() {
        let (tm, _rx) = manager_with_sink();
        let first = tm.create("fetch", None, "first", false, None).await;
        tm.complete(first, None).await;
        // Fill up to the cap minus one: 1 + 198 = 199 tasks, all finished.
        for i in 0..198 {
            let id = tm.create("fetch", None, format!("t{i}"), false, None).await;
            tm.complete(id, None).await;
        }
        // One more create hits the cap and evicts the oldest finished task.
        let extra = tm.create("fetch", None, "extra", false, None).await;
        assert!(tm.get(first).await.is_none());
        assert!(tm.get(extra).await.is_some());
        assert!(tm.get(TaskId(199)).await.is_some());
    }
}
