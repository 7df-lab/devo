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
