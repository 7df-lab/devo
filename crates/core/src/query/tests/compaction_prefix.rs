use super::*;
use pretty_assertions::assert_eq;

const TASK: &str = "Track these observations through completion.";
const NEWEST: &str = "LATEST_OBSERVATION";

#[derive(Default)]
struct CaptureSummary {
    requests: Mutex<Vec<ModelRequest>>,
    reject_first: bool,
}

struct ScenarioProvider {
    main: Arc<ScriptStreamProvider>,
    summaries: Arc<CaptureSummary>,
}

#[async_trait]
impl ModelProviderSDK for ScenarioProvider {
    async fn completion(&self, request: ModelRequest) -> Result<ModelResponse> {
        let mut requests = self.summaries.requests.lock().unwrap();
        requests.push(request);
        if self.summaries.reject_first && requests.len() == 1 {
            anyhow::bail!("Prompt is too long for this deployment");
        }
        Ok(ModelResponse {
            id: "summary".into(),
            content: vec![ResponseContent::Text(
                "Earlier observations checked.".into(),
            )],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: Default::default(),
        })
    }

    async fn completion_stream(&self, request: ModelRequest) -> Result<EventStream> {
        self.main.completion_stream(request).await
    }

    fn name(&self) -> &str {
        "summary-capture"
    }
}

enum SummaryBackend {
    Shared,
    Separate,
}

async fn run_turn(summaries: Arc<CaptureSummary>, backend: SummaryBackend) -> Vec<ModelRequest> {
    let model = Model {
        slug: "prefix-test".into(),
        provider: devo_protocol::ProviderWireApi::OpenAIChatCompletions,
        context_window: 64_000,
        max_tokens: Some(4_000),
        truncation_policy: TruncationPolicyConfig::bytes(100_000),
        ..Default::default()
    };
    let make_fixture = |words| {
        let newest = format!("{NEWEST}{}", " log".repeat(6_000));
        let tool = LargeToolResultTool::new(newest, /*display_content*/ None);
        let mut fixture = ScriptToolFixtures::new(
            Arc::new(tool),
            leak_spec("ipython", "inspect observation").clone(),
            TASK,
        );
        fixture.session.config.token_budget = crate::context::TokenBudget::for_model(&model);
        for step in 0..13 {
            let call_id = format!("old-{step:02}");
            fixture.session.push_message(Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: call_id.clone(),
                    name: "ipython".into(),
                    input: json!({"step": step}),
                }],
            });
            fixture.session.push_message(Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: call_id,
                    content: format!("OBS_{step:02}{}", " log".repeat(words)),
                    is_error: false,
                }],
            });
        }
        fixture
    };
    let turn = TurnConfig::new(model.clone(), /*reasoning_effort_selection*/ None);
    // Calibrate the fixture to the assembled system prompt on this platform.
    let probe_requests = Arc::new(Mutex::new(Vec::new()));
    make_fixture(0)
        .query(
            Arc::new(ScriptedProvider::capturing(
                "calibration",
                probe_requests.clone(),
            )),
            /*callback*/ None,
            turn.clone(),
            QueryOptions::default(),
        )
        .await
        .unwrap();
    let overhead = crate::query::estimate_request_context_breakdown(
        &probe_requests.lock().unwrap()[0],
    )
    .total() as usize;
    let mut fixture = make_fixture((55_000 - overhead) / 13);
    let main = ScriptStreamProvider::new(
        "main",
        vec![
            ScriptStreamResponse {
                preamble: vec![],
                content: vec![ResponseContent::ToolUse {
                    id: "latest".into(),
                    name: "ipython".into(),
                    input: json!({"code":"observe()"}),
                }],
                metadata: Default::default(),
            },
            ScriptStreamResponse {
                preamble: vec![],
                content: vec![ResponseContent::Text("done".into())],
                metadata: Default::default(),
            },
        ],
    );
    let compaction_provider = match backend {
        SummaryBackend::Shared => None,
        SummaryBackend::Separate => Some(Arc::new(ScenarioProvider {
            main: main.clone(),
            summaries: summaries.clone(),
        }) as Arc<dyn ModelProviderSDK>),
    };
    fixture
        .query(
            Arc::new(ScenarioProvider {
                main: main.clone(),
                summaries,
            }),
            /*callback*/ None,
            turn,
            QueryOptions {
                compaction_provider,
                turn_user_inputs: vec![Message::user(TASK)],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    main.requests.lock().unwrap().clone()
}

#[tokio::test]
async fn accepted_prefix_survives_a_6k_tool_result_and_retries_at_a_complete_batch() {
    for reject_first in [false, true] {
        let capture = Arc::new(CaptureSummary {
            reject_first,
            ..Default::default()
        });
        let main = run_turn(capture.clone(), SummaryBackend::Shared).await;
        let [before, after] = main.as_slice() else {
            panic!("expected two main requests")
        };
        let initial = crate::query::estimate_request_context_breakdown(before).total();
        assert!(
            (54_000..56_800).contains(&initial),
            "initial input {initial}"
        );
        let summaries = capture.requests.lock().unwrap();
        assert_eq!(summaries.len(), if reject_first { 2 } else { 1 });
        let mut previous_cut = before.messages.len() + 1;
        for req in summaries.iter() {
            let (instruction, prefix) = req.messages.split_last().unwrap();
            assert!(prefix.len() < previous_cut);
            previous_cut = prefix.len();
            assert_eq!(prefix, &before.messages[..prefix.len()]);
            assert_eq!(instruction.role, "user");
            assert!(matches!(
                prefix.last().unwrap().content.as_slice(),
                [RequestContent::ToolResult { .. }]
            ));
            assert!(req.tools.is_some());
            let mut expected = before.clone();
            expected.messages = req.messages.clone();
            assert_eq!(json!(req), json!(expected));
            assert!(crate::query::estimate_request_context_breakdown(req).total() + 4_000 < 64_000);
        }
        let folded = serde_json::to_string(&summaries.last().unwrap().messages).unwrap();
        let retained = serde_json::to_string(&after.messages).unwrap();
        for step in 0..13 {
            let marker = format!("OBS_{step:02}");
            assert_ne!(
                folded.contains(&marker),
                retained.contains(&marker),
                "{marker}"
            );
        }
        assert!(!folded.contains(NEWEST));
        assert!(retained.contains(&format!("{NEWEST}{}", " log".repeat(6_000))));
        assert!(retained.contains(TASK));
        assert_eq!(retained.matches("<compaction_summary>").count(), 1);
    }
}

#[tokio::test]
async fn separate_summary_backend_receives_only_selected_history() {
    let capture = Arc::new(CaptureSummary::default());
    run_turn(capture.clone(), SummaryBackend::Separate).await;
    let requests = capture.requests.lock().unwrap();
    let [summary] = requests.as_slice() else {
        panic!("one isolated summary expected")
    };
    assert!(summary.system.is_none() && summary.tools.is_none() && summary.extra_body.is_none());
    assert!(summary.messages.iter().any(|message| {
        message.content.iter().any(|block|
        matches!(block, RequestContent::ToolResult { content, .. } if content.contains("OBS_00")))
    }));
}

// Opt in with a loopback OpenAI Chat Completions server configured with one
// warm cache-capable slot, a 64k window and the model alias "prefix-test".
#[tokio::test]
#[ignore = "requires a local inference server reporting cached_tokens"]
async fn live_compaction_prefix_cache_usage() {
    let port: u16 = std::env::var("DEVO_PREFIX_TEST_PORT")
        .unwrap()
        .parse()
        .unwrap();
    let backend = devo_provider::openai::OpenAIProvider::new(format!("http://127.0.0.1:{port}/v1"));
    let capture = Arc::new(CaptureSummary::default());
    let main = run_turn(capture.clone(), SummaryBackend::Shared).await;
    let mut warm = main.into_iter().next().unwrap();
    let summary = capture.requests.lock().unwrap()[0].clone();
    // The scripted tool result is supplied by the fixture. Warm the main
    // request through the normal streaming adapter, then generate a real summary.
    warm.max_tokens = 4;
    let mut stream = backend.completion_stream(warm).await.unwrap();
    while let Some(event) = futures::StreamExt::next(&mut stream).await {
        event.unwrap();
    }
    let response = backend.completion(summary).await.unwrap();
    assert!(
        response
            .content
            .iter()
            .any(|part| matches!(part, ResponseContent::Text(text) if !text.trim().is_empty()))
    );
    assert_eq!(response.stop_reason, Some(StopReason::EndTurn));
    let usage = response.usage;
    eprintln!(
        "summary input {}, cached {:?}, output {}",
        usage.input_tokens, usage.cache_read_input_tokens, usage.output_tokens
    );
    assert!(
        usage
            .cache_read_input_tokens
            .is_some_and(|n| n > 4_000 && n <= usage.input_tokens)
    );
}
