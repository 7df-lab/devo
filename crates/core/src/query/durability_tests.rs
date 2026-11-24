//! Crash-boundary checks for acknowledged execution.

use super::*;
use crate::durable_execution::{ExecutionRecord, ExecutionReplay, ToolIntentJournal};
use pretty_assertions::assert_eq;

#[derive(Clone, Copy)]
enum Fault {
    Intent,
    Outcome,
}

struct Journal {
    state: Mutex<ExecutionReplay>,
    fault: Option<Fault>,
}

#[async_trait]
impl ToolIntentJournal for Journal {
    async fn commit(&self, record: ExecutionRecord) -> Result<()> {
        if matches!(
            (&self.fault, &record),
            (Some(Fault::Intent), ExecutionRecord::IntentBatch { .. })
                | (Some(Fault::Outcome), ExecutionRecord::Outcomes { .. })
        ) {
            anyhow::bail!("injected durable write failure");
        }
        self.state.lock().unwrap().apply(&record)
    }

    async fn replay(&self) -> Result<ExecutionReplay> {
        Ok(self.state.lock().unwrap().clone())
    }
}

struct CountedTool {
    inner: StaticTool,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl ToolHandler for CountedTool {
    fn spec(&self) -> &ToolSpec {
        self.inner.spec()
    }

    async fn handle(
        &self,
        context: crate::tools::ToolContext,
        input: serde_json::Value,
        progress: Option<crate::tools::ToolProgressSender>,
    ) -> Result<crate::tools::ToolResult, crate::tools::ToolCallError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.handle(context, input, progress).await
    }
}

#[tokio::test]
async fn journal_failures_stop_dispatch_or_model_continuation() {
    for (fault, expected_calls) in [(Fault::Intent, 0), (Fault::Outcome, 1)] {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler(
            "mutating_tool",
            Arc::new(CountedTool {
                inner: mutating_tool(),
                calls: calls.clone(),
            }),
        );
        builder.push_spec(ToolSpec::new(
            "mutating_tool",
            "test mutation",
            JsonSchema::object(Default::default(), None, None),
        ));
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_without_permissions(registry.clone());
        let provider = Arc::new(ScriptedProvider::scripted(
            "test-provider",
            StreamScript::SingleMutating,
        ));
        let journal = Arc::new(Journal {
            state: Mutex::new(ExecutionReplay::default()),
            fault: Some(fault),
        });
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("execute"));
        let result = query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            provider.clone(),
            registry,
            &runtime,
            None,
            QueryOptions {
                journal: Some(journal),
                ..QueryOptions::default()
            },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(
            (
                calls.load(Ordering::SeqCst),
                provider.attempts.load(Ordering::SeqCst)
            ),
            (expected_calls, 1)
        );
    }
}

#[tokio::test]
async fn acknowledged_final_response_only_finishes_bookkeeping() {
    let messages = vec![
        Message::user("hello"),
        Message::assistant_text("saved answer"),
    ];
    let mut replay = ExecutionReplay::default();
    replay
        .apply(&ExecutionRecord::ModelCompleted {
            items: messages
                .iter()
                .cloned()
                .flat_map(crate::message_to_response_items)
                .collect(),
            stop_reason: Some(StopReason::EndTurn),
        })
        .unwrap();
    let journal = Arc::new(Journal {
        state: Mutex::new(replay),
        fault: None,
    });
    let provider = Arc::new(ScriptedProvider::scripted(
        "test-provider",
        StreamScript::SingleMutating,
    ));
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(registry.clone());
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        provider.clone(),
        registry,
        &runtime,
        None,
        QueryOptions {
            journal: Some(journal),
            ..QueryOptions::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        (
            provider.attempts.load(Ordering::SeqCst),
            session.prompt_source_messages()
        ),
        (0, messages.as_slice())
    );
}

#[tokio::test]
async fn resumed_compaction_uses_checkpoint_positions_for_resolved_human_inputs() {
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.config.token_budget = crate::context::TokenBudget::new(24_000, 1024);
    let inputs = [
        Message::user("Inspect the attached graph."),
        Message::user("Preserve its labels."),
        Message {
            role: Role::User,
            content: vec![ContentBlock::Image {
                mime_type: "image/png".into(),
                data_base64: "c25hcHNob3Q=".into(),
            }],
        },
        Message::user("Use the corrected labels."),
    ];
    let mut history = inputs[..3].to_vec();
    history.push(Message::assistant_text("old observation ".repeat(8000)));
    history.push(inputs[3].clone());
    history.push(Message::assistant_text("continuing"));
    history.push(Message::user("Please continue from where you left off."));
    let items = history
        .iter()
        .cloned()
        .flat_map(crate::message_to_response_items)
        .collect();
    let pins: Vec<_> = inputs
        .iter()
        .cloned()
        .flat_map(crate::message_to_response_items)
        .collect();
    let checkpoint = super::super::context_recovery::prompt_checkpoint(&session, items, &pins);
    let mut replay = ExecutionReplay::default();
    replay.apply(&checkpoint).unwrap();
    assert_eq!(replay.turn_input_indices, vec![0, 1, 2, 4]);
    session.set_prompt_messages(history);
    let journal = Arc::new(Journal {
        state: Mutex::new(replay),
        fault: None,
    });
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(registry.clone());
    let summary = Arc::new(CompactionProvider {
        completion_calls: AtomicUsize::new(0),
        outcome: CompactionProviderOutcome::Summary,
    });
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(ScriptedProvider::capturing("resume", requests.clone()));
    query(
        &mut session,
        &TurnConfig::new(
            Model {
                input_modalities: vec![
                    devo_protocol::InputModality::Text,
                    devo_protocol::InputModality::Image,
                ],
                max_tokens: Some(1024),
                ..Model::default()
            },
            /*reasoning_effort_selection*/ None,
        ),
        provider,
        registry,
        &runtime,
        /*on_event*/ None,
        QueryOptions {
            // A display projection can combine parts and omit image data.
            turn_user_inputs: vec![
                Message::user("Inspect the attached graph. Preserve its labels."),
                inputs[3].clone(),
            ],
            journal: Some(journal),
            compaction_provider: Some(summary),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let sent = serde_json::to_string(&requests.lock().unwrap()[0].messages).unwrap();
    assert!(sent.contains("<compaction_summary>") && !sent.contains("old observation"));
    let retained: Vec<_> = session
        .prompt_source_messages()
        .iter()
        .filter(|m| inputs.contains(m))
        .cloned()
        .collect();
    assert_eq!(retained, inputs);
}
