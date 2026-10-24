//! Quota exhaustion is a terminal provider failure, including empty streams.
//! Exercise Native notifications and durable replay without any network calls.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

#[path = "support/rollout.rs"]
mod support;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_protocol::native::error::AgentError;
use devo_protocol::native::page::Page;
use devo_protocol::native::rpc_session::SessionMetadataUpdateResult;
use devo_protocol::native::rpc_session::SessionNewResult;
use devo_protocol::native::rpc_session::SessionResumeResult;
use devo_protocol::native::rpc_turn::TurnStartResult;
use devo_protocol::native::turn::Turn;
use devo_protocol::native::turn::TurnStatus;
use devo_provider::ProviderRoute;
use devo_provider::ProviderRouter;
use devo_provider::error::ProviderError;
use devo_server::ClientTransportKind;
use devo_server::ServerRuntime;
use devo_server::SuccessResponse;
use devo_server::test_support::TestRuntime;
use futures::Stream;
use futures::stream;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio::time::timeout;

const QUOTA_MESSAGE: &str =
    "Account quota exhausted: remaining credits = 0. 请充值后重试。 [request_id=quota-test]";

struct QuotaRouter {
    text_before_error: &'static str,
    attempts: AtomicUsize,
}

#[async_trait]
impl ProviderRouter for QuotaRouter {
    async fn stream(
        &self,
        _route: ProviderRoute,
        _request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>, ProviderError>
    {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        let mut events = Vec::new();
        if !self.text_before_error.is_empty() {
            events.push(Ok(StreamEvent::TextDelta {
                index: 0,
                text: self.text_before_error.to_string(),
            }));
        }
        // Fail inside the stream, not while opening it. No MessageDone follows.
        events.push(Err(ProviderError::QuotaExceededError {
            message: QUOTA_MESSAGE.to_string(),
            provider_name: Some("openai".to_string()),
        }
        .into()));
        Ok(Box::pin(stream::iter(events)))
    }

    async fn complete(
        &self,
        _route: ProviderRoute,
        _request: ModelRequest,
    ) -> Result<ModelResponse, ProviderError> {
        // Background title generation must stay local and must not count as a retry.
        Ok(ModelResponse {
            id: "quota-test-title".to_string(),
            content: vec![ResponseContent::Text("Quota test".to_string())],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: ResponseMetadata::default(),
        })
    }

    fn name(&self) -> &str {
        "quota-router"
    }
}

#[tokio::test]
async fn quota_failure_without_text_is_reported_once_and_survives_replay() -> Result<()> {
    assert_quota_failure("").await
}

#[tokio::test]
async fn quota_failure_after_partial_text_is_reported_once_and_survives_replay() -> Result<()> {
    assert_quota_failure("Partial answer before quota exhaustion.").await
}

async fn assert_quota_failure(text_before_error: &'static str) -> Result<()> {
    let data_root = TempDir::new()?;
    std::fs::write(
        data_root.path().join("auth.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "credentials": {
                "test_api_key": { "kind": "api_key", "value": "test-secret" }
            }
        }))?,
    )?;
    // Use a connection route so the provider/authStale path is actually exercised.
    std::fs::write(
        data_root.path().join("config.toml"),
        r#"
[defaults]
model_binding = "main"

[providers.openai]
enabled = true
name = "OpenAI"
credential = "test_api_key"
wire_apis = ["openai_chat_completions"]

[model_bindings.main]
enabled = true
model_slug = "default-model"
provider = "openai"
request_model = "provider-model"
invocation_method = "openai_chat_completions"
"#,
    )?;
    let router = Arc::new(QuotaRouter {
        text_before_error,
        attempts: AtomicUsize::new(0),
    });
    let runtime = build_runtime(data_root.path(), router.clone());
    let (connection_id, mut notifications) = initialize_connection(&runtime).await?;
    let response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 2,
                "method": "session/new",
                "params": {
                    "cwd": data_root.path(),
                    "idempotencyKey": "quota-session"
                }
            }),
        )
        .await
        .context("session/new response")?;
    let session = serde_json::from_value::<SuccessResponse<SessionNewResult>>(response)?
        .result
        .session;
    let session_id = session.id;
    let response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 3,
                "method": "session/metadata/update",
                "params": {
                    "sessionId": session_id,
                    "expectedVersion": 0,
                    "modelBindingId": "main"
                }
            }),
        )
        .await
        .context("session/metadata/update response")?;
    let _: SuccessResponse<SessionMetadataUpdateResult> = serde_json::from_value(response)?;
    let response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 4,
                "method": "turn/start",
                "params": {
                    "sessionId": session_id,
                    "input": [{ "type": "text", "text": "try the provider" }],
                    "idempotencyKey": "quota-turn"
                }
            }),
        )
        .await
        .context("turn/start response")?;
    let started = serde_json::from_value::<SuccessResponse<TurnStartResult>>(response)?.result;
    let turn_id = started.turn.id;
    let mut completed_turns = Vec::new();
    let mut retry_or_auth_notifications = Vec::new();
    let mut assistant_items = Vec::new();
    let mut assistant_text = String::new();
    // Real time: the runtime boots a real RLM kernel even with a mock provider.
    timeout(Duration::from_secs(30), async {
        while let Some(value) = notifications.recv().await {
            match value["method"].as_str() {
                Some("model/queryRetrying" | "provider/authStale") => {
                    retry_or_auth_notifications.push(value);
                }
                Some("item/assistantMessage/delta") => {
                    assistant_text.push_str(
                        value["params"]["delta"]
                            .as_str()
                            .context("assistant delta text")?,
                    );
                }
                Some("item/started" | "item/completed")
                    if value["params"]["item"]["item"]["type"] == "assistantMessage" =>
                {
                    assistant_items.push(value["params"]["item"].clone());
                }
                Some("turn/completed") if value["params"]["turn"]["id"] == json!(turn_id) => {
                    completed_turns.push(value["params"]["turn"].clone());
                }
                Some("session/statusChanged")
                    if value["params"]["sessionId"] == json!(session_id)
                        && value["params"]["status"] == "idle"
                        && !completed_turns.is_empty() =>
                {
                    return Ok(());
                }
                Some(_) | None => {}
            }
        }
        anyhow::bail!("notification channel closed before failed turn became idle")
    })
    .await
    .context("timed out waiting for quota failure")??;

    assert_eq!(router.attempts.load(Ordering::SeqCst), 1);
    assert_eq!(retry_or_auth_notifications, Vec::<Value>::new());
    assert_eq!(assistant_text, text_before_error);
    if text_before_error.is_empty() {
        assert_eq!(assistant_items, Vec::<Value>::new());
    }
    assert_eq!(completed_turns.len(), 1);
    let completed = completed_turns.pop().context("one turn/completed")?;
    // Preserve the provider's exact text, including Unicode and its request id,
    // inside the existing core/provider Display prefixes (no generic fallback).
    let message = format!("model provider error: quota exceeded: {QUOTA_MESSAGE}");
    assert_eq!(
        json!({ "status": completed["status"], "error": completed["error"] }),
        json!({
            "status": "failed",
            "error": {
                "errorCode": "QUOTA_EXCEEDED_ERROR",
                "message": message,
                "retryable": false,
                "requiresSnapshot": false
            }
        })
    );
    let failed_turn: Turn = serde_json::from_value(completed)?;
    assert_eq!(
        (failed_turn.status, failed_turn.error.clone()),
        (
            TurnStatus::Failed,
            Some(AgentError::new("QUOTA_EXCEEDED_ERROR", message.clone()))
        )
    );
    let rollout_path = data_root
        .path()
        .join("sessions")
        .join(format!("{session_id}.jsonl"));
    let persisted_failures = support::read_rollout_records(&rollout_path)?
        .into_iter()
        .filter_map(|line| {
            if let devo_core::LegacyRolloutLine::Turn(line) = line
                && line.turn.id.to_string() == turn_id.as_str()
                && line.turn.status == devo_protocol::TurnStatus::Failed
            {
                Some((line.turn.status, line.turn.error))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(
        persisted_failures,
        vec![(
            devo_protocol::TurnStatus::Failed,
            Some(devo_core::TurnError {
                code: "QUOTA_EXCEEDED_ERROR".to_string(),
                message,
                recovery_hint: None,
            })
        )]
    );

    // Rebuild the runtime: same-runtime resume could pass using cached actor state.
    runtime.shutdown().await;
    drop(runtime);
    let restored = build_runtime(data_root.path(), router.clone());
    restored.load_persisted_sessions().await?;
    let (restored_connection_id, _notifications) = initialize_connection(&restored).await?;
    let response = restored
        .handle_incoming(
            restored_connection_id,
            json!({
                "id": 5,
                "method": "session/resume",
                "params": { "sessionId": session_id }
            }),
        )
        .await
        .context("session/resume from rollout")?;
    let resumed = serde_json::from_value::<SuccessResponse<SessionResumeResult>>(response)?.result;
    assert_eq!(resumed.session.id, session_id);
    let response = restored
        .handle_incoming(
            restored_connection_id,
            json!({
                "id": 6,
                "method": "session/turns/list",
                "params": { "sessionId": session_id }
            }),
        )
        .await
        .context("session/turns/list after replay")?;
    let turns = serde_json::from_value::<SuccessResponse<Page<Turn>>>(response)?.result;
    assert_eq!(turns.data, vec![failed_turn]);
    assert_eq!(router.attempts.load(Ordering::SeqCst), 1);
    restored.shutdown().await;
    Ok(())
}

fn build_runtime(data_root: &Path, router: Arc<QuotaRouter>) -> Arc<ServerRuntime> {
    TestRuntime::noop()
        .router(router)
        .with_named_model("default-model", "Default Model")
        .default_model("default-model")
        .db_file("provider_quota_reporting.db")
        .runtime(data_root)
}

async fn initialize_connection(
    runtime: &Arc<ServerRuntime>,
) -> Result<(u64, mpsc::Receiver<Value>)> {
    let (tx, rx) = devo_server::test_outbound_channel(128);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, tx)
        .await;
    let response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientCapabilities": {},
                    "_meta": { "devo": { "protocol": "native" } },
                    "clientInfo": { "name": "quota-test", "version": "1.0.0" }
                }
            }),
        )
        .await
        .context("initialize response")?;
    assert!(
        response.get("result").is_some(),
        "initialize failed: {response}"
    );
    Ok((connection_id, rx))
}
