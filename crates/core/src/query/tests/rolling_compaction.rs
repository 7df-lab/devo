use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn long_single_turn_checkpoints_between_tool_batches_before_dispatch() {
    const TASK: &str = "Track the deployment through completion";
    const RESULT: &str = "long-tool-observation";
    let mut fixtures = ScriptToolFixtures::new(
        Arc::new(LargeToolResultTool::new(
            RESULT.repeat(6000),
            /*display_content*/ None,
        )),
        leak_spec("read", "read tool").clone(),
        TASK,
    );
    fixtures.session.config.token_budget = crate::context::TokenBudget::new(24_000, 1024);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(ScriptedProvider::capturing_script(
        "rolling-main",
        Arc::clone(&requests),
        StreamScript::RepeatedToolCall(3),
    ));
    let summarizer = Arc::new(CompactionProvider {
        completion_calls: AtomicUsize::new(0),
        outcome: CompactionProviderOutcome::Summary,
    });
    fixtures
        .query(
            provider,
            /*callback*/ None,
            TurnConfig::new(
                Model {
                    max_tokens: Some(1024),
                    truncation_policy: TruncationPolicyConfig::bytes(160_000),
                    ..Model::default()
                },
                /*reasoning_effort_selection*/ None,
            ),
            QueryOptions {
                compaction_provider: Some(summarizer.clone()),
                ..Default::default()
            },
        )
        .await
        .expect("finish one user turn");

    let sent = requests
        .lock()
        .unwrap()
        .iter()
        .map(|req| {
            let body = serde_json::to_string(&req.messages).unwrap();
            (
                body.contains(TASK),
                body.matches("<compaction_summary>").count(),
                body.contains(RESULT),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        sent,
        vec![
            (true, 0, false),
            (true, 1, false),
            (true, 1, false),
            (true, 1, false)
        ]
    );
    assert_eq!(summarizer.completion_calls.load(Ordering::SeqCst), 3);
    let completed_calls = fixtures
        .session
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(completed_calls, vec!["call-0", "call-1", "call-2"]);
    assert_eq!(
        fixtures.session.prompt_source_messages().last(),
        Some(&Message::assistant_text("done"))
    );
}

#[tokio::test]
async fn preflight_reserves_completion_space_even_with_an_absolute_input_threshold() {
    let model = Model {
        max_tokens: Some(4096),
        ..Model::default()
    };
    let mut budget = crate::context::TokenBudget::for_model(&model);
    let turn = TurnConfig::new(model, /*reasoning_effort_selection*/ None);
    let mut observed = Vec::new();
    for _ in 0..2 {
        let mut fixtures = ScriptToolFixtures::new(
            Arc::new(ParallelDelayTool::new()),
            leak_spec("read", "read tool").clone(),
            &"older observation ".repeat(3000),
        );
        fixtures
            .session
            .push_message(Message::assistant_text("recorded"));
        fixtures
            .session
            .push_message(Message::user("continue the job"));
        fixtures.session.config.token_budget = budget.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(ScriptedProvider::capturing("main", Arc::clone(&requests)));
        let summarizer = Arc::new(CompactionProvider {
            completion_calls: AtomicUsize::new(0),
            outcome: CompactionProviderOutcome::Summary,
        });
        fixtures
            .query(
                provider,
                /*callback*/ None,
                turn.clone(),
                QueryOptions {
                    compaction_provider: Some(summarizer.clone()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let requests = requests.lock().unwrap();
        let [request] = requests.as_slice() else {
            panic!("expected one main request");
        };
        let body = serde_json::to_string(&request.messages).unwrap();
        observed.push((
            summarizer.completion_calls.load(Ordering::SeqCst),
            body.contains("older observation"),
            body.contains("continue the job"),
        ));
        // The original input alone fits the second window. Reserving 4096
        // output tokens is the only reason to checkpoint before dispatch.
        let estimated = crate::query::estimate_request_context_breakdown(request).total() as usize;
        budget.context_window = estimated + 1024;
        budget.auto_compact_token_limit = Some(budget.context_window);
    }
    assert_eq!(observed, vec![(0, true, true), (1, false, true)]);
}
