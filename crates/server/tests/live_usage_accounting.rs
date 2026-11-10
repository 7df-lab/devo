//! Real runtime/actor/Native RPC coverage. The model is deterministic; its tool
//! request traverses the real tool runner, and the next request stays blocked.
use anyhow::{Context, Result};
use async_trait::async_trait;
use devo_protocol::native::{ids::SessionId, usage::UsageTotals};
use devo_protocol::{
    ModelRequest, ModelResponse, ResponseContent, ResponseMetadata, StopReason, StreamEvent, Usage,
};
use devo_provider::ModelProviderSDK;
use devo_server::{ClientTransportKind, ServerRuntime, test_support::TestRuntime};
use futures::{Stream, stream};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::{
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tempfile::TempDir;
use tokio::sync::{Notify, mpsc};
use tokio::time::{Duration, timeout};

struct GatedProvider {
    calls: AtomicUsize,
    second_started: Notify,
    release: Notify,
}

#[async_trait]
impl ModelProviderSDK for GatedProvider {
    async fn completion(&self, _: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("No auxiliary request expected with a user-set title")
    }
    async fn completion_stream(
        &self,
        _: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        let (usage, content, stop_reason) = if index == 0 {
            (
                Usage {
                    reported: true,
                    input_tokens: 100,
                    output_tokens: 20,
                    cache_read_input_tokens: Some(40),
                    ..Usage::default()
                }
                .with_field_coverage(
                    /*input_tokens*/ Some(0),
                    /*output_tokens*/ Some(0),
                ),
                vec![ResponseContent::ToolUse {
                    id: "usage-tool".into(),
                    name: "ipython".into(),
                    input: json!({"code":"print('usage tool finished')"}),
                }],
                StopReason::ToolUse,
            )
        } else {
            self.second_started.notify_one();
            self.release.notified().await;
            (
                Usage {
                    reported: true,
                    input_tokens: 200,
                    output_tokens: 30,
                    cache_read_input_tokens: Some(80),
                    ..Usage::default()
                }
                .with_field_coverage(
                    /*input_tokens*/ Some(0),
                    /*output_tokens*/ Some(0),
                ),
                vec![ResponseContent::Text("done".into())],
                StopReason::EndTurn,
            )
        };
        let mut events = vec![
            Ok(StreamEvent::UsageDelta(Usage::default())),
            Ok(StreamEvent::UsageDelta(usage.clone())),
        ];
        if let ResponseContent::ToolUse { id, name, input } = &content[0] {
            events.push(Ok(StreamEvent::ToolCallStart {
                index: 0,
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            }));
        }
        events.push(Ok(StreamEvent::MessageDone {
            response: ModelResponse {
                id: format!("response-{index}"),
                content,
                stop_reason: Some(stop_reason),
                usage,
                metadata: ResponseMetadata::default(),
            },
        }));
        Ok(Box::pin(stream::iter(events)))
    }
    fn name(&self) -> &str {
        "live-usage"
    }
}

async fn rpc(
    runtime: &Arc<ServerRuntime>,
    connection: u64,
    method: &str,
    params: Value,
) -> Result<Value> {
    let response = timeout(
        Duration::from_secs(5),
        runtime.handle_incoming(connection, json!({"id":1,"method":method,"params":params})),
    )
    .await
    .context("RPC blocked")?
    .context("RPC response")?;
    anyhow::ensure!(response.get("error").is_none(), "{method}: {response}");
    Ok(response["result"].clone())
}

async fn connect(runtime: &Arc<ServerRuntime>) -> Result<(u64, mpsc::Receiver<Value>)> {
    let (tx, rx) = devo_server::test_outbound_channel(4096);
    let connection = runtime
        .register_connection(ClientTransportKind::Stdio, tx)
        .await;
    rpc(
        runtime,
        connection,
        "initialize",
        json!({"protocolVersion":1,"clientCapabilities":{},
        "clientInfo":{"name":"usage-test","title":"usage-test","version":"1"},
        "_meta":{"devo":{"protocol":"native"}}}),
    )
    .await?;
    Ok((connection, rx))
}

async fn wait_usage(rx: &mut mpsc::Receiver<Value>, count: u64) -> Result<Value> {
    timeout(Duration::from_secs(30), async {
        while let Some(value) = rx.recv().await {
            if value["method"] == "session/usage/updated"
                && value["params"]["usage"]["total"]["callCount"] == count
            {
                return Ok(value["params"]["usage"].clone());
            }
            anyhow::ensure!(
                value["method"] != "turn/completed",
                "turn ended before requested usage update: {value}"
            );
        }
        anyhow::bail!("notification stream closed")
    })
    .await
    .context("usage notification timeout")?
}

#[tokio::test]
async fn completed_calls_update_notifications_and_busy_reads_before_tools_and_turn_end()
-> Result<()> {
    let temp = TempDir::new()?;
    let provider = Arc::new(GatedProvider {
        calls: AtomicUsize::new(0),
        second_started: Notify::new(),
        release: Notify::new(),
    });
    let runtime = TestRuntime::new(provider.clone())
        .with_test_model()
        .disabled_skills()
        .runtime(temp.path());
    let (connection, mut rx) = connect(&runtime).await?;
    let result = rpc(
        &runtime,
        connection,
        "session/new",
        json!({"cwd":temp.path(),"idempotencyKey":"usage-session"}),
    )
    .await?;
    let session: SessionId = serde_json::from_value(result["session"]["id"].clone())?;
    rpc(
        &runtime,
        connection,
        "session/metadata/update",
        json!({"sessionId":session,"expectedVersion":0,"title":"Usage test"}),
    )
    .await?;
    rpc(
        &runtime,
        connection,
        "subscription/create",
        json!({"selectors":[{"kind":"session","sessionId":session}],"includeSnapshot":false}),
    )
    .await?;
    rpc(&runtime, connection, "turn/start", json!({"sessionId":session,"input":[{"type":"text","text":"Run a tool."}],"idempotencyKey":"usage-turn"})).await?;
    let first = wait_usage(&mut rx, 1).await?;
    assert_eq!(
        serde_json::from_value::<UsageTotals>(first["total"].clone())?,
        UsageTotals {
            unreported_fields: Some(vec![
                devo_protocol::native::usage::UsageField::ReasoningTokens,
                devo_protocol::native::usage::UsageField::CacheCreationInputTokens
            ]),
            input_tokens: 100,
            output_tokens: 20,
            total_tokens: 120,
            cache_read_input_tokens: 40,
            call_count: 1,
            metered_call_count: 1,
            ..UsageTotals::default()
        }
    );
    timeout(Duration::from_secs(30), provider.second_started.notified())
        .await
        .context("second model request never started")?;
    let busy = rpc(
        &runtime,
        connection,
        "session/read",
        json!({"sessionId":session}),
    )
    .await?;
    assert_eq!(busy["session"]["usage"], first);
    assert_eq!(busy["session"]["status"], "active");
    let list = rpc(&runtime, connection, "session/list", json!({})).await?;
    assert_eq!(
        list["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == json!(session))
            .unwrap()["usage"],
        first
    );
    let context = rpc(
        &runtime,
        connection,
        "context/usage/read",
        json!({"sessionId":session}),
    )
    .await?;
    assert_eq!(context["occupancy"]["totalTokens"], 120);

    // Copy the canonical crash-time journal while call 2 is still pending.
    // The new runtime has no stats DB and no terminal turn to repair the count.
    let crash = TempDir::new()?;
    std::fs::create_dir_all(crash.path().join("sessions"))?;
    let journal = format!("{session}.jsonl");
    std::fs::copy(
        temp.path().join("sessions").join(&journal),
        crash.path().join("sessions").join(&journal),
    )?;
    let rebuilt = TestRuntime::noop()
        .with_test_model()
        .disabled_skills()
        .runtime(crash.path());
    rebuilt.refresh_session_index()?;
    let (reconnected, _) = connect(&rebuilt).await?;
    // List is the first session RPC: no actor hydration/read can repair it.
    let recovered_list = rpc(&rebuilt, reconnected, "session/list", json!({})).await?;
    assert_eq!(
        recovered_list["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == json!(session))
            .unwrap()["usage"],
        first
    );
    let recovered = rpc(
        &rebuilt,
        reconnected,
        "session/read",
        json!({"sessionId":session}),
    )
    .await?;
    assert_eq!(recovered["session"]["usage"], first);
    rebuilt.shutdown().await;

    provider.release.notify_one();
    let mut final_turn = None;
    let mut completed_usage = None;
    timeout(Duration::from_secs(30), async {
        while let Some(value) = rx.recv().await {
            if value["method"] == "session/usage/updated"
                && value["params"]["usage"]["total"]["callCount"] == 2
            {
                completed_usage = Some(value["params"]["usage"].clone());
            }
            if value["method"] == "turn/completed" {
                final_turn = Some(value["params"]["turn"].clone());
            }
            if completed_usage.is_some() && final_turn.is_some() {
                break;
            }
        }
    })
    .await
    .context("completion timeout")?;
    assert_eq!(
        serde_json::from_value::<UsageTotals>(
            completed_usage.context("second committed notification")?["total"].clone()
        )?,
        UsageTotals {
            unreported_fields: Some(vec![
                devo_protocol::native::usage::UsageField::ReasoningTokens,
                devo_protocol::native::usage::UsageField::CacheCreationInputTokens
            ]),
            input_tokens: 300,
            output_tokens: 50,
            total_tokens: 350,
            cache_read_input_tokens: 120,
            call_count: 2,
            metered_call_count: 2,
            ..UsageTotals::default()
        }
    );
    assert_eq!(
        final_turn.context("completed turn")?["usage"]["query"]["totalTokens"],
        350
    );
    let final_context = rpc(
        &runtime,
        connection,
        "context/usage/read",
        json!({"sessionId":session}),
    )
    .await?;
    assert_eq!(final_context["occupancy"]["totalTokens"], 230);
    runtime.shutdown().await;
    Ok(())
}

#[path = "live_usage_accounting/recovery.rs"]
mod recovery;
