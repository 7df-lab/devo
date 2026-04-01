use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// The lifecycle state of a background task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// A notification emitted by a task that can be fed back into the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNotification {
    pub task_id: String,
    pub message: String,
    pub is_final: bool,
}

/// Metadata describing a running or completed task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInfo {
    pub id: String,
    pub name: String,
    pub state: TaskState,
    pub output: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// A long-running unit of work managed by the task runtime.
///
/// Tasks are the key abstraction separating synchronous tool calls from
/// background execution. They support lifecycle tracking, cancellation,
/// and notification back to the main conversation.
#[async_trait]
pub trait Task: Send + Sync {
    fn id(&self) -> &str;
    fn name(&self) -> &str;

    /// Start the task. Returns when the task completes or fails.
    async fn run(&self) -> anyhow::Result<String>;

    /// Request graceful cancellation.
    async fn cancel(&self) -> anyhow::Result<()>;

    fn state(&self) -> TaskState;
}
