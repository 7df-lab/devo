use super::*;
use pretty_assertions::assert_eq;

struct ReplyProvider {
    queries: AtomicUsize,
    auxiliary: AtomicUsize,
    large_second: bool,
}

fn reply(input: usize, output: usize) -> ModelResponse {
    ModelResponse {
        id: "reply".into(),
        content: vec![ResponseContent::Text(
            "A compact summary of the completed task.".into(),
        )],
        stop_reason: Some(StopReason::EndTurn),
        usage: Usage {
            reported: true,
            input_tokens: input,
            output_tokens: output,
            unreported_fields: Some(Vec::new()),
            ..Usage::default()
        },
        metadata: ResponseMetadata::default(),
    }
}

#[async_trait]
impl ModelProviderSDK for ReplyProvider {
    async fn completion(&self, _: ModelRequest) -> Result<ModelResponse> {
        self.auxiliary.fetch_add(1, Ordering::SeqCst);
        Ok(reply(5, 2))
    }
    async fn completion_stream(
        &self,
        _: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let index = self.queries.fetch_add(1, Ordering::SeqCst);
        Ok(Box::pin(stream::iter([Ok(StreamEvent::MessageDone {
            response: reply(
                if self.large_second && index == 1 {
                    400_000
                } else {
                    100
                },
                20,
            ),
        })])))
    }
    fn name(&self) -> &str {
        "usage-recovery"
    }
}

async fn create(
    runtime: &Arc<ServerRuntime>,
    connection: u64,
    temp: &TempDir,
) -> Result<SessionId> {
    let created = rpc(
        runtime,
        connection,
        "session/new",
        json!({"cwd":temp.path(),"idempotencyKey":"source"}),
    )
    .await?;
    let session: SessionId = serde_json::from_value(created["session"]["id"].clone())?;
    rpc(
        runtime,
        connection,
        "session/metadata/update",
        json!({"sessionId":session,"expectedVersion":0,"title":"Fixed title"}),
    )
    .await?;
    rpc(
        runtime,
        connection,
        "subscription/create",
        json!({"selectors":[{"kind":"session","sessionId":session}],"includeSnapshot":false}),
    )
    .await?;
    Ok(session)
}

async fn terminal(rx: &mut mpsc::Receiver<Value>) -> Result<Value> {
    timeout(Duration::from_secs(30), async {
        while let Some(value) = rx.recv().await {
            if value["method"] == "turn/completed" {
                return Ok(value["params"]["turn"].clone());
            }
        }
        anyhow::bail!("closed before terminal turn")
    })
    .await
    .context("terminal timeout")?
}

async fn turn(
    runtime: &Arc<ServerRuntime>,
    connection: u64,
    rx: &mut mpsc::Receiver<Value>,
    session: SessionId,
    text: &str,
    key: &str,
) -> Result<()> {
    rpc(
        runtime,
        connection,
        "turn/start",
        json!({"sessionId":session,"input":[{"type":"text","text":text}],"idempotencyKey":key}),
    )
    .await?;
    let end = terminal(rx).await?;
    assert_eq!(end["status"], "completed", "{end}");
    Ok(())
}

#[tokio::test]
async fn fork_spend_starts_empty_across_read_list_restart_and_new_call() -> Result<()> {
    let temp = TempDir::new()?;
    let provider = Arc::new(ReplyProvider {
        queries: AtomicUsize::new(0),
        auxiliary: AtomicUsize::new(0),
        large_second: false,
    });
    let runtime = TestRuntime::new(provider.clone())
        .with_test_model()
        .disabled_skills()
        .runtime(temp.path());
    let (connection, mut rx) = connect(&runtime).await?;
    let source = create(&runtime, connection, &temp).await?;
    turn(
        &runtime,
        connection,
        &mut rx,
        source,
        "Source",
        "source-turn",
    )
    .await?;
    let fork = rpc(
        &runtime,
        connection,
        "session/fork",
        json!({"sessionId":source}),
    )
    .await?;
    let fork: SessionId = serde_json::from_value(fork["session"]["id"].clone())?;
    let read = rpc(
        &runtime,
        connection,
        "session/read",
        json!({"sessionId":fork}),
    )
    .await?;
    assert_eq!(read["session"]["usage"]["total"]["totalTokens"], 0);
    let list = rpc(&runtime, connection, "session/list", json!({})).await?;
    assert_eq!(
        list["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == json!(fork))
            .unwrap()["usage"]["total"]["callCount"],
        0
    );
    runtime.shutdown().await;
    drop(runtime);
    let rebuilt = TestRuntime::new(provider)
        .with_test_model()
        .disabled_skills()
        .runtime(temp.path());
    rebuilt.refresh_session_index()?;
    let (connection, mut rx) = connect(&rebuilt).await?;
    let list = rpc(&rebuilt, connection, "session/list", json!({})).await?;
    let usage = &list["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == json!(fork))
        .unwrap()["usage"];
    assert_eq!(usage["total"]["totalTokens"], 0);
    assert_eq!(usage["total"]["callCount"], 0);
    rpc(
        &rebuilt,
        connection,
        "subscription/create",
        json!({"selectors":[{"kind":"session","sessionId":fork}],"includeSnapshot":false}),
    )
    .await?;
    rpc(
        &rebuilt,
        connection,
        "session/resume",
        json!({"sessionId":fork}),
    )
    .await?;
    turn(
        &rebuilt,
        connection,
        &mut rx,
        fork,
        "Independent request",
        "fork-turn",
    )
    .await?;
    let read = rpc(
        &rebuilt,
        connection,
        "session/read",
        json!({"sessionId":fork}),
    )
    .await?;
    assert_eq!(read["session"]["usage"]["total"]["totalTokens"], 120);
    assert_eq!(read["session"]["usage"]["total"]["callCount"], 1);
    rebuilt.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn manual_compaction_context_survives_reload_without_repeat_compaction() -> Result<()> {
    let temp = TempDir::new()?;
    let provider = Arc::new(ReplyProvider {
        queries: AtomicUsize::new(0),
        auxiliary: AtomicUsize::new(0),
        large_second: true,
    });
    let runtime = TestRuntime::new(provider.clone())
        .with_test_model()
        .disabled_skills()
        .runtime(temp.path());
    let (connection, mut rx) = connect(&runtime).await?;
    let session = create(&runtime, connection, &temp).await?;
    turn(
        &runtime,
        connection,
        &mut rx,
        session,
        &"a".repeat(30_000),
        "one",
    )
    .await?;
    turn(
        &runtime,
        connection,
        &mut rx,
        session,
        &"b".repeat(30_000),
        "two",
    )
    .await?;
    rpc(
        &runtime,
        connection,
        "session/compact/start",
        json!({"sessionId":session}),
    )
    .await?;
    terminal(&mut rx).await?;
    let compacted = rpc(
        &runtime,
        connection,
        "context/usage/read",
        json!({"sessionId":session}),
    )
    .await?;
    assert!(
        compacted["occupancy"]["totalTokens"]
            .as_u64()
            .context("compacted occupancy")?
            < 200_000
    );
    assert_eq!(provider.auxiliary.load(Ordering::SeqCst), 1);
    runtime.shutdown().await;
    drop(runtime);
    let rebuilt = TestRuntime::new(provider.clone())
        .with_test_model()
        .disabled_skills()
        .runtime(temp.path());
    rebuilt.refresh_session_index()?;
    let (connection, mut rx) = connect(&rebuilt).await?;
    rpc(
        &rebuilt,
        connection,
        "session/resume",
        json!({"sessionId":session}),
    )
    .await?;
    let restored = rpc(
        &rebuilt,
        connection,
        "context/usage/read",
        json!({"sessionId":session}),
    )
    .await?;
    assert_eq!(
        restored["occupancy"]["totalTokens"],
        compacted["occupancy"]["totalTokens"]
    );
    rpc(
        &rebuilt,
        connection,
        "subscription/create",
        json!({"selectors":[{"kind":"session","sessionId":session}],"includeSnapshot":false}),
    )
    .await?;
    turn(
        &rebuilt,
        connection,
        &mut rx,
        session,
        "Continue",
        "after-compact",
    )
    .await?;
    assert_eq!(
        provider.auxiliary.load(Ordering::SeqCst),
        1,
        "restart must not restore pre-compaction pressure"
    );
    assert_eq!(provider.queries.load(Ordering::SeqCst), 3);
    rebuilt.shutdown().await;
    Ok(())
}

// Compatible streaming endpoints may report zero before counting the prompt.
// Exercise the real resume -> actor -> core stream -> Native notification path.
struct ProvisionalZeroProvider {
    release: Arc<Notify>,
    final_tokens: Option<usize>,
    intermediate: bool,
    fail: bool,
}

#[async_trait]
impl ModelProviderSDK for ProvisionalZeroProvider {
    async fn completion(&self, _: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("No auxiliary request expected with a fixed title")
    }

    async fn completion_stream(
        &self,
        _: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let release = self.release.clone();
        let final_tokens = self.final_tokens;
        let fail = self.fail;
        let zero = reply(/*input*/ 0, /*output*/ 0).usage;
        let mut events = Vec::new();
        if self.intermediate {
            let mut measured = reply(150, 0).usage;
            measured.total_tokens = Some(0); // tolerate a placeholder total
            events.push(Ok(StreamEvent::UsageDelta(measured)));
        }
        events.extend([
            Ok(StreamEvent::UsageDelta(zero)),
            Ok(StreamEvent::TextStart { index: 0 }),
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: "usage-start-marker".into(),
            }),
        ]);
        let prefix = stream::iter(events);
        let final_event = stream::once(async move {
            release.notified().await;
            if fail {
                anyhow::bail!("deliberate stream failure")
            }
            let mut response = reply(final_tokens.unwrap_or(0), 0);
            if final_tokens.is_none() {
                response.usage = Usage::default();
            }
            Ok(StreamEvent::MessageDone { response })
        });
        Ok(Box::pin(futures::StreamExt::chain(prefix, final_event)))
    }

    fn name(&self) -> &str {
        "provisional-zero"
    }
}

async fn resumed_context_survives_zero_stream_sample(
    final_tokens: Option<usize>,
    intermediate: bool,
    fail: bool,
    cancel: bool,
) -> Result<()> {
    let temp = TempDir::new()?;
    let initial = Arc::new(ReplyProvider {
        queries: AtomicUsize::new(0),
        auxiliary: AtomicUsize::new(0),
        large_second: false,
    });
    let runtime = TestRuntime::new(initial)
        .with_test_model()
        .disabled_skills()
        .runtime(temp.path());
    let (connection, mut rx) = connect(&runtime).await?;
    let session = create(&runtime, connection, &temp).await?;
    turn(&runtime, connection, &mut rx, session, "First", "first").await?;
    runtime.shutdown().await;
    drop(runtime);

    let release = Arc::new(Notify::new());
    let rebuilt = TestRuntime::new(Arc::new(ProvisionalZeroProvider {
        release: release.clone(),
        final_tokens,
        intermediate,
        fail,
    }))
    .with_test_model()
    .disabled_skills()
    .runtime(temp.path());
    rebuilt.refresh_session_index()?;
    let (connection, mut rx) = connect(&rebuilt).await?;
    let resumed = rpc(
        &rebuilt,
        connection,
        "session/resume",
        json!({"sessionId":session}),
    )
    .await?;
    assert_eq!(resumed["lastContextOccupancy"]["totalTokens"], 120);
    rpc(
        &rebuilt,
        connection,
        "subscription/create",
        json!({"selectors":[{"kind":"session","sessionId":session}],"includeSnapshot":false}),
    )
    .await?;
    rpc(
        &rebuilt,
        connection,
        "turn/start",
        json!({"sessionId":session,"input":[{"type":"text","text":"Continue"}],"idempotencyKey":"second"}),
    )
    .await?;
    timeout(Duration::from_secs(30), async {
        while let Some(value) = rx.recv().await {
            if value["method"] == "context/usageUpdated" {
                assert_ne!(value["params"]["occupancy"]["totalTokens"], 0);
            }
            if value.to_string().contains("usage-start-marker") {
                return Ok::<_, anyhow::Error>(());
            }
        }
        anyhow::bail!("closed before streamed marker")
    })
    .await
    .context("stream marker timeout")??;
    let live = rpc(
        &rebuilt,
        connection,
        "context/usage/read",
        json!({"sessionId":session}),
    )
    .await?;
    let last_known = if intermediate { 150 } else { 120 };
    assert_eq!(live["occupancy"]["totalTokens"], last_known);
    if cancel {
        rpc(
            &rebuilt,
            connection,
            "session/interrupt",
            json!({"scope":{"scope":"session","sessionId":session}}),
        )
        .await?;
    } else {
        release.notify_one();
    }
    assert_eq!(
        terminal(&mut rx).await?["status"],
        if cancel {
            "interrupted"
        } else if fail {
            "failed"
        } else {
            "completed"
        }
    );
    let completed = rpc(
        &rebuilt,
        connection,
        "context/usage/read",
        json!({"sessionId":session}),
    )
    .await?;
    assert_eq!(
        completed["occupancy"]["totalTokens"],
        if fail || cancel {
            last_known
        } else {
            final_tokens.unwrap_or(last_known)
        }
    );
    rebuilt.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn resumed_context_is_not_reset_by_initial_zero_stream_usage() -> Result<()> {
    resumed_context_survives_zero_stream_sample(Some(230), false, false, false).await
}

#[tokio::test]
async fn completed_zero_usage_still_replaces_resumed_context() -> Result<()> {
    resumed_context_survives_zero_stream_sample(Some(0), false, false, false).await
}

#[tokio::test]
async fn missing_final_usage_does_not_promote_initial_zero() -> Result<()> {
    resumed_context_survives_zero_stream_sample(None, false, false, false).await
}
#[tokio::test]
async fn failed_stream_does_not_promote_initial_zero() -> Result<()> {
    resumed_context_survives_zero_stream_sample(None, false, true, false).await
}
#[tokio::test]
async fn cancelled_stream_does_not_promote_initial_zero() -> Result<()> {
    resumed_context_survives_zero_stream_sample(None, false, false, true).await
}
#[tokio::test]
async fn later_zero_does_not_replace_nonzero_stream_counters() -> Result<()> {
    resumed_context_survives_zero_stream_sample(None, true, false, false).await
}
