use std::io::Write;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::task;

#[path = "support/rollout.rs"]
mod support;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_core::AppConfigStore;
use futures::stream::Stream;
use futures::stream::{self};
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::Duration;
use tokio::time::timeout;

use devo_core::LegacyRolloutLine;
use devo_core::ModelCatalog;
use devo_core::PresetModelCatalog;
use devo_core::SessionRecord;
use devo_core::TurnError;
use devo_core::TurnRecord;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::SessionId;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::TurnStatus;
use devo_protocol::Usage;
use devo_provider::ModelProviderSDK;
use devo_server::ClientTransportKind;
use devo_server::ServerRuntime;
use devo_server::test_support::TestRuntime;

struct SingleReplyProvider;

struct UsageReplyProvider {
    input_tokens: usize,
    output_tokens: usize,
    cache_read_input_tokens: Option<usize>,
}

impl UsageReplyProvider {
    fn new(input_tokens: usize, output_tokens: usize) -> Self {
        Self {
            input_tokens,
            output_tokens,
            cache_read_input_tokens: Some(input_tokens / 2),
        }
    }

    fn usage(&self) -> Usage {
        Usage {
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: self.cache_read_input_tokens,
            reasoning_output_tokens: None,
            total_tokens: Some(self.input_tokens + self.output_tokens),
        }
    }
}

#[derive(Default)]
struct CapturingProvider {
    requests: Mutex<Vec<ModelRequest>>,
}

fn text_response(id: &str, text: &str, usage: Usage) -> ModelResponse {
    ModelResponse {
        id: id.into(),
        content: vec![ResponseContent::Text(text.into())],
        stop_reason: Some(StopReason::EndTurn),
        usage,
        metadata: ResponseMetadata::default(),
    }
}

fn text_stream(
    id: &str,
    text: &str,
    usage: Usage,
) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>> {
    Box::pin(stream::iter(vec![
        Ok(StreamEvent::TextDelta {
            index: 0,
            text: text.into(),
        }),
        Ok(StreamEvent::MessageDone {
            response: text_response(id, text, usage),
        }),
    ]))
}

#[async_trait]
impl ModelProviderSDK for SingleReplyProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(text_response(
            "title-1",
            "Generated rollout title",
            Usage::default(),
        ))
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        Ok(text_stream(
            "resp-1",
            "Hello from persistence test.",
            Usage::default(),
        ))
    }

    fn name(&self) -> &str {
        "single-reply-test-provider"
    }
}

#[async_trait]
impl ModelProviderSDK for UsageReplyProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(text_response(
            "title-usage",
            "Generated usage title",
            self.usage(),
        ))
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        let usage = self.usage();
        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::UsageDelta(usage.clone())),
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: "Usage reply".into(),
            }),
            Ok(StreamEvent::MessageDone {
                response: text_response("resp-usage", "Usage reply", usage),
            }),
        ])))
    }

    fn name(&self) -> &str {
        "usage-reply-test-provider"
    }
}

#[async_trait]
impl ModelProviderSDK for CapturingProvider {
    async fn completion(&self, request: ModelRequest) -> Result<ModelResponse> {
        self.requests.lock().expect("lock requests").push(request);
        Ok(text_response(
            "title-1",
            "Generated rollout title",
            Usage::default(),
        ))
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        self.requests.lock().expect("lock requests").push(request);
        Ok(text_stream(
            "resp-capture",
            "Captured request reply.",
            Usage::default(),
        ))
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

    let session = create_native_session(
        &runtime,
        connection_id,
        1,
        data_root.path(),
        "persistence-resume-session",
        Some("Persistent session"),
        None,
    )
    .await?;
    let session_id = session.id;

    start_turn_and_wait(
        &runtime,
        connection_id,
        2,
        &session_id,
        "persist this session",
        &mut notifications_rx,
    )
    .await?;

    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (rebuilt_connection_id, _rebuilt_notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;

    let sessions = list_sessions(&rebuilt_runtime, rebuilt_connection_id, 3).await?;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id.as_str(), session_id.to_string());
    assert_eq!(sessions[0].title.as_deref(), Some("Persistent session"));

    let resume_result =
        resume_session(&rebuilt_runtime, rebuilt_connection_id, 4, &session_id).await?;

    assert_eq!(resume_result.session.id, session_id);
    assert_eq!(
        resume_result.session.title.as_deref(),
        Some("Persistent session")
    );
    assert_eq!(
        resume_result.session.status,
        devo_protocol::native::session::SessionStatus::Idle
    );
    Ok(())
}

#[tokio::test]
async fn resume_restores_plan_collaboration_mode_from_latest_turn() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let session = create_native_session(
        &runtime,
        connection_id,
        1,
        data_root.path(),
        "persistence-plan-session",
        Some("Plan mode session"),
        None,
    )
    .await?;
    let session_id = session.id;

    let mode_response = metadata_update(
        &runtime,
        connection_id,
        2,
        &session_id,
        serde_json::json!({ "settings": { "mode": "plan" } }),
    )
    .await?;
    assert!(mode_response.get("error").is_none(), "{mode_response}");

    start_turn_and_wait_with(
        &runtime,
        connection_id,
        2,
        &session_id,
        "draft a plan",
        serde_json::json!({ "collaboration_mode": "plan" }),
        &mut notifications_rx,
    )
    .await?;

    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (rebuilt_connection_id, _rebuilt_notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;

    let resume_result =
        resume_session(&rebuilt_runtime, rebuilt_connection_id, 3, &session_id).await?;

    assert_eq!(resume_result.session.settings.mode.as_deref(), Some("plan"));
    Ok(())
}

#[tokio::test]
async fn resume_restores_session_permission_preset_and_plan_mode_without_turn() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, _notifications_rx) = initialize_connection(&runtime).await?;

    let session = create_native_session(
        &runtime,
        connection_id,
        1,
        data_root.path(),
        "persistence-overrides-session",
        Some("Session overrides"),
        None,
    )
    .await?;
    let session_id = session.id;
    let started_model = session.model.model.clone();

    let permissions_response = metadata_update(
        &runtime,
        connection_id,
        2,
        &session_id,
        serde_json::json!({ "settings": { "permissionProfile": "fullAccess" } }),
    )
    .await?;
    let permissions_result = serde_json::from_value::<
        devo_server::SuccessResponse<
            devo_protocol::native::rpc_session::SessionMetadataUpdateResult,
        >,
    >(permissions_response)?
    .result;
    assert_eq!(
        permissions_result.session.settings.permission_profile,
        devo_protocol::native::model::PermissionProfile::FullAccess
    );

    let metadata_result = serde_json::from_value::<
        devo_server::SuccessResponse<
            devo_protocol::native::rpc_session::SessionMetadataUpdateResult,
        >,
    >(
        metadata_update(
            &runtime,
            connection_id,
            3,
            &session_id,
            serde_json::json!({ "settings": { "mode": "plan" } }),
        )
        .await?,
    )?
    .result;
    assert_eq!(
        metadata_result.session.settings.mode.as_deref(),
        Some("plan")
    );
    assert_eq!(
        metadata_result.session.settings.permission_profile,
        devo_protocol::native::model::PermissionProfile::FullAccess
    );
    assert_eq!(metadata_result.session.model.model, started_model);

    drop(runtime);
    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (connection_id, _notifications_rx) = initialize_connection(&rebuilt_runtime).await?;

    let resume_result = resume_session(&rebuilt_runtime, connection_id, 4, &session_id).await?;

    assert_eq!(resume_result.session.settings.mode.as_deref(), Some("plan"));
    assert_eq!(
        resume_result.session.settings.permission_profile,
        devo_protocol::native::model::PermissionProfile::FullAccess
    );
    Ok(())
}

#[tokio::test]
async fn runtime_generates_final_title_and_persists_explicit_rename() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let session = create_native_session(
        &runtime,
        connection_id,
        11,
        data_root.path(),
        "persistence-generated-title-session",
        None,
        None,
    )
    .await?;
    let session_id = session.id;

    start_turn_and_wait(
        &runtime,
        connection_id,
        12,
        &session_id,
        "implement rollout persistence for the rust server",
        &mut notifications_rx,
    )
    .await?;
    wait_for_title_update(&mut notifications_rx, "Generated rollout title").await?;

    let completed_result = resume_session(&runtime, connection_id, 13, &session_id).await?;
    assert_eq!(
        completed_result.session.title.as_deref(),
        Some("Generated rollout title")
    );

    let rename_result = serde_json::from_value::<
        devo_server::SuccessResponse<
            devo_protocol::native::rpc_session::SessionMetadataUpdateResult,
        >,
    >(
        metadata_update(
            &runtime,
            connection_id,
            14,
            &session_id,
            serde_json::json!({ "title": "Rollout persistence follow-up" }),
        )
        .await?,
    )?
    .result;
    assert_eq!(
        rename_result.session.title.as_deref(),
        Some("Rollout persistence follow-up")
    );
    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (rebuilt_connection_id, _notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;
    let rebuilt_result =
        resume_session(&rebuilt_runtime, rebuilt_connection_id, 15, &session_id).await?;
    assert_eq!(
        rebuilt_result.session.title.as_deref(),
        Some("Rollout persistence follow-up")
    );
    Ok(())
}

#[tokio::test]
async fn runtime_assigns_generated_title_after_first_turn() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let session = create_native_session(
        &runtime,
        connection_id,
        21,
        data_root.path(),
        "persistence-generated-title-session-2",
        None,
        None,
    )
    .await?;
    let session_id = session.id;

    start_turn_and_wait(
        &runtime,
        connection_id,
        22,
        &session_id,
        "investigate why the current session title stays null",
        &mut notifications_rx,
    )
    .await?;
    wait_for_title_update(&mut notifications_rx, "Generated rollout title").await?;

    let sessions = list_sessions(&runtime, connection_id, 23).await?;
    assert_eq!(
        sessions[0].title.as_deref(),
        Some("Generated rollout title")
    );
    Ok(())
}

#[tokio::test]
async fn runtime_skips_invalid_rollout_files_when_loading_sessions() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let session = create_native_session(
        &runtime,
        connection_id,
        31,
        data_root.path(),
        "persistence-valid-session",
        Some("Valid session"),
        None,
    )
    .await?;
    let session_id = session.id;

    start_turn_and_wait(
        &runtime,
        connection_id,
        32,
        &session_id,
        "persist the valid session",
        &mut notifications_rx,
    )
    .await?;

    let bad_rollout_dir = data_root.path().join("sessions");
    std::fs::create_dir_all(&bad_rollout_dir)?;
    let bad_rollout_path = bad_rollout_dir.join("legacy-invalid.jsonl");
    std::fs::write(
        &bad_rollout_path,
        "{ definitely not valid json\n{\"still\":\"broken\"}\n",
    )?;

    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (rebuilt_connection_id, _notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;

    let sessions = list_sessions(&rebuilt_runtime, rebuilt_connection_id, 33).await?;

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id.as_str(), session_id.to_string());
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
        let rollout_dir = data_root.join("sessions");
        std::fs::create_dir_all(&rollout_dir)?;
        let rollout_path = rollout_dir.join(format!("{session_id}.jsonl"));
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
            effective_context_window: None,
            tokens_used: 0,
            first_user_message: None,
            archived_at: None,
            git_sha: None,
            git_branch: None,
            git_origin_url: None,
            parent_session_id: None,
            fork_from_id: None,
            fork_at_turn_id: None,
            session_context: None,
            latest_turn_context: None,
            collaboration_mode: None,
            permission_preset: None,
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
            // Live turns persist the binding as a qualified `provider/model`
            // reference; replay resolves the resumed session's model from it.
            model_binding_id: Some("test/deepseek-v4-flash".into()),
            reasoning_effort_selection,
            request_model: "deepseek-v4-flash".into(),
            request_thinking: Some("default".into()),
            input_token_estimate: None,
            usage: None,
            latest_query_usage: None,
            context_occupancy: None,
            stop_reason: None,
            failure_reason: None,
            error: None,
            session_context: None,
            turn_context: None,
            schema_version: 2,
        };

        // The rollout indexer parses only v2 lines, so the fixture writes the
        // v2 envelope through the same record converters the migrate path uses.
        let session_line = devo_core::session_line_from_record(&session, now)?;
        let turn_line = devo_core::turn_line_from_record(&turn, now)?;
        let mut file = std::fs::File::create(&rollout_path)?;
        writeln!(file, "{}", serde_json::to_string(&session_line)?)?;
        writeln!(file, "{}", serde_json::to_string(&turn_line)?)?;
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
    // Hand-written rollouts are not in the SQLite index yet; startup indexing
    // is a background step, so rebuild it explicitly for the fixture.
    runtime.refresh_session_index()?;
    runtime.load_persisted_sessions().await?;
    let (connection_id, _notifications_rx) = initialize_connection(&runtime).await?;

    for session_id in [&missing_thinking_session, &default_thinking_session] {
        let resume_result = resume_session(&runtime, connection_id, 34, session_id).await?;

        assert_eq!(resume_result.session.id.as_str(), (*session_id).to_string());
        // The persisted binding must survive resume as the resolved model —
        // not the runtime default (`test/test-model`). Catalog slugs are
        // provider-qualified.
        assert_eq!(resume_result.session.model.model, "test/deepseek-v4-flash");
    }

    Ok(())
}

#[tokio::test]
async fn failed_turn_resume_restores_terminal_history_without_prompt_contamination() -> Result<()> {
    let data_root = TempDir::new()?;
    let session_id = SessionId::new();
    let failed_turn_id = devo_protocol::TurnId::new();
    let completed_turn_id = devo_protocol::TurnId::new();
    let now = chrono::Utc::now();
    let rollout_dir = data_root.path().join("sessions");
    std::fs::create_dir_all(&rollout_dir)?;
    let rollout_path = rollout_dir.join(format!("{session_id}.jsonl"));
    let terminal_error = TurnError {
        code: "PROVIDER_SERVER_ERROR".to_string(),
        message: "exact persisted provider failure".to_string(),
        recovery_hint: None,
    };
    let session = SessionRecord {
        id: session_id,
        rollout_path: rollout_path.clone(),
        created_at: now,
        updated_at: now + chrono::Duration::seconds(5),
        last_activity_at: Some(now + chrono::Duration::seconds(5)),
        source: "cli".into(),
        agent_nickname: None,
        agent_role: None,
        agent_path: None,
        model_provider: "test-provider".into(),
        model: Some("test-model".into()),
        model_binding_id: None,
        reasoning_effort_selection: None,
        cwd: data_root.path().to_path_buf(),
        additional_directories: Vec::new(),
        cli_version: "0.1.0".into(),
        title: Some("Failed turn resume".into()),
        title_state: devo_core::SessionTitleState::Final(
            devo_core::SessionTitleFinalSource::ExplicitCreate,
        ),
        sandbox_policy: "workspace-write".into(),
        approval_mode: "on-request".into(),
        effective_context_window: None,
        tokens_used: 0,
        first_user_message: Some("failing prompt".into()),
        archived_at: None,
        git_sha: None,
        git_branch: None,
        git_origin_url: None,
        parent_session_id: None,
        fork_from_id: None,
        fork_at_turn_id: None,
        session_context: None,
        latest_turn_context: None,
        collaboration_mode: None,
        permission_preset: None,
        schema_version: 2,
    };
    let failed_running = TurnRecord {
        id: failed_turn_id,
        session_id,
        sequence: 1,
        started_at: now,
        completed_at: None,
        status: TurnStatus::Running,
        kind: devo_core::TurnKind::Regular,
        model: "test-model".into(),
        model_binding_id: None,
        reasoning_effort_selection: None,
        request_model: "test-model".into(),
        request_thinking: None,
        input_token_estimate: None,
        usage: None,
        latest_query_usage: None,
        context_occupancy: None,
        stop_reason: None,
        failure_reason: None,
        error: None,
        session_context: None,
        turn_context: None,
        schema_version: 4,
    };
    let failed_terminal = TurnRecord {
        completed_at: Some(now + chrono::Duration::seconds(2)),
        status: TurnStatus::Failed,
        error: Some(terminal_error.clone()),
        ..failed_running.clone()
    };
    let completed_running = TurnRecord {
        id: completed_turn_id,
        sequence: 2,
        started_at: now + chrono::Duration::seconds(3),
        ..failed_running.clone()
    };
    let completed_terminal = TurnRecord {
        completed_at: Some(now + chrono::Duration::seconds(5)),
        status: TurnStatus::Completed,
        ..completed_running.clone()
    };
    use devo_core::RolloutLine;
    use devo_protocol::native::item::Item;
    use devo_protocol::native::item::ItemEnvelope;
    use devo_protocol::native::item::ItemState;
    use devo_protocol::native::item::UserInput;

    let item_line = |turn_id: devo_protocol::TurnId,
                     seq: u64,
                     timestamp: chrono::DateTime<chrono::Utc>,
                     item: Item| {
        RolloutLine::Item {
            v: devo_core::ROLLOUT_FORMAT_VERSION,
            timestamp,
            item: ItemEnvelope {
                id: devo_core::ItemId::new(),
                session_id,
                turn_id,
                seq,
                revision: 1,
                created_at: timestamp,
                updated_at: timestamp,
                state: ItemState::Completed,
                item,
                parent_id: None,
            },
        }
    };
    let user_message = |text: &str| Item::UserMessage {
        client_user_message_id: None,
        content: vec![UserInput::Text {
            text: text.to_string(),
        }],
        entry: Default::default(),
    };
    // The rollout indexer and resume replay parse only v2 lines, so the
    // fixture writes the v2 envelope through the same record converters the
    // live append path uses.
    let rollout_lines = vec![
        devo_core::session_line_from_record(&session, now)?,
        devo_core::turn_line_from_record(&failed_running, now)?,
        item_line(failed_turn_id, 1, now, user_message("failing prompt")),
        item_line(
            failed_turn_id,
            2,
            now + chrono::Duration::seconds(1),
            Item::AssistantMessage {
                text: "partial response".to_string(),
            },
        ),
        devo_core::turn_line_from_record(&failed_terminal, now + chrono::Duration::seconds(2))?,
        devo_core::turn_line_from_record(&completed_running, now + chrono::Duration::seconds(3))?,
        item_line(
            completed_turn_id,
            3,
            now + chrono::Duration::seconds(3),
            user_message("next prompt"),
        ),
        item_line(
            completed_turn_id,
            4,
            now + chrono::Duration::seconds(4),
            Item::AssistantMessage {
                text: "next response".to_string(),
            },
        ),
        devo_core::turn_line_from_record(&completed_terminal, now + chrono::Duration::seconds(5))?,
    ];
    let mut file = std::fs::File::create(&rollout_path)?;
    for line in rollout_lines {
        writeln!(file, "{}", serde_json::to_string(&line)?)?;
    }

    let provider = Arc::new(CapturingProvider::default());
    let runtime = build_runtime_with_provider(data_root.path(), provider.clone())?;
    // Hand-written rollout needs an explicit index rebuild (background at startup).
    runtime.refresh_session_index()?;
    runtime.load_persisted_sessions().await?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let _resume = resume_session(&runtime, connection_id, 35, &session_id).await?;
    let items = list_native_items(&runtime, connection_id, session_id.to_string()).await?;
    assert!(!items.is_empty());

    start_turn_and_wait(
        &runtime,
        connection_id,
        36,
        &session_id,
        "continue after failure",
        &mut notifications_rx,
    )
    .await?;
    let requests = provider.requests.lock().expect("lock requests");
    let request_json = serde_json::to_string(requests.last().context("captured model request")?)?;
    assert!(!request_json.contains(&terminal_error.code));
    assert!(!request_json.contains(&terminal_error.message));

    Ok(())
}

#[tokio::test]
async fn runtime_recovers_session_when_middle_rollout_line_is_corrupted() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let session = create_native_session(
        &runtime,
        connection_id,
        41,
        data_root.path(),
        "persistence-recoverable-session",
        Some("Recoverable session"),
        None,
    )
    .await?;
    let session_id = session.id;

    start_turn_and_wait(
        &runtime,
        connection_id,
        42,
        &session_id,
        "persist this session before corruption",
        &mut notifications_rx,
    )
    .await?;

    let sessions_root = data_root.path().join("sessions");
    // Root rollouts live flat at `sessions/<session_id>.jsonl` — the older
    // year/month/day partition layout is gone.
    let rollout_path = sessions_root.join(format!("{session_id}.jsonl"));
    assert!(
        rollout_path.is_file(),
        "expected rollout at {}",
        rollout_path.display()
    );

    let mut lines = std::fs::read_to_string(&rollout_path)?
        .lines()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    assert!(lines.len() >= 4);
    lines[2] = "{\"Turn\":{\"timestamp\":\"broken\"".to_string();
    std::fs::write(&rollout_path, format!("{}\n", lines.join("\n")))?;

    // Fail closed (05 §2.2): a damaged mid-file line marks the session
    // damaged — it is skipped at load and refuses to resume, rather than
    // silently dropping the history after the damage.
    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (rebuilt_connection_id, _notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;

    let resume_response = rpc(
        &rebuilt_runtime,
        rebuilt_connection_id,
        43,
        "session/resume",
        serde_json::json!({ "sessionId": session_id }),
    )
    .await?;
    assert!(
        resume_response.get("error").is_some(),
        "damaged session must refuse resume: {resume_response}"
    );
    Ok(())
}

#[tokio::test]
async fn session_compact_runs_asynchronously_and_emits_lifecycle_events() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let session = create_native_session(
        &runtime,
        connection_id,
        51,
        data_root.path(),
        "persistence-compaction-session",
        Some("Compaction session"),
        None,
    )
    .await?;
    let session_id = session.id;

    start_turn_and_wait(
        &runtime,
        connection_id,
        52,
        &session_id,
        "create some history first",
        &mut notifications_rx,
    )
    .await?;

    let compact_response = compact_session(&runtime, connection_id, 53, &session_id).await?;
    let compact_result: devo_server::SuccessResponse<
        devo_protocol::native::rpc_turn::TurnStartResult,
    > = serde_json::from_value(compact_response)?;
    assert_eq!(
        compact_result.result.turn.session_id.as_str(),
        session_id.to_string()
    );

    wait_for_notification_method(&mut notifications_rx, "turn/started").await?;
    wait_for_notification_method(&mut notifications_rx, "context/compactionStarted").await?;
    wait_for_notification_method(&mut notifications_rx, "context/compactionCompleted").await?;
    Ok(())
}

#[tokio::test]
async fn compacted_session_resume_keeps_full_transcript_after_restart() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let session = create_native_session(
        &runtime,
        connection_id,
        61,
        data_root.path(),
        "persistence-compacted-session",
        Some("Persist compacted session"),
        None,
    )
    .await?;
    let session_id = session.id;

    for request_id in 0..3 {
        let large_prompt = "x".repeat(30_000);
        start_turn_and_wait(
            &runtime,
            connection_id,
            62 + request_id,
            &session_id,
            &large_prompt,
            &mut notifications_rx,
        )
        .await?;
    }

    let _ = compact_session(&runtime, connection_id, 70, &session_id).await?;
    wait_for_notification_method(&mut notifications_rx, "context/compactionCompleted").await?;

    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (rebuilt_connection_id, _notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;

    let _resume_result =
        resume_session(&rebuilt_runtime, rebuilt_connection_id, 71, &session_id).await?;

    let items = list_native_items(
        &rebuilt_runtime,
        rebuilt_connection_id,
        session_id.to_string(),
    )
    .await?;
    let items_json = serde_json::to_string(&items)?;
    assert!(
        items.len() >= 6,
        "expected full transcript to survive compaction"
    );
    assert!(items_json.contains("contextCompaction"));
    assert!(items_json.contains("Hello from persistence test."));
    Ok(())
}

#[tokio::test]
async fn compacted_session_next_query_uses_compaction_summary_after_restart() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let session = create_native_session(
        &runtime,
        connection_id,
        81,
        data_root.path(),
        "persistence-prompt-snapshot-session",
        Some("Prompt snapshot session"),
        None,
    )
    .await?;
    let session_id = session.id;

    // Distinct payloads per turn: the compaction snapshot preserves the last
    // complete turn verbatim, so only the earlier turns are guaranteed to be
    // compacted away — the assertions below key on "a"/"b", not "c".
    let large_prompts = ["a".repeat(30_000), "b".repeat(30_000), "c".repeat(30_000)];
    for (offset, large_prompt) in large_prompts.iter().enumerate() {
        start_turn_and_wait(
            &runtime,
            connection_id,
            u64::try_from(82 + offset).expect("request id fits"),
            &session_id,
            large_prompt,
            &mut notifications_rx,
        )
        .await?;
    }

    let _ = compact_session(&runtime, connection_id, 90, &session_id).await?;
    wait_for_notification_method(&mut notifications_rx, "context/compactionCompleted").await?;
    // The compaction turn is still finalizing when compactionCompleted fires;
    // wait for its terminal turn/completed before starting the next turn.
    wait_for_turn_completed(&mut notifications_rx).await?;

    // Turns after the compaction are the shape every real resumed session
    // has: summary + suffix. The restart below must keep that shape instead
    // of reverting to the full linear history.
    for offset in 0..2 {
        start_turn_and_wait(
            &runtime,
            connection_id,
            92 + offset,
            &session_id,
            &format!("after compaction {offset}"),
            &mut notifications_rx,
        )
        .await?;
    }

    let capturing_provider = Arc::new(CapturingProvider::default());
    let rebuilt_runtime =
        build_runtime_with_provider(data_root.path(), capturing_provider.clone())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (rebuilt_connection_id, mut rebuilt_notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;
    resume_session(&rebuilt_runtime, rebuilt_connection_id, 94, &session_id).await?;
    start_turn_and_wait(
        &rebuilt_runtime,
        rebuilt_connection_id,
        95,
        &session_id,
        "go on",
        &mut rebuilt_notifications_rx,
    )
    .await?;

    let requests = capturing_provider.requests.lock().expect("lock requests");
    let request = requests
        .last()
        .context("expected captured model request after restart")?;
    let serialized = serde_json::to_string(request)?;

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
                | devo_protocol::RequestContent::ToolResult { .. }
                | devo_protocol::RequestContent::Image { .. } => false,
            })
        }),
        "expected prompt request to include compaction summary after restart"
    );
    assert!(
        !serialized.contains(&"a".repeat(2_000)) && !serialized.contains(&"b".repeat(2_000)),
        "path-scoped resume rebuilt the full linear prompt: compacted-away \
         large-turn payloads must stay out of the post-restart context"
    );
    Ok(())
}

/// A tree navigate that keeps the compaction summary on the active branch
/// must keep the compacted prompt; rewinding past the summary legitimately
/// falls back to the linear scoped history (the designed degradation).
#[tokio::test]
async fn tree_navigate_keeps_compaction_summary_on_branch() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(CapturingProvider::default());
    let runtime = build_runtime_with_provider(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session = create_native_session(
        &runtime,
        connection_id,
        100,
        data_root.path(),
        "tree-navigate-compaction",
        Some("Tree navigate compaction"),
        None,
    )
    .await?;
    let session_id = session.id;

    start_turn_and_wait(
        &runtime,
        connection_id,
        101,
        &session_id,
        &"x".repeat(30_000),
        &mut notifications_rx,
    )
    .await?;
    start_turn_and_wait(
        &runtime,
        connection_id,
        102,
        &session_id,
        &"y".repeat(30_000),
        &mut notifications_rx,
    )
    .await?;

    let _ = compact_session(&runtime, connection_id, 103, &session_id).await?;
    wait_for_notification_method(&mut notifications_rx, "context/compactionCompleted").await?;
    wait_for_turn_completed(&mut notifications_rx).await?;
    start_turn_and_wait(
        &runtime,
        connection_id,
        104,
        &session_id,
        "post-compaction turn",
        &mut notifications_rx,
    )
    .await?;

    let items = list_native_items(&runtime, connection_id, session_id.to_string()).await?;
    let first_reply = items
        .iter()
        .find(|envelope| {
            matches!(
                &envelope.item,
                devo_protocol::native::item::Item::AssistantMessage { .. }
            )
        })
        .context("first assistant reply")?
        .clone();

    // Navigate back onto the compaction summary itself: the branch keeps the
    // summary as its tip, so the next turn must run the compacted prompt.
    let summary_item = items
        .iter()
        .find(|envelope| {
            matches!(
                &envelope.item,
                devo_protocol::native::item::Item::ContextCompaction { .. }
            )
        })
        .context("compaction summary item")?
        .clone();
    rpc(
        &runtime,
        connection_id,
        105,
        "session/tree/navigate",
        serde_json::json!({ "sessionId": session_id, "entryId": summary_item.id }),
    )
    .await?;
    start_turn_and_wait(
        &runtime,
        connection_id,
        106,
        &session_id,
        "what was compacted?",
        &mut notifications_rx,
    )
    .await?;
    {
        let requests = provider.requests.lock().expect("lock requests");
        let last = serde_json::to_string(requests.last().expect("captured request"))?;
        assert!(
            last.contains("<compaction_summary>"),
            "navigate onto the summary must keep the compacted prompt"
        );
        assert!(
            !last.contains(&"x".repeat(2_000)),
            "navigate onto the summary reverted the prompt to full history"
        );
    }

    // Rewind past the summary: the linear scoped history is the correct
    // fallback there (the summary is off-branch).
    rpc(
        &runtime,
        connection_id,
        107,
        "session/tree/navigate",
        serde_json::json!({ "sessionId": session_id, "entryId": first_reply.id }),
    )
    .await?;
    start_turn_and_wait(
        &runtime,
        connection_id,
        108,
        &session_id,
        "which letters?",
        &mut notifications_rx,
    )
    .await?;
    {
        let requests = provider.requests.lock().expect("lock requests");
        let last = serde_json::to_string(requests.last().expect("captured request"))?;
        assert!(
            last.contains(&"x".repeat(2_000)),
            "rewind past the summary must fall back to the linear scoped history"
        );
    }
    Ok(())
}

/// High-usage streaming provider that also captures requests and returns a
/// compaction summary from the non-streaming completion path.
struct AutoCompactTestProvider {
    requests: Mutex<Vec<ModelRequest>>,
    input_tokens: usize,
    output_tokens: usize,
}

impl AutoCompactTestProvider {
    fn new(input_tokens: usize, output_tokens: usize) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            input_tokens,
            output_tokens,
        }
    }

    fn usage(&self) -> Usage {
        Usage {
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: Some(self.input_tokens / 2),
            reasoning_output_tokens: None,
            total_tokens: Some(self.input_tokens + self.output_tokens),
        }
    }
}

#[async_trait]
impl ModelProviderSDK for AutoCompactTestProvider {
    async fn completion(&self, request: ModelRequest) -> Result<ModelResponse> {
        self.requests.lock().expect("lock requests").push(request);
        Ok(text_response(
            "compact-summary",
            "auto compact summary for resume",
            Usage::default(),
        ))
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        self.requests.lock().expect("lock requests").push(request);
        let usage = self.usage();
        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::UsageDelta(usage.clone())),
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: "Hello from auto compact test.".into(),
            }),
            Ok(StreamEvent::MessageDone {
                response: text_response(
                    "resp-auto-compact",
                    "Hello from auto compact test.",
                    usage,
                ),
            }),
        ])))
    }

    fn name(&self) -> &str {
        "auto-compact-test-provider"
    }
}

#[tokio::test]
async fn auto_compaction_persists_snapshot_and_survives_resume() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(AutoCompactTestProvider::new(
        /*input_tokens*/ 96_000, /*output_tokens*/ 1_000,
    ));
    let runtime = build_runtime_with_provider(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let session = create_native_session(
        &runtime,
        connection_id,
        201,
        data_root.path(),
        "persistence-auto-compact-session",
        Some("Auto compact persist session"),
        None,
    )
    .await?;
    let session_id = session.id;

    let _ = metadata_update(
        &runtime,
        connection_id,
        202,
        &session_id,
        serde_json::json!({ "settings": { "effectiveContextWindow": 5_000 } }),
    )
    .await?;

    // Build enough oversized history that Auto preserve budget cannot keep it all.
    for request_id in 0..3 {
        let large_prompt = "x".repeat(100_000);
        start_turn_and_wait(
            &runtime,
            connection_id,
            210 + request_id,
            &session_id,
            &large_prompt,
            &mut notifications_rx,
        )
        .await?;
    }

    let capturing_provider = Arc::new(CapturingProvider::default());
    let rebuilt_runtime =
        build_runtime_with_provider(data_root.path(), capturing_provider.clone())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (rebuilt_connection_id, mut rebuilt_notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;
    let _resume_result =
        resume_session(&rebuilt_runtime, rebuilt_connection_id, 220, &session_id).await?;

    let items = list_native_items(
        &rebuilt_runtime,
        rebuilt_connection_id,
        session_id.to_string(),
    )
    .await?;
    let items_json = serde_json::to_string(&items)?;
    assert!(items_json.contains("contextCompaction"));
    // Occupancy deltas are recorded on the compaction items so tree badges
    // render real numbers instead of unmeasured zeros.
    assert!(items_json.contains("\"measured\":true"));

    start_turn_and_wait(
        &rebuilt_runtime,
        rebuilt_connection_id,
        221,
        &session_id,
        "continue",
        &mut rebuilt_notifications_rx,
    )
    .await?;

    let requests = capturing_provider.requests.lock().expect("lock requests");
    let request = requests
        .last()
        .context("expected captured model request after auto-compact resume")?;
    assert!(
        request.messages.iter().any(|message| {
            message.content.iter().any(|content| match content {
                devo_protocol::RequestContent::Text { text }
                | devo_protocol::RequestContent::Reasoning { text } => {
                    text.contains("<compaction_summary>") || text.contains("auto compact summary")
                }
                devo_protocol::RequestContent::ProviderReasoning { .. }
                | devo_protocol::RequestContent::ToolUse { .. }
                | devo_protocol::RequestContent::HostedToolUse { .. }
                | devo_protocol::RequestContent::ToolResult { .. }
                | devo_protocol::RequestContent::Image { .. } => false,
            })
        }),
        "expected post-resume prompt to use compacted history, got {:?}",
        request.messages
    );
    Ok(())
}

#[tokio::test]
async fn configured_request_model_is_used_for_turn_metadata_and_provider_request() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(CapturingProvider::default());
    let runtime = build_runtime_with_provider(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let session = create_native_session(
        &runtime,
        connection_id,
        101,
        data_root.path(),
        "persistence-request-model-session",
        Some("Request model session"),
        Some("test/test-model"),
    )
    .await?;
    let session_id = session.id;

    let reasoning_response = metadata_update(
        &runtime,
        connection_id,
        103,
        &session_id,
        serde_json::json!({ "settings": { "reasoningEffort": "on" } }),
    )
    .await?;
    assert!(
        reasoning_response.get("error").is_none(),
        "{reasoning_response}"
    );

    start_turn_with(
        &runtime,
        connection_id,
        102,
        &session_id,
        "use configured request model",
        serde_json::json!({}),
    )
    .await?;
    let turn_started = wait_for_notification_value(&mut notifications_rx, "turn/started").await?;
    wait_for_turn_completed(&mut notifications_rx).await?;

    assert_eq!(
        turn_started["params"]["turn"]["model"],
        serde_json::json!({
            "provider": "test/test-model",
            "model": "vendor/test-model",
            // Live turns record the resolved reasoning effort on the binding
            // (followup.rs): selection "on" normalizes to the model's
            // non-off level ("high" for this toggle catalog).
            "reasoningEffort": "high",
        })
    );
    let requests = provider.requests.lock().expect("lock requests");
    assert_eq!(
        requests.last().expect("captured request").model,
        "vendor/test-model"
    );

    Ok(())
}

async fn rpc(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    id: u64,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({ "id": id, "method": method, "params": params }),
        )
        .await
        .with_context(|| format!("{method} response"))
}

async fn start_turn_with(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
    session_id: &devo_protocol::native::ids::SessionId,
    text: &str,
    extra: serde_json::Value,
) -> Result<serde_json::Value> {
    let mut params = serde_json::json!({
        "sessionId": session_id,
        "input": [{ "type": "text", "text": text }],
        "idempotencyKey": format!("persistence-resume-turn-{}", uuid::Uuid::new_v4()),
        "model": null,
        "sandbox": null,
        "approval_policy": null,
        "cwd": null
    });
    if let Some(extra) = extra.as_object() {
        for (key, value) in extra {
            params[key] = value.clone();
        }
    }
    rpc(runtime, connection_id, request_id, "turn/start", params).await
}

async fn start_turn_and_wait(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
    session_id: &devo_protocol::native::ids::SessionId,
    text: &str,
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
) -> Result<()> {
    start_turn_and_wait_with(
        runtime,
        connection_id,
        request_id,
        session_id,
        text,
        serde_json::json!({}),
        notifications_rx,
    )
    .await
}

async fn start_turn_and_wait_with(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
    session_id: &devo_protocol::native::ids::SessionId,
    text: &str,
    extra: serde_json::Value,
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
) -> Result<()> {
    let response =
        start_turn_with(runtime, connection_id, request_id, session_id, text, extra).await?;
    let _: devo_server::SuccessResponse<devo_protocol::native::rpc_turn::TurnStartResult> =
        serde_json::from_value(response)?;
    wait_for_turn_completed(notifications_rx).await
}

async fn metadata_update(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
    session_id: &devo_protocol::native::ids::SessionId,
    extra: serde_json::Value,
) -> Result<serde_json::Value> {
    let mut params = serde_json::json!({
        "sessionId": session_id,
        "expectedVersion": 0,
    });
    if let Some(extra) = extra.as_object() {
        for (key, value) in extra {
            params[key] = value.clone();
        }
    }
    rpc(
        runtime,
        connection_id,
        request_id,
        "session/metadata/update",
        params,
    )
    .await
}

async fn list_sessions(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
) -> Result<Vec<devo_protocol::native::session::Session>> {
    decode_native_session_list_response(
        rpc(
            runtime,
            connection_id,
            request_id,
            "session/list",
            serde_json::json!({}),
        )
        .await?,
    )
}

async fn compact_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
    session_id: &devo_protocol::native::ids::SessionId,
) -> Result<serde_json::Value> {
    rpc(
        runtime,
        connection_id,
        request_id,
        "session/compact/start",
        serde_json::json!({ "sessionId": session_id }),
    )
    .await
}

async fn resume_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
    session_id: &devo_protocol::native::ids::SessionId,
) -> Result<devo_protocol::native::rpc_session::SessionResumeResult> {
    let response = rpc(
        runtime,
        connection_id,
        request_id,
        "session/resume",
        serde_json::json!({ "sessionId": session_id }),
    )
    .await?;
    let parsed = serde_json::from_value::<
        devo_server::SuccessResponse<devo_protocol::native::rpc_session::SessionResumeResult>,
    >(response.clone())
    .map_err(|error| {
        anyhow::anyhow!("session/resume response: {response} — decode error: {error}")
    })?;
    Ok(parsed.result)
}

async fn create_native_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
    cwd: &std::path::Path,
    idempotency_key: &str,
    title: Option<&str>,
    model: Option<&str>,
) -> Result<devo_protocol::native::session::Session> {
    let response = rpc(
        runtime,
        connection_id,
        request_id,
        "session/new",
        serde_json::json!({ "cwd": cwd, "idempotencyKey": idempotency_key }),
    )
    .await?;
    let response: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionNewResult,
    > = serde_json::from_value(response)?;
    let mut metadata = serde_json::json!({
        "sessionId": response.result.session.id,
        "expectedVersion": 0
    });
    if let Some(title) = title {
        metadata["title"] = serde_json::json!(title);
    }
    if let Some(model) = model {
        metadata["model"] = serde_json::json!({ "provider": "", "model": model });
    }
    if title.is_some() || model.is_some() {
        let response = rpc(
            runtime,
            connection_id,
            request_id + 1,
            "session/metadata/update",
            metadata,
        )
        .await?;
        let response: devo_server::SuccessResponse<
            devo_protocol::native::rpc_session::SessionMetadataUpdateResult,
        > = serde_json::from_value(response)?;
        return Ok(response.result.session);
    }
    Ok(response.result.session)
}

fn build_runtime(data_root: &std::path::Path) -> Result<Arc<ServerRuntime>> {
    build_runtime_with_provider(data_root, Arc::new(SingleReplyProvider))
}

fn ensure_test_provider_catalog(data_root: &std::path::Path) -> Result<()> {
    let providers_path = data_root.join("providers.json");
    if providers_path.exists() {
        return Ok(());
    }
    std::fs::write(
        providers_path,
        r#"{
  "model": "test/test-model",
  "providers": {
    "test": {
      "name": "Test",
      "wire_api": "openai_chat_completions",
      "models": {
        "test-model": {
          "name": "test-model",
          "context_window": 100000,
          "effective_context_window_percent": 95.0,
          "reasoning_capability": "toggle",
          "reasoning_implementation": {
            "model_variant": {
              "variants": [
                {
                  "selection_value": "disabled",
                  "model": "test-model",
                  "label": "Off",
                  "description": "Disable reasoning effort"
                },
                {
                  "selection_value": "enabled",
                  "model": "vendor/test-model",
                  "reasoning_effort": "medium",
                  "label": "On",
                  "description": "Enable reasoning effort"
                }
              ]
            }
          },
          "base_instructions": "Test model"
        },
        "deepseek-v4-flash": {
          "name": "deepseek-v4-flash",
          "context_window": 100000,
          "reasoning_capability": { "levels": ["off", "high", "max"] },
          "default_reasoning_effort": "high",
          "base_instructions": "Flash model"
        }
      }
    }
  }
}"#,
    )?;
    Ok(())
}

fn build_runtime_with_provider(
    data_root: &std::path::Path,
    provider: Arc<dyn ModelProviderSDK>,
) -> Result<Arc<ServerRuntime>> {
    ensure_test_provider_catalog(data_root)?;
    let config_store = AppConfigStore::load(data_root.to_path_buf(), None)?;
    let model_catalog = Arc::new(
        PresetModelCatalog::load_from_provider_config_with_overrides(
            &config_store.effective_config().provider_catalog_config(),
            &config_store.effective_config().provider.model_overrides,
        )?,
    );
    let default_model = model_catalog
        .resolve_for_turn(Some("test/test-model"))
        .map(|model| model.slug.clone())
        .unwrap_or_else(|_| "test/test-model".to_string());
    Ok(TestRuntime::new(provider)
        .default_model(default_model)
        .catalog(model_catalog)
        .config_store(Arc::new(std::sync::Mutex::new(config_store)))
        .db_file("test_persistence.db")
        .runtime(data_root))
}

async fn initialize_connection(
    runtime: &Arc<ServerRuntime>,
) -> Result<(u64, mpsc::Receiver<serde_json::Value>)> {
    let (notifications_tx, notifications_rx) = devo_server::test_outbound_channel(4096);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, notifications_tx)
        .await;
    let initialize_response = rpc(
        runtime,
        connection_id,
        10,
        "initialize",
        serde_json::json!({
            "protocolVersion": 1,
            "clientCapabilities": {},
            "_meta": { "devo": { "protocol": "native" } },
            "clientInfo": { "name": "test", "title": "test", "version": "1.0.0" }
        }),
    )
    .await?;
    let response: serde_json::Value = initialize_response;
    assert_eq!(
        response["result"]["agentInfo"]["name"],
        serde_json::json!("devo-server")
    );
    Ok((connection_id, notifications_rx))
}

fn decode_native_session_list_response(
    response: serde_json::Value,
) -> Result<Vec<devo_protocol::native::session::Session>> {
    let response: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionListResult,
    > = serde_json::from_value(response)?;
    Ok(response.result.data)
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
    let Some((method, params)) = devo_protocol::original_notification_wire_from_acp(&notification)
    else {
        return value;
    };
    serde_json::json!({
        "method": method,
        "params": params,
    })
}

fn title_from_notification(value: &serde_json::Value) -> Option<&str> {
    if value.get("method") == Some(&serde_json::json!("session/metadataUpdated")) {
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
    // These turns include real Python startup, namespace snapshots and automatic
    // compaction while the persistence cases run concurrently. Five seconds is
    // too short on shared CI runners; this remains a bounded lifecycle wait.
    timeout(Duration::from_secs(30), async {
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

async fn wait_for_turn_usage_updated(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
) -> Result<devo_protocol::native::usage::UsageTotals> {
    timeout(Duration::from_secs(5), async {
        while let Some(value) = notifications_rx.recv().await {
            if value.get("method") == Some(&serde_json::json!("turn/usage/updated")) {
                return serde_json::from_value(value["params"]["sessionTotals"].clone())
                    .context("decode Native turn/usage/updated session totals");
            }
        }
        anyhow::bail!("notification channel closed before turn/usage/updated")
    })
    .await
    .context("timed out waiting for turn/usage/updated")?
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

    let session = create_native_session(
        &runtime,
        connection_id,
        1,
        data_root.path(),
        "persistence-interrupt-session",
        None,
        None,
    )
    .await?;
    let session_id = session.id;

    let _ = start_turn_with(
        &runtime,
        connection_id,
        2,
        &session_id,
        "interrupt me",
        serde_json::json!({}),
    )
    .await?;

    // Wait until the assistant item has started streaming.  The provider yields
    // one TextDelta, then blocks, so once we see the delta notification we know
    // deferred_assistant has been stored in the session.
    wait_for_notification_method(&mut notifications_rx, "item/assistantMessage/delta").await?;

    // Now interrupt the turn while it is still in-progress.
    let interrupt_response = rpc(
        &runtime,
        connection_id,
        3,
        "session/interrupt",
        serde_json::json!({
            "scope": { "scope": "session", "sessionId": session_id }
        }),
    )
    .await?;
    let interrupt_result: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionInterruptResult,
    > = serde_json::from_value(interrupt_response)?;
    assert!(interrupt_result.result.interrupted);

    // Native projects every terminal turn state through turn/completed.
    wait_for_notification_method(&mut notifications_rx, "turn/completed").await?;

    // Rebuild runtime (simulates restart) and resume the session.
    let gated2 = Arc::new(GatedProvider::new());
    let rebuilt = build_runtime_with_provider(data_root.path(), Arc::clone(&gated2) as _)?;
    rebuilt.load_persisted_sessions().await?;
    let (rebuilt_cid, _) = initialize_connection(&rebuilt).await?;

    let _resume_result = resume_session(&rebuilt, rebuilt_cid, 4, &session_id).await?;

    let items = list_native_items(&rebuilt, rebuilt_cid, session_id.to_string()).await?;
    let item_json = serde_json::to_string(&items)?;
    assert_eq!(
        items
            .iter()
            .filter(|item| {
                matches!(
                    &item.item,
                    devo_protocol::native::item::Item::UserMessage { .. }
                )
            })
            .count(),
        1,
        "expected exactly one User item"
    );
    assert!(item_json.contains("assistantMessage"));

    Ok(())
}

#[tokio::test]
async fn first_usage_update_after_resume_preserves_historical_session_totals() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(UsageReplyProvider::new(100, 25));
    let runtime = build_runtime_with_provider(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let session = create_native_session(
        &runtime,
        connection_id,
        1,
        data_root.path(),
        "persistence-usage-resume-session",
        Some("Usage resume base"),
        None,
    )
    .await?;
    let session_id = session.id;

    start_turn_with(
        &runtime,
        connection_id,
        2,
        &session_id,
        "persist usage totals",
        serde_json::json!({}),
    )
    .await?;
    let first_usage = wait_for_turn_usage_updated(&mut notifications_rx).await?;
    assert_eq!(first_usage.input_tokens, 100);
    wait_for_turn_completed(&mut notifications_rx).await?;

    let rebuilt_runtime = build_runtime_with_provider(data_root.path(), provider)?;
    rebuilt_runtime.refresh_session_index()?;
    let (rebuilt_connection_id, mut rebuilt_notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;
    let resumed_result =
        resume_session(&rebuilt_runtime, rebuilt_connection_id, 3, &session_id).await?;
    let _resumed = resumed_result.session;
    // Context length comes from the latest query snapshot, not cumulative totals.
    assert_eq!(resumed_result.last_query_total_tokens, Some(125));

    start_turn_with(
        &rebuilt_runtime,
        rebuilt_connection_id,
        4,
        &session_id,
        "post resume usage",
        serde_json::json!({}),
    )
    .await?;
    let post_resume_usage = wait_for_turn_usage_updated(&mut rebuilt_notifications_rx).await?;
    assert_eq!(post_resume_usage.input_tokens, 200);
    assert_eq!(post_resume_usage.output_tokens, 50);
    assert_eq!(post_resume_usage.cache_read_input_tokens, 100);
    Ok(())
}

#[tokio::test]
async fn rollout_writes_base_instructions_once_across_multiple_turns() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let session = create_native_session(
        &runtime,
        connection_id,
        1,
        data_root.path(),
        "persistence-rollout-dedupe-session",
        Some("Rollout dedupe"),
        None,
    )
    .await?;
    let session_id = session.id;

    for (request_id, prompt) in [(2, "first turn"), (3, "second turn")] {
        start_turn_and_wait(
            &runtime,
            connection_id,
            request_id,
            &session_id,
            prompt,
            &mut notifications_rx,
        )
        .await?;
    }

    runtime.refresh_session_index()?;
    let db = devo_server::db::Database::open(data_root.path().join("test_persistence.db"))?;
    let index = db
        .get_session_index(&SessionId::from(session_id.as_str()))?
        .expect("indexed session");
    let rollout_path = index.rollout_path.expect("rollout path");
    let rollout_lines = support::read_rollout_records(&rollout_path)?;
    let mut session_context_lines = 0usize;
    let mut turn_lines_with_session_context = 0usize;
    for rollout_line in rollout_lines {
        match rollout_line {
            LegacyRolloutLine::SessionContextUpdated(_) => session_context_lines += 1,
            LegacyRolloutLine::Turn(turn_line) if turn_line.turn.session_context.is_some() => {
                turn_lines_with_session_context += 1;
            }
            _ => {}
        }
    }
    assert_eq!(session_context_lines, 1);
    assert_eq!(turn_lines_with_session_context, 0);

    let rebuilt_runtime = build_runtime(data_root.path())?;
    let (rebuilt_connection_id, _) = initialize_connection(&rebuilt_runtime).await?;
    let _ = resume_session(&rebuilt_runtime, rebuilt_connection_id, 4, &session_id).await?;

    Ok(())
}

#[tokio::test]
async fn turn_start_persists_session_context_before_turn_completes() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let session = create_native_session(
        &runtime,
        connection_id,
        1,
        data_root.path(),
        "persistence-turn-start-crash-window-session",
        Some("Turn start crash window"),
        None,
    )
    .await?;
    let session_id = session.id;

    start_turn_with(
        &runtime,
        connection_id,
        2,
        &session_id,
        "crash before complete",
        serde_json::json!({}),
    )
    .await?;

    // Inspect the rollout before waiting for turn completion to simulate a crash
    // after durable turn-start persistence.
    runtime.refresh_session_index()?;
    let db = devo_server::db::Database::open(data_root.path().join("test_persistence.db"))?;
    let index = db
        .get_session_index(&SessionId::from(session_id.as_str()))?
        .expect("indexed session");
    let rollout_path = index.rollout_path.expect("rollout path");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let (session_context_lines, turn_lines) = loop {
        let rollout_lines = support::read_rollout_records(&rollout_path)?;
        let mut session_context_lines = 0usize;
        let mut turn_lines = 0usize;
        for rollout_line in rollout_lines {
            match rollout_line {
                LegacyRolloutLine::SessionContextUpdated(_) => session_context_lines += 1,
                LegacyRolloutLine::Turn(_) => turn_lines += 1,
                _ => {}
            }
        }
        if session_context_lines >= 1 && turn_lines >= 1 {
            break (session_context_lines, turn_lines);
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!(
                "timed out waiting for turn-start SessionContextUpdated; context_lines={session_context_lines} turn_lines={turn_lines}"
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(session_context_lines, 1);
    assert!(turn_lines >= 1);

    wait_for_turn_completed(&mut notifications_rx).await?;
    Ok(())
}

#[tokio::test]
async fn metadata_update_renames_cold_session_by_id_without_resume() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let session = create_native_session(
        &runtime,
        connection_id,
        1,
        data_root.path(),
        "persistence-cold-title-session",
        Some("Cold title"),
        None,
    )
    .await?;
    let session_id = session.id;
    start_turn_and_wait(
        &runtime,
        connection_id,
        2,
        &session_id,
        "persist the cold session",
        &mut notifications_rx,
    )
    .await?;
    drop(runtime);

    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.refresh_session_index()?;
    let (rebuilt_connection_id, _) = initialize_connection(&rebuilt_runtime).await?;
    let updated = serde_json::from_value::<
        devo_server::SuccessResponse<
            devo_protocol::native::rpc_session::SessionMetadataUpdateResult,
        >,
    >(
        metadata_update(
            &rebuilt_runtime,
            rebuilt_connection_id,
            3,
            &session_id,
            serde_json::json!({
                "model": { "provider": "", "model": "cold-model" },
                "settings": { "reasoningEffort": "high", "mode": "plan" }
            }),
        )
        .await?,
    )?
    .result;
    assert_eq!(updated.session.model.model, "cold-model");
    assert_eq!(
        updated.session.settings.reasoning_effort.as_deref(),
        Some("high")
    );
    assert_eq!(updated.session.settings.mode.as_deref(), Some("plan"));

    let renamed = serde_json::from_value::<
        devo_server::SuccessResponse<
            devo_protocol::native::rpc_session::SessionMetadataUpdateResult,
        >,
    >(
        metadata_update(
            &rebuilt_runtime,
            rebuilt_connection_id,
            4,
            &session_id,
            serde_json::json!({ "title": "Renamed while cold" }),
        )
        .await?,
    )?
    .result;
    assert_eq!(renamed.session.title.as_deref(), Some("Renamed while cold"));

    let sessions = list_sessions(&rebuilt_runtime, rebuilt_connection_id, 5).await?;
    let listed = sessions
        .iter()
        .find(|session| session.id.as_str() == session_id.to_string())
        .expect("renamed cold session remains listed");
    assert_eq!(listed.title.as_deref(), Some("Renamed while cold"));

    drop(rebuilt_runtime);
    let restarted_runtime = build_runtime(data_root.path())?;
    restarted_runtime.refresh_session_index()?;
    let (restarted_connection_id, _) = initialize_connection(&restarted_runtime).await?;
    let read = serde_json::from_value::<
        devo_server::SuccessResponse<devo_protocol::native::rpc_session::SessionReadResult>,
    >(
        rpc(
            &restarted_runtime,
            restarted_connection_id,
            6,
            "session/read",
            serde_json::json!({ "sessionId": session_id }),
        )
        .await?,
    )?
    .result;
    assert_eq!(read.session.model.model, "cold-model");
    assert_eq!(
        read.session.settings.reasoning_effort.as_deref(),
        Some("high")
    );
    assert_eq!(read.session.settings.mode.as_deref(), Some("plan"));
    Ok(())
}

#[tokio::test]
async fn lazy_resume_loads_parent_session_from_rollout_on_map_miss() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let session = create_native_session(
        &runtime,
        connection_id,
        1,
        data_root.path(),
        "persistence-lazy-resume-session",
        Some("Lazy resume session"),
        None,
    )
    .await?;
    let session_id = session.id;

    start_turn_and_wait(
        &runtime,
        connection_id,
        2,
        &session_id,
        "persist for lazy resume",
        &mut notifications_rx,
    )
    .await?;

    let db = devo_server::db::Database::open(data_root.path().join("test_persistence.db"))?;
    let rollout_path = db
        .get_session_index(&SessionId::from(session_id.as_str()))?
        .expect("indexed session")
        .rollout_path
        .expect("rollout path");
    let expected_transcript_size = std::fs::metadata(&rollout_path)?.len();
    let sessions = list_sessions(&runtime, connection_id, 20).await?;
    let listed = sessions
        .iter()
        .find(|session| session.id.as_str() == session_id.to_string())
        .expect("listed persisted session");
    assert_eq!(listed.transcript_size_bytes, Some(expected_transcript_size));

    let turns_response = rpc(
        &runtime,
        connection_id,
        21,
        "session/turns/list",
        serde_json::json!({ "sessionId": session_id }),
    )
    .await?;
    assert_eq!(
        turns_response["result"]["data"][0]["collaborationMode"],
        "build"
    );

    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.refresh_session_index()?;

    let (rebuilt_connection_id, _rebuilt_notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;
    let resume_result =
        resume_session(&rebuilt_runtime, rebuilt_connection_id, 3, &session_id).await?;
    assert_eq!(resume_result.session.id, session_id);
    Ok(())
}

#[tokio::test]
async fn lazy_resume_after_compat_backfill_without_refresh_session_index() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let session = create_native_session(
        &runtime,
        connection_id,
        1,
        data_root.path(),
        "persistence-lazy-resume-backfill-session",
        Some("Lazy resume backfill"),
        None,
    )
    .await?;
    let session_id = session.id;

    start_turn_and_wait(
        &runtime,
        connection_id,
        2,
        &session_id,
        "persist for compat backfill",
        &mut notifications_rx,
    )
    .await?;

    let db_path = data_root.path().join("test_persistence.db");
    {
        let conn = rusqlite::Connection::open(&db_path)?;
        conn.execute(
            "UPDATE sessions SET rollout_path = NULL WHERE id = ?1",
            rusqlite::params![session_id.to_string()],
        )?;
    }

    let rebuilt_runtime = build_runtime(data_root.path())?;
    assert!(rebuilt_runtime.backfill_session_index_if_required()?);

    let (rebuilt_connection_id, _) = initialize_connection(&rebuilt_runtime).await?;
    let resume_result =
        resume_session(&rebuilt_runtime, rebuilt_connection_id, 3, &session_id).await?;
    assert_eq!(resume_result.session.id, session_id);
    Ok(())
}

#[tokio::test]
async fn concurrent_lazy_resume_single_actor() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let session = create_native_session(
        &runtime,
        connection_id,
        1,
        data_root.path(),
        "persistence-concurrent-lazy-resume-session",
        Some("Concurrent lazy resume"),
        None,
    )
    .await?;
    let session_id = session.id;

    start_turn_and_wait(
        &runtime,
        connection_id,
        2,
        &session_id,
        "persist for concurrent resume",
        &mut notifications_rx,
    )
    .await?;

    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.refresh_session_index()?;
    let (connection_a, _) = initialize_connection(&rebuilt_runtime).await?;
    let (connection_b, _) = initialize_connection(&rebuilt_runtime).await?;

    let (result_a, result_b) = tokio::try_join!(
        resume_session(&rebuilt_runtime, connection_a, 3, &session_id),
        resume_session(&rebuilt_runtime, connection_b, 4, &session_id),
    )?;
    assert_eq!(result_a.session.id, session_id);
    assert_eq!(result_b.session.id, session_id);

    Ok(())
}

#[tokio::test]
async fn lazy_resume_subagent_requires_rollout_file() -> Result<()> {
    let data_root = TempDir::new()?;
    let parent_id = SessionId::new();
    let child_id = SessionId::new();
    let now = chrono::Utc::now();
    let db = devo_server::db::Database::open(data_root.path().join("test_persistence.db"))?;
    let mut parent = sample_indexed_session(parent_id, data_root.path(), now, None);
    parent.title = Some("Parent".into());
    db.upsert_session(&parent, Some("/tmp/parent.jsonl".as_ref()))?;
    let mut child = sample_indexed_session(child_id, data_root.path(), now, Some(parent_id));
    child.title = Some("Child".into());
    child.agent_path = Some("root/subagent".into());
    // Index points at a missing rollout — resume must fail for missing file, not
    // because the session is a subagent.
    db.upsert_session(&child, Some("/tmp/child-missing.jsonl".as_ref()))?;
    let runtime = build_runtime(data_root.path())?;

    let (connection_id, _) = initialize_connection(&runtime).await?;
    let error = serde_json::from_value::<devo_server::ErrorResponse>(
        rpc(
            &runtime,
            connection_id,
            1,
            "session/resume",
            serde_json::json!({ "sessionId": child_id }),
        )
        .await?,
    )?;
    assert!(
        error.error.message.to_lowercase().contains("rollout")
            || error.error.message.to_lowercase().contains("missing")
            || error.error.message.to_lowercase().contains("restore"),
        "expected missing-rollout failure, got: {}",
        error.error.message
    );
    assert!(
        !error
            .error
            .message
            .contains("subagent sessions cannot be resumed directly"),
        "subagent sessions must be resumable when a rollout exists"
    );
    Ok(())
}

#[tokio::test]
async fn lazy_resume_fails_when_rollout_file_is_missing() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let session = create_native_session(
        &runtime,
        connection_id,
        1,
        data_root.path(),
        "persistence-missing-rollout-session",
        Some("Missing rollout"),
        None,
    )
    .await?;
    let session_id = session.id;

    start_turn_and_wait(
        &runtime,
        connection_id,
        2,
        &session_id,
        "create rollout then delete it",
        &mut notifications_rx,
    )
    .await?;

    let db = devo_server::db::Database::open(data_root.path().join("test_persistence.db"))?;
    let index = db
        .get_session_index(&SessionId::from(session_id.as_str()))?
        .expect("indexed session");
    let rollout_path = index.rollout_path.expect("rollout path");
    std::fs::remove_file(&rollout_path)?;

    let rebuilt_runtime = build_runtime(data_root.path())?;
    let (rebuilt_connection_id, _) = initialize_connection(&rebuilt_runtime).await?;
    let sessions = list_sessions(&rebuilt_runtime, rebuilt_connection_id, 30).await?;
    let listed = sessions
        .iter()
        .find(|session| session.id.as_str() == session_id.to_string())
        .expect("missing-rollout session remains listed");
    assert_eq!(listed.transcript_size_bytes, None);

    let error = serde_json::from_value::<devo_server::ErrorResponse>(
        rpc(
            &rebuilt_runtime,
            rebuilt_connection_id,
            3,
            "session/resume",
            serde_json::json!({ "sessionId": session_id }),
        )
        .await?,
    )?;
    assert_eq!(
        error.error.code,
        devo_server::ProtocolErrorCode::InternalError
    );
    assert!(error.error.message.contains("rollout file is missing"));
    Ok(())
}

async fn list_native_items(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: String,
) -> Result<Vec<devo_protocol::native::item::ItemEnvelope>> {
    let response = rpc(
        runtime,
        connection_id,
        900,
        "session/items/list",
        serde_json::json!({ "sessionId": session_id }),
    )
    .await?;
    Ok(serde_json::from_value::<
        devo_protocol::native::page::Page<devo_protocol::native::item::ItemEnvelope>,
    >(response["result"].clone())?
    .data)
}

fn sample_indexed_session(
    session_id: SessionId,
    cwd: &std::path::Path,
    now: chrono::DateTime<chrono::Utc>,
    parent_session_id: Option<SessionId>,
) -> devo_server::db::SessionIndexRow {
    devo_server::db::SessionIndexRow {
        session_id,
        cwd: cwd.to_path_buf(),
        additional_directories: Vec::new(),
        created_at: now,
        updated_at: now,
        last_activity_at: now,
        title: None,
        title_state: devo_core::SessionTitleState::Unset,
        parent_session_id,
        fork_from_id: None,
        fork_at_turn_id: None,
        agent_path: None,
        ephemeral: false,
        model: Some("test-model".into()),
        reasoning_effort_selection: None,
    }
}

#[tokio::test]
async fn tree_navigate_scopes_next_turn_context_to_the_branch() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(CapturingProvider::default());
    let runtime = build_runtime_with_provider(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session = create_native_session(
        &runtime,
        connection_id,
        300,
        data_root.path(),
        "tree-navigate-scope",
        Some("Tree navigate scope"),
        None,
    )
    .await?;
    let session_id = session.id;

    start_turn_and_wait(
        &runtime,
        connection_id,
        301,
        &session_id,
        "Remember the number 4271.",
        &mut notifications_rx,
    )
    .await?;
    start_turn_and_wait(
        &runtime,
        connection_id,
        302,
        &session_id,
        "Remember the number secret-9918.",
        &mut notifications_rx,
    )
    .await?;

    let items = list_native_items(&runtime, connection_id, session_id.to_string()).await?;
    let item_text = |envelope: &devo_protocol::native::item::ItemEnvelope| match &envelope.item {
        devo_protocol::native::item::Item::UserMessage { content, .. } => content
            .iter()
            .filter_map(|part| match part {
                devo_protocol::native::item::UserInput::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>(),
        _ => String::new(),
    };
    let first_user = items
        .iter()
        .find(|envelope| item_text(envelope).contains("4271"))
        .expect("first user message");
    let first_reply = items
        .iter()
        .find(|envelope| {
            matches!(
                &envelope.item,
                devo_protocol::native::item::Item::AssistantMessage { .. }
            )
        })
        .expect("first assistant reply");

    // Navigate onto the first turn's reply (a non-user tip): the second
    // turn becomes an abandoned branch.
    rpc(
        &runtime,
        connection_id,
        303,
        "session/tree/navigate",
        serde_json::json!({ "sessionId": session_id, "entryId": first_reply.id }),
    )
    .await?;
    start_turn_and_wait(
        &runtime,
        connection_id,
        304,
        &session_id,
        "Which numbers do you remember? Answer in one line.",
        &mut notifications_rx,
    )
    .await?;
    {
        let requests = provider.requests.lock().expect("lock requests");
        let last = serde_json::to_string(requests.last().expect("captured request"))?;
        assert!(
            last.contains("4271"),
            "the kept branch must stay in the post-navigate context"
        );
        assert!(
            !last.contains("secret-9918"),
            "the abandoned branch leaked into the post-navigate model context"
        );
    }

    // Navigate before the first message (a user tip rewinds to its parent):
    // the whole journal is abandoned.
    rpc(
        &runtime,
        connection_id,
        305,
        "session/tree/navigate",
        serde_json::json!({ "sessionId": session_id, "entryId": first_user.id }),
    )
    .await?;
    start_turn_and_wait(
        &runtime,
        connection_id,
        306,
        &session_id,
        "Which numbers do you remember? Answer in one line.",
        &mut notifications_rx,
    )
    .await?;
    {
        let requests = provider.requests.lock().expect("lock requests");
        let last = serde_json::to_string(requests.last().expect("captured request"))?;
        assert!(
            !last.contains("secret-9918"),
            "abandoned branch leaked after a rewind before the first message"
        );
        assert!(
            !last.contains("4271"),
            "pre-branch rows leaked after a rewind before the first message"
        );
    }
    Ok(())
}
