use std::collections::HashMap;

use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::{TaskInfo, TaskNotification, TaskState};

/// Manages the lifecycle of background tasks.
///
/// The manager tracks all spawned tasks, collects their notifications,
/// and makes completed task output available for injection into the
/// conversation.
pub struct TaskManager {
    tasks: RwLock<HashMap<String, TaskInfo>>,
    // Notifications are user-visible turn context. Keep the buffer lossless and
    // drain it at turn boundaries rather than dropping older task updates here.
    notifications: RwLock<Vec<TaskNotification>>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
            notifications: RwLock::new(Vec::new()),
        }
    }

    pub async fn register(&self, info: TaskInfo) {
        info!(task_id = %info.id, name = %info.name, "task registered");
        self.tasks.write().await.insert(info.id.clone(), info);
    }

    pub async fn update_state(&self, task_id: &str, state: TaskState) {
        if let Some(info) = self.tasks.write().await.get_mut(task_id) {
            // Background workers can report late completion after cancellation;
            // preserve the first terminal state so user-visible task history is stable.
            if is_terminal_state(info.state) {
                return;
            }

            info.state = state;
            if is_terminal_state(state) {
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

fn is_terminal_state(state: TaskState) -> bool {
    matches!(
        state,
        TaskState::Completed | TaskState::Failed | TaskState::Cancelled
    )
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TaskState;
    use pretty_assertions::assert_eq;

    fn make_task_info(id: &str, name: &str) -> TaskInfo {
        TaskInfo {
            id: id.into(),
            name: name.into(),
            state: TaskState::Pending,
            output: None,
            created_at: chrono::Utc::now(),
            finished_at: None,
        }
    }

    #[tokio::test]
    async fn register_and_get() {
        let mgr = TaskManager::new();
        let info = make_task_info("t1", "compile");
        mgr.register(info.clone()).await;

        assert_eq!(mgr.get("t1").await, Some(info));
    }

    #[tokio::test]
    async fn get_nonexistent_returns_none() {
        let mgr = TaskManager::new();
        assert!(mgr.get("nope").await.is_none());
    }

    #[tokio::test]
    async fn update_state_to_completed() {
        let mgr = TaskManager::new();
        mgr.register(make_task_info("t1", "build")).await;
        mgr.update_state("t1", TaskState::Running).await;

        let task = mgr.get("t1").await.unwrap();
        assert_eq!(task.state, TaskState::Running);
        assert!(task.finished_at.is_none());

        mgr.update_state("t1", TaskState::Completed).await;
        let task = mgr.get("t1").await.unwrap();
        assert_eq!(task.state, TaskState::Completed);
        assert!(task.finished_at.is_some());
    }

    #[tokio::test]
    async fn set_output() {
        let mgr = TaskManager::new();
        let info = make_task_info("t1", "run");
        let mut expected = info.clone();
        expected.output = Some("success output".into());

        mgr.register(info).await;
        mgr.set_output("t1", "success output".into()).await;

        assert_eq!(mgr.get("t1").await, Some(expected));
    }

    #[tokio::test]
    async fn notifications_drain() {
        let mgr = TaskManager::new();
        let step_done = TaskNotification {
            task_id: "t1".into(),
            message: "step 1 done".into(),
            is_final: false,
        };
        let finished = TaskNotification {
            task_id: "t1".into(),
            message: "finished".into(),
            is_final: true,
        };

        mgr.push_notification(step_done.clone()).await;
        mgr.push_notification(finished.clone()).await;

        let notifs = mgr.drain_notifications().await;
        assert_eq!(notifs, vec![step_done, finished]);

        // After drain, should be empty
        let notifs = mgr.drain_notifications().await;
        assert!(notifs.is_empty());
    }

    #[tokio::test]
    async fn list_all_tasks() {
        let mgr = TaskManager::new();
        mgr.register(make_task_info("t1", "a")).await;
        mgr.register(make_task_info("t2", "b")).await;
        mgr.register(make_task_info("t3", "c")).await;

        let tasks = mgr.list().await;
        assert_eq!(tasks.len(), 3);
    }

    #[tokio::test]
    async fn cancel_sets_state_and_finished_at() {
        let mgr = TaskManager::new();
        mgr.register(make_task_info("t1", "long_task")).await;
        mgr.cancel("t1").await;

        let task = mgr.get("t1").await.unwrap();
        assert_eq!(task.state, TaskState::Cancelled);
        assert!(task.finished_at.is_some());
    }

    #[tokio::test]
    async fn terminal_state_is_not_overwritten_by_late_updates() {
        let mgr = TaskManager::new();
        mgr.register(make_task_info("t1", "long_task")).await;
        mgr.update_state("t1", TaskState::Running).await;
        mgr.cancel("t1").await;

        let cancelled = mgr.get("t1").await.unwrap();
        mgr.update_state("t1", TaskState::Completed).await;

        assert_eq!(mgr.get("t1").await.unwrap(), cancelled);
    }

    #[tokio::test]
    async fn update_state_nonexistent_is_no_op() {
        let mgr = TaskManager::new();
        mgr.update_state("nonexistent", TaskState::Failed).await;
        assert!(mgr.get("nonexistent").await.is_none());
    }
}
