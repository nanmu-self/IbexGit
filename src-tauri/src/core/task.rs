use crate::core::error::AppError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(pub u64);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Running,
    Cancelling,
    Cancelled,
    Success,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub kind: String,
    pub repo_id: Option<String>,
    pub status: TaskStatus,
    pub progress: u32,
    pub message: String,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
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
            cancellable: true,
            error: None,
        }
    }
}

#[derive(Debug)]
pub struct TaskHandle {
    pub id: TaskId,
}

/// TaskManager tracks user-facing tasks and their lifecycle.
pub struct TaskManager {
    next_id: u64,
    tasks: Arc<RwLock<HashMap<TaskId, Task>>>,
    cancel_txs: Arc<RwLock<HashMap<TaskId, tokio::sync::oneshot::Sender<()>>>>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            tasks: Arc::new(RwLock::new(HashMap::new())),
            cancel_txs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn create(&mut self, kind: impl Into<String>) -> TaskHandle {
        let id = TaskId(self.next_id);
        self.next_id += 1;
        let mut task = Task::new(id, kind);
        task.status = TaskStatus::Queued;

        let (cancel_tx, _cancel_rx) = tokio::sync::oneshot::channel::<()>();
        let mut tasks = self.tasks.write().await;
        tasks.insert(id, task);
        self.cancel_txs.write().await.insert(id, cancel_tx);

        TaskHandle { id }
    }

    pub async fn start(&self, id: TaskId) {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.get_mut(&id) {
            task.status = TaskStatus::Running;
            task.started_at = Some(chrono::Utc::now().timestamp_millis());
        }
    }

    pub async fn update_progress(&self, id: TaskId, progress: u32, message: impl Into<String>) {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.get_mut(&id) {
            task.progress = progress;
            task.message = message.into();
        }
    }

    pub async fn complete(&self, id: TaskId, error: Option<AppError>) {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.get_mut(&id) {
            task.status = if error.is_some() {
                TaskStatus::Failed
            } else {
                TaskStatus::Success
            };
            task.error = error;
            task.finished_at = Some(chrono::Utc::now().timestamp_millis());
            task.progress = 100;
        }
    }

    pub async fn cancel(&self, id: TaskId) -> Result<(), AppError> {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.get_mut(&id) {
            task.status = TaskStatus::Cancelling;
            task.message = "Cancelling...".to_string();
        }
        // Signal cancellation
        if let Some(tx) = self.cancel_txs.write().await.remove(&id) {
            let _ = tx.send(());
        }
        Ok(())
    }

    pub async fn get(&self, id: TaskId) -> Option<Task> {
        let tasks = self.tasks.read().await;
        tasks.get(&id).cloned()
    }

    pub async fn list(&self) -> Vec<Task> {
        let tasks = self.tasks.read().await;
        tasks.values().cloned().collect()
    }

    pub async fn remove(&self, id: TaskId) {
        let mut tasks = self.tasks.write().await;
        tasks.remove(&id);
        self.cancel_txs.write().await.remove(&id);
    }
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}
