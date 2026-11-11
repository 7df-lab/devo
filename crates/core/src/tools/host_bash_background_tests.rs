use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use devo_protocol::{
    AgentInfo, AgentListParams, AgentMessageParams, AgentMessageResult, CloseAgentParams,
    CloseAgentResult, CollaborationMode, SessionId, SpawnAgentParams, SpawnAgentResult,
    WaitAgentParams, WaitAgentResult,
};
use devo_tools::{AgentToolCoordinator, ToolAgentScope};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{SandboxRetry, cancel_session, control, start};
use crate::contracts::{ToolBudgets, ToolCallError, ToolContext};
use crate::invocation::ToolCallId;

#[derive(Debug, PartialEq)]
enum WorkEvent {
    Started(SessionId, String),
    Completed(SessionId, String, Value),
}

struct RecordingCoordinator(mpsc::UnboundedSender<WorkEvent>);

#[async_trait]
impl AgentToolCoordinator for RecordingCoordinator {
    fn background_work_started(&self, session_id: SessionId, work_id: String) {
        self.0
            .send(WorkEvent::Started(session_id, work_id))
            .unwrap();
    }

    fn background_work_completed(&self, session_id: SessionId, work_id: String, result: String) {
        self.0
            .send(WorkEvent::Completed(
                session_id,
                work_id,
                serde_json::from_str(&result).unwrap(),
            ))
            .unwrap();
    }

    async fn spawn_agent(
        self: Arc<Self>,
        _: SpawnAgentParams,
    ) -> Result<SpawnAgentResult, ToolCallError> {
        unreachable!("not an agent test")
    }

    async fn send_message(
        self: Arc<Self>,
        _: AgentMessageParams,
    ) -> Result<AgentMessageResult, ToolCallError> {
        unreachable!("not an agent test")
    }

    async fn wait_agent(
        self: Arc<Self>,
        _: WaitAgentParams,
    ) -> Result<WaitAgentResult, ToolCallError> {
        unreachable!("not an agent test")
    }

    async fn list_agents(
        self: Arc<Self>,
        _: AgentListParams,
    ) -> Result<Vec<AgentInfo>, ToolCallError> {
        unreachable!("not an agent test")
    }

    async fn close_agent(
        self: Arc<Self>,
        _: CloseAgentParams,
    ) -> Result<CloseAgentResult, ToolCallError> {
        unreachable!("not an agent test")
    }
}

fn context(session_id: SessionId, coordinator: Arc<RecordingCoordinator>) -> ToolContext {
    ToolContext {
        output_store: None,
        tool_call_id: ToolCallId("background-lifecycle".into()),
        session_id,
        turn_id: None,
        workspace_root: std::env::temp_dir(),
        budgets: ToolBudgets {
            output_limit_bytes: 32 * 1024,
            wall_time_limit_ms: Some(10_000),
        },
        cancel_token: CancellationToken::new(),
        agent_scope: ToolAgentScope::Parent,
        collaboration_mode: CollaborationMode::Build,
        agent_coordinator: Some(coordinator),
        client_filesystem: None,
        file_read_ledger: None,
        network_proxy: None,
        network_no_proxy: None,
        sandbox_profile: Some("off".into()),
        sandbox_permission_overlay: None,
        kernel: None,
        python_cell_first_wait_ms: None,
        python_cell_watch: None,
        python_cell_completion: None,
        session_dir: None,
    }
}

#[tokio::test]
async fn success_and_failure_publish_retained_terminal_snapshot() {
    for command in ["printf result", "printf failed; exit 7"] {
        let session_id = SessionId::new();
        let (sender, mut events) = mpsc::unbounded_channel();
        let started = start(
            context(session_id, Arc::new(RecordingCoordinator(sender))),
            json!({"command": command}),
            SandboxRetry::Never,
            /*kernel_lifetime*/ None,
        );
        let id = started["result"]["handle_id"].as_str().unwrap();
        let work_id = format!("bash:{id}");
        assert_eq!(
            events.try_recv().unwrap(),
            WorkEvent::Started(session_id, work_id.clone())
        );
        let completed = tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .unwrap()
            .unwrap();
        let retained = control(session_id.as_str(), "bash.poll", &json!({"handle_id": id}));
        assert_eq!(
            completed,
            WorkEvent::Completed(session_id, work_id, retained.clone())
        );
        assert_eq!(retained["result"]["running"], false);
        // Native task/list observations survive Python's automatic forget.
        let observed = super::session_activities(session_id.as_str());
        assert_eq!(observed.len(), 1);
        let snapshot = observed[0].clone();
        assert_eq!(snapshot.command, command);
        assert!(!snapshot.running);
        assert_eq!(
            snapshot.output,
            retained["result"]["output"].as_str().unwrap()
        );
        assert_eq!(snapshot.exit_code, retained["result"]["exit_code"].as_i64());
        assert_eq!(super::session_activities(SessionId::new().as_str()), vec![]);
        assert_eq!(
            control(
                session_id.as_str(),
                "bash.forget",
                &json!({"handle_id": id})
            ),
            json!({"status":"ok","result":{"forgotten":true}})
        );
        assert_eq!(
            super::session_activities(session_id.as_str()),
            vec![snapshot.clone()]
        );
        assert_eq!(super::activity_snapshot(id), Some(snapshot));
        assert!(matches!(
            events.try_recv(),
            Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected)
        ));
        cancel_session(session_id.as_str());
    }
}

#[tokio::test]
async fn cancellation_publishes_terminal_result_even_after_session_cleanup() {
    let session_id = SessionId::new();
    let (sender, mut events) = mpsc::unbounded_channel();
    let started = start(
        context(session_id, Arc::new(RecordingCoordinator(sender))),
        json!({"command": "sleep 30"}),
        SandboxRetry::Never,
        /*kernel_lifetime*/ None,
    );
    let id = started["result"]["handle_id"].as_str().unwrap();
    let work_id = format!("bash:{id}");
    assert_eq!(
        events.try_recv().unwrap(),
        WorkEvent::Started(session_id, work_id.clone())
    );
    cancel_session(session_id.as_str());
    let completed = tokio::time::timeout(Duration::from_secs(10), events.recv())
        .await
        .unwrap()
        .unwrap();
    let WorkEvent::Completed(owner, key, retained) = completed else {
        panic!("expected terminal callback");
    };
    assert_eq!((owner, key), (session_id, work_id));
    assert_eq!(retained["result"]["running"], false);
    assert!(
        retained["result"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("cancelled"))
    );
    assert!(matches!(
        events.try_recv(),
        Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected)
    ));
}
