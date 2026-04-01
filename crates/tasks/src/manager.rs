use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::{TaskInfo, TaskNotification, TaskState};

/// Manages the lifecycle of background tasks.
///
/// The manager tracks all spawned tasks, collects their notifications,
/// and makes completed task output available for injection into the
/// conversation.
pub struct TaskManager {
    tasks: Arc<RwLock<HashMap<String, TaskInfo>>>,
    notifications: Arc<RwLock<Vec<TaskNotification>>>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            notifications: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub async fn register(&self, info: TaskInfo) {
        info!(task_id = %info.id, name = %info.name, "task registered");
        self.tasks.write().await.insert(info.id.clone(), info);
    }

    pub async fn update_state(&self, task_id: &str, state: TaskState) {
        if let Some(info) = self.tasks.write().await.get_mut(task_id) {
            info.state = state;
            if matches!(
                state,
                TaskState::Completed | TaskState::Failed | TaskState::Cancelled
            ) {
                info.finished_at = Some(chrono::Utc::now());
            }
        }
    }

    pub async fn set_output(&self, task_id: &str, output: String) {
        if let Some(info) = self.tasks.write().await.get_mut(task_id) {
            info.output = Some(output);
        }
    }

    pub async fn push_notification(&self, notification: TaskNotification) {
        info!(task_id = %notification.task_id, "task notification");
        self.notifications.write().await.push(notification);
    }

    /// Drain all pending notifications for injection into the next turn.
    pub async fn drain_notifications(&self) -> Vec<TaskNotification> {
        let mut notifs = self.notifications.write().await;
        std::mem::take(&mut *notifs)
    }

    pub async fn get(&self, task_id: &str) -> Option<TaskInfo> {
        self.tasks.read().await.get(task_id).cloned()
    }

    pub async fn list(&self) -> Vec<TaskInfo> {
        self.tasks.read().await.values().cloned().collect()
    }

    pub async fn cancel(&self, task_id: &str) {
        warn!(task_id, "cancel requested");
        self.update_state(task_id, TaskState::Cancelled).await;
    }
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}
