use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use devo_protocol::{
    AwaitTaskResult, CommandTaskMetadata, SessionId, TaskId, TaskInfo, TaskKind, TaskState,
};
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

use super::unified_exec::process::UnifiedExecProcess;
use super::unified_exec::store::ProcessStore;

#[derive(Clone)]
pub(crate) struct BackgroundTaskStore {
    tasks: Arc<Mutex<HashMap<TaskId, Arc<CommandTask>>>>,
    process_store: Arc<ProcessStore>,
}

struct CommandTask {
    owner_session_id: SessionId,
    process: Arc<UnifiedExecProcess>,
    state: Mutex<CommandTaskState>,
    notify: Notify,
}

#[derive(Debug)]
struct CommandTaskState {
    info: TaskInfo,
    output: Option<String>,
}

impl BackgroundTaskStore {
    pub(crate) fn new(process_store: Arc<ProcessStore>) -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            process_store,
        }
    }

    pub(crate) async fn register_command(
        &self,
        owner_session_id: SessionId,
        process_id: i32,
        command: String,
        process: Arc<UnifiedExecProcess>,
    ) -> TaskInfo {
        let task_id = TaskId(format!("task-{}", Uuid::new_v4()));
        let info = TaskInfo {
            task_id: task_id.clone(),
            kind: TaskKind::Command,
            state: TaskState::Running,
            agent: None,
            command: Some(CommandTaskMetadata {
                process_id,
                command,
                exit_code: None,
            }),
        };
        self.tasks.lock().await.insert(
            task_id,
            Arc::new(CommandTask {
                owner_session_id,
                process,
                state: Mutex::new(CommandTaskState {
                    info: info.clone(),
                    output: None,
                }),
                notify: Notify::new(),
            }),
        );
        info
    }

    pub(crate) async fn complete_command(
        &self,
        task_id: &TaskId,
        exit_code: Option<i32>,
        output: String,
    ) {
        let Some(task) = self.tasks.lock().await.get(task_id).cloned() else {
            return;
        };
        let mut state = task.state.lock().await;
        if state.info.state == TaskState::Canceled {
            return;
        }
        state.info.state = if exit_code == Some(0) {
            TaskState::Completed
        } else {
            TaskState::Failed
        };
        if let Some(command) = state.info.command.as_mut() {
            command.exit_code = exit_code;
        }
        state.output = Some(output);
        let process_id = state
            .info
            .command
            .as_ref()
            .map(|command| command.process_id);
        drop(state);
        if let Some(process_id) = process_id {
            self.process_store.remove(process_id).await;
        }
        task.notify.notify_waiters();
    }

    pub(crate) async fn list(&self, owner_session_id: SessionId) -> Vec<TaskInfo> {
        let tasks = self
            .tasks
            .lock()
            .await
            .values()
            .filter(|task| task.owner_session_id == owner_session_id)
            .cloned()
            .collect::<Vec<_>>();
        let mut infos = Vec::with_capacity(tasks.len());
        for task in tasks {
            infos.push(task.state.lock().await.info.clone());
        }
        infos.sort_by(|left, right| left.task_id.0.cmp(&right.task_id.0));
        infos
    }

    pub(crate) async fn await_task(
        &self,
        owner_session_id: SessionId,
        task_id: &TaskId,
        timeout: Duration,
    ) -> Option<AwaitTaskResult> {
        let task = self.tasks.lock().await.get(task_id).cloned()?;
        if task.owner_session_id != owner_session_id {
            return None;
        }
        let started = Instant::now();
        loop {
            let notified = task.notify.notified();
            let state = task.state.lock().await;
            if matches!(
                state.info.state,
                TaskState::Completed | TaskState::Failed | TaskState::Canceled
            ) {
                return Some(AwaitTaskResult::Terminal {
                    task: state.info.clone(),
                    output: state.output.clone(),
                });
            }
            if started.elapsed() >= timeout {
                return Some(AwaitTaskResult::TimedOut {
                    task: state.info.clone(),
                });
            }
            let remaining = timeout.saturating_sub(started.elapsed());
            drop(state);
            if tokio::time::timeout(remaining, notified).await.is_err() {
                let state = task.state.lock().await;
                return Some(AwaitTaskResult::TimedOut {
                    task: state.info.clone(),
                });
            }
        }
    }

    pub(crate) async fn cancel(
        &self,
        owner_session_id: SessionId,
        task_id: &TaskId,
    ) -> Option<TaskInfo> {
        let task = self.tasks.lock().await.get(task_id).cloned()?;
        if task.owner_session_id != owner_session_id {
            return None;
        }
        task.process.terminate();
        let mut state = task.state.lock().await;
        state.info.state = TaskState::Canceled;
        state.output = None;
        let info = state.info.clone();
        let process_id = info.command.as_ref().map(|command| command.process_id);
        drop(state);
        if let Some(process_id) = process_id {
            self.process_store.remove(process_id).await;
        }
        task.notify.notify_waiters();
        Some(info)
    }
}
