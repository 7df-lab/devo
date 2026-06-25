use std::io::Write;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::task;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_core::AppConfigStore;
use devo_core::BundledSkillsConfig;
use devo_core::ProviderVendorCatalog;
use futures::stream::Stream;
use futures::stream::{self};
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::Duration;
use tokio::time::timeout;

use devo_core::FileSystemSkillCatalog;
use devo_core::PresetModelCatalog;
use devo_core::RolloutLine;
use devo_core::SessionMetaLine;
use devo_core::SessionRecord;
use devo_core::SkillsConfig;
use devo_core::TurnLine;
use devo_core::TurnRecord;
use devo_core::tools::ToolRegistry;
use devo_protocol::Model;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ReasoningCapability;
use devo_protocol::ReasoningEffort;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::ServerEvent;
use devo_protocol::SessionHistoryItemKind;
use devo_protocol::SessionId;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::TurnStatus;
use devo_protocol::Usage;
use devo_provider::ModelProviderSDK;
use devo_provider::SingleProviderRouter;
use devo_server::ClientTransportKind;
use devo_server::ServerRuntime;
use devo_server::ServerRuntimeDependencies;

struct SingleReplyProvider;

#[derive(Default)]
struct CapturingProvider {
    requests: Mutex<Vec<ModelRequest>>,
}

#[async_trait]
impl ModelProviderSDK for SingleReplyProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(ModelResponse {
            id: "title-1".into(),
            content: vec![ResponseContent::Text("Generated rollout title".to_string())],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: ResponseMetadata::default(),
        })
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: "Hello from persistence test.".into(),
            }),
            Ok(StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "resp-1".into(),
                    content: vec![ResponseContent::Text("Hello from persistence test.".into())],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: ResponseMetadata::default(),
                },
            }),
        ])))
    }

    fn name(&self) -> &str {
        "single-reply-test-provider"
    }
}

#[async_trait]
impl ModelProviderSDK for CapturingProvider {
    async fn completion(&self, request: ModelRequest) -> Result<ModelResponse> {
        self.requests.lock().expect("lock requests").push(request);
        Ok(ModelResponse {
            id: "title-1".into(),
            content: vec![ResponseContent::Text("Generated rollout title".to_string())],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: ResponseMetadata::default(),
        })
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        self.requests.lock().expect("lock requests").push(request);
        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: "Captured request reply.".into(),
            }),
            Ok(StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "resp-capture".into(),
                    content: vec![ResponseContent::Text("Captured request reply.".into())],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: ResponseMetadata::default(),
                },
            }),
        ])))
    }

    fn name(&self) -> &str {
        "capturing-provider"
    }
}

/// A stream that yields one TextDelta, then blocks on a oneshot until unblocked or
/// cancelled, then yields MessageDone.  Used by tests that need to interrupt a turn
/// mid-stream to exercise the deferred-item completion race.
struct GatedStream {
    block_rx: oneshot::Receiver<()>,
    state: u8,
}

impl GatedStream {
    fn new(block_rx: oneshot::Receiver<()>) -> Self {
        Self { block_rx, state: 0 }
    }
}

impl Stream for GatedStream {
    type Item = Result<StreamEvent>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> task::Poll<Option<Self::Item>> {
        match self.state {
            0 => {
                self.state = 1;
                task::Poll::Ready(Some(Ok(StreamEvent::TextDelta {
                    index: 0,
                    text: "mid-interrupt content".into(),
                })))
            }
            1 => match Pin::new(&mut self.block_rx).poll(cx) {
                task::Poll::Ready(Ok(())) => {
                    self.state = 2;
                    task::Poll::Ready(Some(Ok(StreamEvent::MessageDone {
                        response: ModelResponse {
                            id: "resp-gated".into(),
                            content: vec![ResponseContent::Text("mid-interrupt content".into())],
                            stop_reason: Some(StopReason::EndTurn),
                            usage: Usage::default(),
                            metadata: ResponseMetadata::default(),
                        },
                    })))
                }
                task::Poll::Ready(Err(_)) => task::Poll::Ready(None),
                task::Poll::Pending => task::Poll::Pending,
            },
            2 => {
                self.state = 3;
                task::Poll::Ready(None)
            }
            _ => task::Poll::Ready(None),
        }
    }
}

/// Provider whose stream blocks mid-way, letting the test send an interrupt while
/// the assistant item is still in-progress.
struct GatedProvider {
    /// Kept alive so the oneshot receiver in GatedStream blocks forever
    /// (or until the task is aborted, dropping the receiver).
    _block_tx: Mutex<Option<oneshot::Sender<()>>>,
    /// Receiver taken by the first completion_stream call.
    block_rx: Mutex<Option<oneshot::Receiver<()>>>,
}

impl GatedProvider {
    fn new() -> Self {
        let (tx, rx) = oneshot::channel();
        Self {
            _block_tx: Mutex::new(Some(tx)),
            block_rx: Mutex::new(Some(rx)),
        }
    }
}

#[async_trait]
impl ModelProviderSDK for GatedProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(ModelResponse {
            id: "title-gated".into(),
            content: vec![ResponseContent::Text("Gated title".to_string())],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: ResponseMetadata::default(),
        })
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let rx = self
            .block_rx
            .lock()
            .expect("lock block_rx")
            .take()
            .expect("completion_stream called more than once");
        Ok(Box::pin(GatedStream::new(rx)))
    }

    fn name(&self) -> &str {
        "gated-provider"
    }
}

#[tokio::test]
async fn runtime_rebuilds_sessions_from_rollout_and_resume_works() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": "Persistent session",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    let turn_start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "_devo/turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "persist this session" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response")?;
    let _: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(turn_start_response)?;

    wait_for_turn_completed(&mut notifications_rx).await?;

    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (rebuilt_connection_id, _rebuilt_notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;

    let list_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 3,
                "method": "session/list",
                "params": {}
            }),
        )
        .await
        .context("session/list response")?;
    let sessions = decode_acp_session_list_response(list_response)?;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, session_id);
    assert_eq!(sessions[0].title.as_deref(), Some("Persistent session"));

    let resume_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 4,
                "method": "_devo/session/resume",
                "params": {
                    "session_id": session_id
                }
            }),
        )
        .await
        .context("session/resume response")?;
    let resume_result = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionResumeResult>,
    >(resume_response)?
    .result;

    assert_eq!(resume_result.session.session_id, session_id);
    assert_eq!(
        resume_result.session.title.as_deref(),
        Some("Persistent session")
    );
    assert!(resume_result.loaded_item_count >= 2);
    assert!(resume_result.latest_turn.is_some());
    Ok(())
}

#[tokio::test]
async fn runtime_generates_final_title_and_persists_explicit_rename() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 11,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": null,
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 12,
                "method": "_devo/turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "implement rollout persistence for the rust server" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response")?;

    wait_for_title_update(&mut notifications_rx, "Generated rollout title").await?;
    wait_for_turn_completed(&mut notifications_rx).await?;

    let resume_after_completion = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 13,
                "method": "_devo/session/resume",
                "params": {
                    "session_id": session_id
                }
            }),
        )
        .await
        .context("session/resume response after completion")?;
    let completed_result = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionResumeResult>,
    >(resume_after_completion)?
    .result;
    assert_eq!(
        completed_result.session.title.as_deref(),
        Some("Generated rollout title")
    );
    assert_eq!(
        completed_result.session.title_state,
        devo_core::SessionTitleState::Final(devo_core::SessionTitleFinalSource::ModelGenerated)
    );

    let rename_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 14,
                "method": "_devo/session/title/update",
                "params": {
                    "session_id": session_id,
                    "title": "Rollout persistence follow-up"
                }
            }),
        )
        .await
        .context("session/title/update response")?;
    let rename_result = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionTitleUpdateResult>,
    >(rename_response)?
    .result;
    assert_eq!(
        rename_result.session.title.as_deref(),
        Some("Rollout persistence follow-up")
    );
    assert_eq!(
        rename_result.session.title_state,
        devo_core::SessionTitleState::Final(devo_core::SessionTitleFinalSource::UserRename)
    );

    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (rebuilt_connection_id, _notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;
    let resume_after_rebuild = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 15,
                "method": "_devo/session/resume",
                "params": {
                    "session_id": session_id
                }
            }),
        )
        .await
        .context("session/resume response after rebuild")?;
    let rebuilt_result = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionResumeResult>,
    >(resume_after_rebuild)?
    .result;
    assert_eq!(
        rebuilt_result.session.title.as_deref(),
        Some("Rollout persistence follow-up")
    );
    assert_eq!(
        rebuilt_result.session.title_state,
        devo_core::SessionTitleState::Final(devo_core::SessionTitleFinalSource::UserRename)
    );
    Ok(())
}

#[tokio::test]
async fn runtime_assigns_provisional_title_after_first_prompt() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 21,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": null,
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 22,
                "method": "_devo/turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "investigate why the current session title stays null" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response")?;

    let provisional_title = wait_for_any_title_update(&mut notifications_rx).await?;
    assert_eq!(
        provisional_title,
        "Investigate why the current session title stays null"
    );

    let list_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 23,
                "method": "session/list",
                "params": {}
            }),
        )
        .await
        .context("session/list response")?;
    let sessions = decode_acp_session_list_response(list_response)?;
    assert_eq!(
        sessions[0].title.as_deref(),
        Some("Investigate why the current session title stays null")
    );
    assert_eq!(
        sessions[0].title_state,
        devo_core::SessionTitleState::Provisional
    );
    Ok(())
}

#[tokio::test]
async fn runtime_skips_invalid_rollout_files_when_loading_sessions() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 31,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": "Valid session",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 32,
                "method": "_devo/turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "persist the valid session" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response")?;

    wait_for_turn_completed(&mut notifications_rx).await?;

    let bad_rollout_dir = data_root.path().join("sessions/2026/04/28");
    std::fs::create_dir_all(&bad_rollout_dir)?;
    let bad_rollout_path =
        bad_rollout_dir.join("rollout-2026-04-28T15-12-34Z-legacy-invalid.jsonl");
    std::fs::write(
        &bad_rollout_path,
        "{ definitely not valid json\n{\"still\":\"broken\"}\n",
    )?;

    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (rebuilt_connection_id, _notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;

    let list_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 33,
                "method": "session/list",
                "params": {}
            }),
        )
        .await
        .context("session/list response")?;
    let sessions = decode_acp_session_list_response(list_response)?;

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, session_id);
    assert_eq!(sessions[0].title.as_deref(), Some("Valid session"));
    Ok(())
}

#[tokio::test]
async fn resume_normalizes_historical_default_reasoning_effort() -> Result<()> {
    fn write_historical_rollout(
        data_root: &std::path::Path,
        session_id: &SessionId,
        reasoning_effort_selection: Option<String>,
    ) -> Result<()> {
        let now = chrono::Utc::now();
        let rollout_dir = data_root.join("sessions/2026/06/07");
        std::fs::create_dir_all(&rollout_dir)?;
        let rollout_path =
            rollout_dir.join(format!("rollout-2026-06-07T00-00-00-{session_id}.jsonl"));
        let session = SessionRecord {
            id: *session_id,
            rollout_path: rollout_path.clone(),
            created_at: now,
            updated_at: now,
            last_activity_at: Some(now),
            source: "cli".into(),
            agent_nickname: None,
            agent_role: None,
            agent_path: None,
            model_provider: "openai_chat_completions".into(),
            model: Some("deepseek-v4-flash".into()),
            model_binding_id: None,
            reasoning_effort_selection: reasoning_effort_selection.clone(),
            cwd: data_root.to_path_buf(),
            additional_directories: Vec::new(),
            cli_version: "0.1.0".into(),
            title: Some("Historical session".into()),
            title_state: devo_core::SessionTitleState::Final(
                devo_core::SessionTitleFinalSource::ExplicitCreate,
            ),
            sandbox_policy: "workspace-write".into(),
            approval_mode: "on-request".into(),
            tokens_used: 0,
            first_user_message: None,
            archived_at: None,
            git_sha: None,
            git_branch: None,
            git_origin_url: None,
            parent_session_id: None,
            session_context: None,
            latest_turn_context: None,
            schema_version: 2,
        };
        let turn = TurnRecord {
            id: devo_protocol::TurnId::new(),
            session_id: *session_id,
            sequence: 1,
            started_at: now,
            completed_at: Some(now),
            status: TurnStatus::Completed,
            kind: devo_core::TurnKind::Regular,
            model: "deepseek-v4-flash".into(),
            model_binding_id: None,
            reasoning_effort_selection,
            request_model: "deepseek-v4-flash".into(),
            request_thinking: Some("default".into()),
            input_token_estimate: None,
            usage: None,
            stop_reason: None,
            failure_reason: None,
            session_context: None,
            turn_context: None,
            schema_version: 2,
        };

        let mut file = std::fs::File::create(&rollout_path)?;
        writeln!(
            file,
            "{}",
            serde_json::to_string(&RolloutLine::SessionMeta(Box::new(SessionMetaLine {
                timestamp: now,
                session,
            })))?
        )?;
        writeln!(
            file,
            "{}",
            serde_json::to_string(&RolloutLine::Turn(Box::new(TurnLine {
                timestamp: now,
                turn,
            })))?
        )?;
        Ok(())
    }

    let data_root = TempDir::new()?;
    let missing_thinking_session = SessionId::new();
    let default_thinking_session = SessionId::new();
    write_historical_rollout(data_root.path(), &missing_thinking_session, None)?;
    write_historical_rollout(
        data_root.path(),
        &default_thinking_session,
        Some("default".into()),
    )?;

    let runtime = build_runtime(data_root.path())?;
    runtime.load_persisted_sessions().await?;
    let (connection_id, _notifications_rx) = initialize_connection(&runtime).await?;

    for session_id in [&missing_thinking_session, &default_thinking_session] {
        let resume_response = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 34,
                    "method": "_devo/session/resume",
                    "params": {
                        "session_id": session_id
                    }
                }),
            )
            .await
            .context("session/resume response")?;
        let resume_result = serde_json::from_value::<
            devo_server::SuccessResponse<devo_server::SessionResumeResult>,
        >(resume_response)?
        .result;

        assert_eq!(resume_result.session.session_id, (*session_id).clone());
        assert_eq!(
            resume_result.session.model.as_deref(),
            Some("deepseek-v4-flash")
        );
        assert_eq!(
            resume_result.session.reasoning_effort_selection.as_deref(),
            Some("high")
        );
        assert_eq!(
            resume_result.session.reasoning_effort,
            Some(ReasoningEffort::High)
        );
    }

    Ok(())
}

#[tokio::test]
async fn runtime_recovers_session_when_middle_rollout_line_is_corrupted() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 41,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": "Recoverable session",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 42,
                "method": "_devo/turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "persist this session before corruption" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response")?;

    wait_for_turn_completed(&mut notifications_rx).await?;

    let sessions_root = data_root.path().join("sessions");
    let rollout_path = std::fs::read_dir(&sessions_root)?
        .next()
        .context("expected year partition")??
        .path();
    let rollout_path = std::fs::read_dir(rollout_path)?
        .next()
        .context("expected month partition")??
        .path();
    let rollout_path = std::fs::read_dir(rollout_path)?
        .next()
        .context("expected day partition")??
        .path();
    let rollout_path = std::fs::read_dir(rollout_path)?
        .next()
        .context("expected rollout file")??
        .path();

    let mut lines = std::fs::read_to_string(&rollout_path)?
        .lines()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    assert!(lines.len() >= 4);
    lines[2] = "{\"Turn\":{\"timestamp\":\"broken\"".to_string();
    std::fs::write(&rollout_path, format!("{}\n", lines.join("\n")))?;

    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (rebuilt_connection_id, _notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;

    let resume_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 43,
                "method": "_devo/session/resume",
                "params": {
                    "session_id": session_id
                }
            }),
        )
        .await
        .context("session/resume response")?;
    let resume_result = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionResumeResult>,
    >(resume_response)?
    .result;

    assert_eq!(resume_result.session.session_id, session_id);
    assert_eq!(
        resume_result.session.title.as_deref(),
        Some("Recoverable session")
    );
    assert!(resume_result.loaded_item_count >= 1);
    Ok(())
}

#[tokio::test]
async fn session_compact_runs_asynchronously_and_emits_lifecycle_events() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 51,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": "Compaction session",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 52,
                "method": "_devo/turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "create some history first" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response")?;

    wait_for_turn_completed(&mut notifications_rx).await?;

    let compact_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 53,
                "method": "_devo/session/compact",
                "params": {
                    "session_id": session_id
                }
            }),
        )
        .await
        .context("session/compact response")?;
    let compact_result = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionCompactResult>,
    >(compact_response)?
    .result;
    assert_eq!(compact_result.session.session_id, session_id);

    wait_for_notification_method(&mut notifications_rx, "session/compaction/started").await?;
    wait_for_notification_method(&mut notifications_rx, "session/compaction/completed").await?;
    Ok(())
}

#[tokio::test]
async fn compacted_session_resume_keeps_full_transcript_after_restart() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 61,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": "Persist compacted session",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    for request_id in 0..3 {
        let large_prompt = "x".repeat(30_000);
        let _ = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 62 + request_id,
                    "method": "_devo/turn/start",
                    "params": {
                        "session_id": session_id,
                        "input": [{ "type": "text", "text": large_prompt }],
                        "model": null,
                        "sandbox": null,
                        "approval_policy": null,
                        "cwd": null
                    }
                }),
            )
            .await
            .context("turn/start response")?;
        wait_for_turn_completed(&mut notifications_rx).await?;
    }

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 70,
                "method": "_devo/session/compact",
                "params": {
                    "session_id": session_id
                }
            }),
        )
        .await
        .context("session/compact response")?;
    wait_for_notification_method(&mut notifications_rx, "session/compaction/completed").await?;

    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (rebuilt_connection_id, _notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;

    let resume_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 71,
                "method": "_devo/session/resume",
                "params": {
                    "session_id": session_id
                }
            }),
        )
        .await
        .context("session/resume response")?;
    let resume_result = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionResumeResult>,
    >(resume_response)?
    .result;

    assert!(
        resume_result.history_items.len() >= 6,
        "expected full transcript to survive compaction, got {:?}",
        resume_result.history_items
    );
    assert!(
        resume_result
            .history_items
            .iter()
            .all(|item| !item.body.contains("<compaction_summary>")),
        "compaction summary must not appear in user-visible transcript"
    );
    assert!(
        resume_result
            .history_items
            .iter()
            .any(|item| item.body.contains("Hello from persistence test.")),
        "expected assistant transcript entries to remain visible"
    );
    Ok(())
}

#[tokio::test]
async fn compacted_session_next_query_uses_compaction_summary_after_restart() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 81,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": "Prompt snapshot session",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    for request_id in 0..3 {
        let large_prompt = "x".repeat(30_000);
        let _ = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 82 + request_id,
                    "method": "_devo/turn/start",
                    "params": {
                        "session_id": session_id,
                        "input": [{ "type": "text", "text": large_prompt }],
                        "model": null,
                        "sandbox": null,
                        "approval_policy": null,
                        "cwd": null
                    }
                }),
            )
            .await
            .context("turn/start response")?;
        wait_for_turn_completed(&mut notifications_rx).await?;
    }

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 90,
                "method": "_devo/session/compact",
                "params": {
                    "session_id": session_id
                }
            }),
        )
        .await
        .context("session/compact response")?;
    wait_for_notification_method(&mut notifications_rx, "session/compaction/completed").await?;

    let capturing_provider = Arc::new(CapturingProvider::default());
    let rebuilt_runtime =
        build_runtime_with_provider(data_root.path(), capturing_provider.clone())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (rebuilt_connection_id, mut rebuilt_notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;
    let resume_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 90,
                "method": "_devo/session/resume",
                "params": {
                    "session_id": session_id
                }
            }),
        )
        .await
        .context("session/resume response after restart")?;
    let _: devo_server::SuccessResponse<devo_server::SessionResumeResult> =
        serde_json::from_value(resume_response)?;

    let _ = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 91,
                "method": "_devo/turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "go on" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response after restart")?;
    wait_for_turn_completed(&mut rebuilt_notifications_rx).await?;

    let requests = capturing_provider.requests.lock().expect("lock requests");
    let request = requests
        .last()
        .context("expected captured model request after restart")?;

    assert!(
        request.messages.iter().any(|message| {
            message.content.iter().any(|content| match content {
                devo_protocol::RequestContent::Text { text }
                | devo_protocol::RequestContent::Reasoning { text } => {
                    text.contains("<compaction_summary>")
                }
                devo_protocol::RequestContent::ProviderReasoning { .. }
                | devo_protocol::RequestContent::ToolUse { .. }
                | devo_protocol::RequestContent::HostedToolUse { .. }
                | devo_protocol::RequestContent::ToolResult { .. } => false,
            })
        }),
        "expected prompt request to include compaction summary after restart"
    );
    Ok(())
}

#[tokio::test]
async fn configured_model_name_is_used_for_turn_metadata_and_provider_request() -> Result<()> {
    let data_root = TempDir::new()?;
    std::fs::create_dir_all(data_root.path().join(".devo"))?;
    std::fs::write(
        data_root.path().join(".devo").join("models.json"),
        r#"
[
  {
    "slug": "test-model",
    "display_name": "test-model",
    "provider": "openai_chat_completions",
    "reasoning_capability": "toggle",
    "reasoning_implementation": {
      "model_variant": {
        "variants": [
          {
            "selection_value": "disabled",
            "model_slug": "test-model",
            "reasoning_effort": null,
            "label": "Off",
            "description": "Disable reasoning effort"
          },
          {
            "selection_value": "enabled",
            "model_slug": "vendor/test-model",
            "reasoning_effort": "medium",
            "label": "On",
            "description": "Enable reasoning effort"
          }
        ]
      }
    },
    "base_instructions": "Test model",
    "priority": 999
  }
]
"#,
    )?;
    let provider = Arc::new(CapturingProvider::default());
    let runtime = build_runtime_with_provider(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 101,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": "Model name session",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 102,
                "method": "_devo/turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "use configured model name" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response")?;
    let turn_started = wait_for_notification_value(&mut notifications_rx, "turn/started").await?;
    wait_for_turn_completed(&mut notifications_rx).await?;

    assert_eq!(
        turn_started["params"]["turn"]["model"],
        serde_json::json!("test-model")
    );
    assert_eq!(
        turn_started["params"]["turn"]["request_model"],
        serde_json::json!("vendor/test-model")
    );
    let requests = provider.requests.lock().expect("lock requests");
    assert_eq!(
        requests.last().expect("captured request").model,
        "vendor/test-model"
    );

    Ok(())
}

fn build_runtime(data_root: &std::path::Path) -> Result<Arc<ServerRuntime>> {
    build_runtime_with_provider(data_root, Arc::new(SingleReplyProvider))
}

fn build_runtime_with_provider(
    data_root: &std::path::Path,
    provider: Arc<dyn ModelProviderSDK>,
) -> Result<Arc<ServerRuntime>> {
    let db_path = data_root.join("test_persistence.db");
    let db = Arc::new(devo_server::db::Database::open(db_path).expect("open test database"));
    Ok(ServerRuntime::new(
        data_root.to_path_buf(),
        ServerRuntimeDependencies::new(
            Arc::clone(&provider),
            Arc::new(SingleProviderRouter::new(provider)),
            Arc::new(ToolRegistry::new()),
            "test-model".to_string(),
            Arc::new(PresetModelCatalog::new(vec![
                Model {
                    slug: "test-model".to_string(),
                    display_name: "test-model".to_string(),
                    ..Model::default()
                },
                Model {
                    slug: "deepseek-v4-flash".to_string(),
                    display_name: "deepseek-v4-flash".to_string(),
                    reasoning_capability: ReasoningCapability::ToggleWithLevels(vec![
                        ReasoningEffort::High,
                        ReasoningEffort::Max,
                    ]),
                    default_reasoning_effort: Some(ReasoningEffort::High),
                    ..Model::default()
                },
            ])),
            Arc::new(ProviderVendorCatalog::default()),
            Box::new(FileSystemSkillCatalog::new(SkillsConfig {
                bundled: Some(BundledSkillsConfig { enabled: false }),
                ..SkillsConfig::default()
            })),
            devo_core::AgentsMdConfig::default(),
            db,
            Arc::new(std::sync::Mutex::new(
                AppConfigStore::load(data_root.to_path_buf(), None).expect("load app config store"),
            )),
        ),
    ))
}

async fn initialize_connection(
    runtime: &Arc<ServerRuntime>,
) -> Result<(u64, mpsc::Receiver<serde_json::Value>)> {
    let (notifications_tx, notifications_rx) = mpsc::channel(/*buffer*/ 4096);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, notifications_tx)
        .await;
    let initialize_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 10,
                "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientCapabilities": {},
                    "clientInfo": {
                        "name": "test",
                        "title": "test",
                        "version": "1.0.0"
                    }
                }
            }),
        )
        .await
        .context("initialize response")?;
    let response: serde_json::Value = initialize_response;
    assert_eq!(
        response["result"]["agentInfo"]["name"],
        serde_json::json!("devo-server")
    );
    Ok((connection_id, notifications_rx))
}

fn decode_acp_session_list_response(
    response: serde_json::Value,
) -> Result<Vec<devo_server::SessionMetadata>> {
    let response: devo_server::AcpSuccessResponse<devo_server::AcpListSessionsResult> =
        serde_json::from_value(response)?;
    response
        .result
        .sessions
        .into_iter()
        .map(|session| {
            session
                .meta
                .as_ref()
                .and_then(|meta| meta.get(devo_server::DEVO_SESSION_META))
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .context("decode Devo session metadata from ACP session/list response")?
                .with_context(|| {
                    format!(
                        "ACP session/list response missing Devo session metadata for {}",
                        session.session_id
                    )
                })
        })
        .collect()
}

fn legacy_event_from_acp_notification(value: serde_json::Value) -> serde_json::Value {
    if value.get("method") != Some(&serde_json::json!("session/update")) {
        return value;
    }
    let Ok(notification) =
        serde_json::from_value::<devo_protocol::AcpSessionNotification>(value["params"].clone())
    else {
        return value;
    };
    let Some((method, event)) = devo_protocol::original_event_from_acp_notification(&notification)
    else {
        return value;
    };
    let params = match event {
        ServerEvent::TurnCompleted(payload)
        | ServerEvent::TurnInterrupted(payload)
        | ServerEvent::TurnFailed(payload)
        | ServerEvent::TurnStarted(payload) => serde_json::to_value(payload),
        ServerEvent::SessionCompactionStarted(payload)
        | ServerEvent::SessionCompactionCompleted(payload)
        | ServerEvent::SessionTitleUpdated(payload)
        | ServerEvent::SessionStarted(payload) => serde_json::to_value(payload),
        ServerEvent::ItemCompleted(payload) | ServerEvent::ItemStarted(payload) => {
            serde_json::to_value(payload)
        }
        ServerEvent::ItemDelta {
            delta_kind,
            payload,
        } => serde_json::to_value(serde_json::json!({
            "delta_kind": delta_kind,
            "payload": payload,
        })),
        other => serde_json::to_value(other),
    }
    .expect("serialize legacy event params");
    serde_json::json!({
        "method": method,
        "params": params,
    })
}

fn title_from_notification(value: &serde_json::Value) -> Option<&str> {
    if value.get("method") == Some(&serde_json::json!("session/title/updated")) {
        return value["params"]["session"]["title"].as_str();
    }
    if value.get("method") == Some(&serde_json::json!("session/update"))
        && value["params"]["update"]["sessionUpdate"] == serde_json::json!("session_info_update")
    {
        return value["params"]["update"]["title"].as_str();
    }
    None
}

fn notification_matches_method(value: &serde_json::Value, method: &str) -> bool {
    value.get("method") == Some(&serde_json::json!(method))
        || (method == "item/agentMessage/delta"
            && value.get("method") == Some(&serde_json::json!("session/update"))
            && value["params"]["update"]["sessionUpdate"]
                == serde_json::json!("agent_message_chunk"))
}

async fn wait_for_turn_completed(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
) -> Result<()> {
    timeout(Duration::from_secs(5), async {
        while let Some(value) = notifications_rx.recv().await {
            let value = legacy_event_from_acp_notification(value);
            if value.get("method") == Some(&serde_json::json!("turn/completed")) {
                return Ok(());
            }
        }
        anyhow::bail!("notification channel closed before turn/completed")
    })
    .await
    .context("timed out waiting for turn/completed")??;
    Ok(())
}

async fn wait_for_title_update(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    expected_title: &str,
) -> Result<()> {
    timeout(Duration::from_secs(5), async {
        while let Some(value) = notifications_rx.recv().await {
            if title_from_notification(&value) == Some(expected_title) {
                return Ok(());
            }
        }
        anyhow::bail!("notification channel closed before expected title update")
    })
    .await
    .context("timed out waiting for title update")??;
    Ok(())
}

async fn wait_for_any_title_update(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
) -> Result<String> {
    timeout(Duration::from_secs(5), async {
        while let Some(value) = notifications_rx.recv().await {
            if let Some(title) = title_from_notification(&value) {
                return Ok(title.to_string());
            }
        }
        anyhow::bail!("notification channel closed before any title update")
    })
    .await
    .context("timed out waiting for title update")?
}

async fn wait_for_notification_method(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    method: &str,
) -> Result<()> {
    wait_for_notification_value(notifications_rx, method)
        .await
        .map(|_| ())
}

async fn wait_for_notification_value(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    method: &str,
) -> Result<serde_json::Value> {
    let value = timeout(Duration::from_secs(5), async {
        while let Some(value) = notifications_rx.recv().await {
            let normalized = legacy_event_from_acp_notification(value.clone());
            if notification_matches_method(&normalized, method)
                || notification_matches_method(&value, method)
            {
                return Ok(normalized);
            }
        }
        anyhow::bail!("notification channel closed before {method}")
    })
    .await
    .with_context(|| format!("timed out waiting for {method}"))??;
    Ok(value)
}

#[tokio::test]
async fn interrupt_mid_stream_does_not_duplicate_last_item_on_resume() -> Result<()> {
    let data_root = TempDir::new()?;
    let gated = Arc::new(GatedProvider::new());
    let runtime = build_runtime_with_provider(data_root.path(), Arc::clone(&gated) as _)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": null,
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    let turn_start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "_devo/turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "interrupt me" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start")?;
    let turn_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::TurnStartResult>,
    >(turn_start_response)?
    .result
    .turn_id()
    .expect("turn/start should start a streaming turn");

    // Wait until the assistant item has started streaming.  The provider yields
    // one TextDelta, then blocks, so once we see the delta notification we know
    // deferred_assistant has been stored in the session.
    wait_for_notification_method(&mut notifications_rx, "item/agentMessage/delta").await?;

    // Now interrupt the turn while it is still in-progress.
    let interrupt_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 3,
                "method": "_devo/turn/interrupt",
                "params": {
                    "session_id": session_id,
                    "turn_id": turn_id,
                    "reason": "test duplicate bug"
                }
            }),
        )
        .await
        .context("turn/interrupt")?;
    let interrupt_result: devo_server::SuccessResponse<devo_server::TurnInterruptResult> =
        serde_json::from_value(interrupt_response)?;
    assert_eq!(interrupt_result.result.status, TurnStatus::Interrupted);

    // The server broadcasts both turn/interrupted and turn/completed.
    wait_for_notification_method(&mut notifications_rx, "turn/interrupted").await?;
    wait_for_notification_method(&mut notifications_rx, "turn/completed").await?;

    // Rebuild runtime (simulates restart) and resume the session.
    let gated2 = Arc::new(GatedProvider::new());
    let rebuilt = build_runtime_with_provider(data_root.path(), Arc::clone(&gated2) as _)?;
    rebuilt.load_persisted_sessions().await?;
    let (rebuilt_cid, _) = initialize_connection(&rebuilt).await?;

    let resume_response = rebuilt
        .handle_incoming(
            rebuilt_cid,
            serde_json::json!({
                "id": 4,
                "method": "_devo/session/resume",
                "params": {
                    "session_id": session_id
                }
            }),
        )
        .await
        .context("session/resume")?;
    let resume_result = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionResumeResult>,
    >(resume_response)?
    .result;

    // The crucial assertion: no two consecutive items should have the same
    // kind if they are Assistant or Reasoning — those are the types that
    // were being duplicated by the event_task post-loop cleanup race.
    let kinds: Vec<_> = resume_result
        .history_items
        .iter()
        .map(|i| &i.kind)
        .collect();
    for window in kinds.windows(2) {
        if window[0] == window[1] {
            match window[0] {
                SessionHistoryItemKind::Assistant | SessionHistoryItemKind::Reasoning => {
                    anyhow::bail!(
                        "duplicate consecutive {:?} items detected: indices {:?}",
                        window[0],
                        kinds
                            .iter()
                            .enumerate()
                            .filter_map(|(idx, k)| {
                                if *k == window[0] { Some(idx) } else { None }
                            })
                            .collect::<Vec<_>>()
                    );
                }
                _ => {}
            }
        }
    }

    // Sanity: there should be exactly one User and one Assistant item.
    let user_count = kinds
        .iter()
        .filter(|k| matches!(k, SessionHistoryItemKind::User))
        .count();
    let assistant_count = kinds
        .iter()
        .filter(|k| matches!(k, SessionHistoryItemKind::Assistant))
        .count();
    assert_eq!(user_count, 1, "expected exactly one User item");
    assert_eq!(
        assistant_count, 1,
        "expected exactly one Assistant item, got history: {kinds:?}"
    );

    Ok(())
}
