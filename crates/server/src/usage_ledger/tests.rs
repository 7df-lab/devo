use devo_core::RolloutLine;
use devo_core::parse_rollout_line;
use devo_protocol::ModelProfileKey;
use devo_protocol::SamplingControls;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;

#[test]
fn usage_record_is_appended_to_the_session_rollout() {
    let temp = TempDir::new().expect("temp dir");
    let db = Arc::new(Database::open(temp.path().join("devo.db")).expect("open database"));
    let rollout_store = RolloutStore::new(temp.path().to_path_buf(), Some(Arc::clone(&db)));
    let session_id = SessionId::new();
    let session_record = rollout_store.create_session_record(
        session_id,
        Utc::now(),
        temp.path().to_path_buf(),
        Vec::new(),
        None,
        Some("catalog-model".into()),
        None,
        None,
        "test-provider".into(),
        None,
    );
    rollout_store
        .append_session_meta(&session_record)
        .expect("append session metadata");
    let index_row = crate::persistence::session_index_row_from_record(
        &session_record,
        session_record.created_at,
    );
    db.upsert_session(index_row, Some(session_record.rollout_path.as_path()))
        .expect("index session");
    let ledger = UsageLedger::new(rollout_store, db);
    let request = ModelRequest {
        model_slug: ModelProfileKey::CatalogSlug("catalog-model".into()),
        model: "wire-model".into(),
        system: None,
        messages: Vec::new(),
        max_tokens: 128,
        tools: None,
        hosted_tools: Vec::new(),
        sampling: SamplingControls::default(),
        request_thinking: None,
        reasoning_effort: None,
        extra_body: None,
    };
    let usage = Usage {
        unreported_fields: None,
        reported: true,
        input_tokens: 10,
        output_tokens: 4,
        cache_creation_input_tokens: Some(2),
        cache_read_input_tokens: Some(3),
        reasoning_output_tokens: Some(1),
        total_tokens: Some(14),
    };

    ledger
        .record(
            "test-call",
            &RuntimeCallContext::new(session_id, None, UsagePurpose::TitleGeneration),
            "test-provider",
            &request,
            UsageCallOutcome::Succeeded,
            Some(&usage),
        )
        .expect("persist usage");

    let raw = std::fs::read_to_string(&session_record.rollout_path).expect("read rollout");
    let last = raw.lines().last().expect("usage line");
    let line = parse_rollout_line(last).expect("parse usage line");
    let RolloutLine::Internal {
        entry: devo_core::InternalRecord::UsageRecord { record },
        ..
    } = line
    else {
        panic!("usage must use the internal usage record");
    };
    assert_eq!(
        record,
        UsageRecord {
            call_id: record.call_id.clone(),
            session_id,
            turn_id: None,
            parent: None,
            purpose: UsagePurpose::TitleGeneration,
            model: ModelBinding {
                provider: "test-provider".into(),
                model: "wire-model".into(),
                variant: None,
                reasoning_effort: None,
            },
            outcome: UsageCallOutcome::Succeeded,
            usage: Some(TokenUsage {
                unreported_fields: None,
                total_tokens: Some(14),
                input_tokens: 10,
                output_tokens: 4,
                reasoning_tokens: 1,
                cache_read_input_tokens: 3,
                cache_creation_input_tokens: 2,
            }),
            estimated_cost: None,
            recorded_at: record.recorded_at,
        }
    );
}

use devo_protocol::native::usage::UsageTotals;
use devo_protocol::{ResponseContent, ResponseMetadata, StopReason};
use futures::{StreamExt, stream};

struct Fixture {
    _temp: TempDir,
    ledger: UsageLedger,
    session: devo_core::SessionRecord,
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let db = Arc::new(Database::open(temp.path().join("devo.db")).unwrap());
        let store = RolloutStore::new(temp.path().to_path_buf(), Some(db.clone()));
        let session = store.create_session_record(
            SessionId::new(),
            Utc::now(),
            temp.path().to_path_buf(),
            Vec::new(),
            None,
            Some("test".into()),
            None,
            None,
            "test".into(),
            None,
        );
        store.append_session_meta(&session).unwrap();
        let row = crate::persistence::session_index_row_from_record(&session, session.created_at);
        db.upsert_session(row, Some(&session.rollout_path)).unwrap();
        Self {
            _temp: temp,
            ledger: UsageLedger::new(store, db),
            session,
        }
    }

    fn replay(&self) -> UsageLedger {
        let ledger = UsageLedger::new(self.ledger.rollout_store.clone(), self.ledger.db.clone());
        ledger.load_session(self.session.id).unwrap();
        ledger
    }

    fn record(
        &self,
        session: SessionId,
        turn: TurnId,
        usage: Usage,
        parent: Option<UsageAttribution>,
    ) {
        let context = RuntimeCallContext {
            parent,
            ..RuntimeCallContext::new(session, Some(turn), UsagePurpose::TurnQuery)
        };
        self.ledger
            .record(
                &Uuid::now_v7().to_string(),
                &context,
                "test",
                &request(),
                UsageCallOutcome::Succeeded,
                Some(&usage),
            )
            .unwrap();
    }
}

fn request() -> ModelRequest {
    ModelRequest {
        model_slug: ModelProfileKey::CatalogSlug("test".into()),
        model: "test".into(),
        system: None,
        messages: vec![],
        max_tokens: 128,
        tools: None,
        hosted_tools: vec![],
        sampling: SamplingControls::default(),
        request_thinking: None,
        reasoning_effort: None,
        extra_body: None,
    }
}

fn reported(input: usize, output: usize) -> Usage {
    Usage {
        unreported_fields: Some(Vec::new()),
        reported: true,
        input_tokens: input,
        output_tokens: output,
        ..Usage::default()
    }
}

fn response(usage: Usage) -> ModelResponse {
    ModelResponse {
        id: "response".into(),
        content: vec![ResponseContent::Text("done".into())],
        stop_reason: Some(StopReason::EndTurn),
        usage,
        metadata: ResponseMetadata::default(),
    }
}

struct ScriptedProvider {
    events: std::sync::Mutex<Option<Vec<Result<StreamEvent>>>>,
    pending: bool,
}

#[async_trait]
impl ModelProviderSDK for ScriptedProvider {
    async fn completion(&self, _: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("failed attempt")
    }
    async fn completion_stream(
        &self,
        _: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let events = self.events.lock().unwrap().take().unwrap_or_default();
        if self.pending {
            Ok(Box::pin(stream::iter(events).chain(stream::pending())))
        } else {
            Ok(Box::pin(stream::iter(events)))
        }
    }
    fn name(&self) -> &str {
        "scripted"
    }
}

fn instrument(
    f: &Fixture,
    turn: TurnId,
    events: Vec<Result<StreamEvent>>,
    pending: bool,
) -> Arc<dyn ModelProviderSDK> {
    f.ledger.instrumented_provider(
        Arc::new(ScriptedProvider {
            events: std::sync::Mutex::new(Some(events)),
            pending,
        }),
        f.session.id,
        Some(turn),
        UsagePurpose::TurnQuery,
    )
}

#[tokio::test]
async fn each_final_response_is_durable_before_return_and_restart_before_turn_end() {
    let f = Fixture::new();
    let turn = TurnId::new();
    for usage in [reported(100, 20), reported(200, 30)] {
        let provider = instrument(
            &f,
            turn,
            vec![
                Ok(StreamEvent::UsageDelta(usage.clone())),
                Ok(StreamEvent::UsageDelta(usage.clone())),
                Ok(StreamEvent::MessageDone {
                    response: response(usage),
                }),
            ],
            false,
        );
        let mut stream = provider.completion_stream(request()).await.unwrap();
        stream.next().await.unwrap().unwrap();
        stream.next().await.unwrap().unwrap();
        // Samples are provisional: they never inflate committed spend.
        let before = f
            .ledger
            .session_usage(f.session.id)
            .map(|u| u.total.call_count)
            .unwrap_or(0);
        stream.next().await.unwrap().unwrap();
        let replay = f.replay();
        assert_eq!(
            replay.session_usage(f.session.id),
            f.ledger.session_usage(f.session.id)
        );
        assert_eq!(
            replay.session_usage(f.session.id).unwrap().total.call_count,
            before + 1
        );
        assert!(stream.next().await.is_none());
    }
    let expected = UsageTotals {
        input_tokens: 300,
        output_tokens: 50,
        total_tokens: 350,
        call_count: 2,
        metered_call_count: 2,
        ..UsageTotals::default()
    };
    assert_eq!(
        f.ledger.session_usage(f.session.id).unwrap().total,
        expected
    );
    assert_eq!(
        f.ledger.turn_usage(f.session.id, turn),
        TurnUsage {
            query: expected,
            overhead: UsageTotals::default()
        }
    );
    let raw = std::fs::read_to_string(&f.session.rollout_path).unwrap();
    assert!(
        !raw.contains("\"kind\":\"turn\""),
        "there is no completed-turn record to recover from"
    );
}

#[tokio::test]
async fn missing_reported_zero_error_cancel_and_retry_have_distinct_accounting() {
    let f = Fixture::new();
    let turn = TurnId::new();
    for usage in [Usage::default(), reported(0, 0)] {
        let provider = instrument(
            &f,
            turn,
            vec![Ok(StreamEvent::MessageDone {
                response: response(usage),
            })],
            false,
        );
        provider
            .completion_stream(request())
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;
    }
    let failing = instrument(
        &f,
        turn,
        vec![
            Ok(StreamEvent::UsageDelta(reported(10, 2))),
            Err(anyhow::anyhow!("stream failure")),
        ],
        false,
    );
    failing
        .completion_stream(request())
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    assert!(failing.completion(request()).await.is_err());
    let cancelled = instrument(
        &f,
        turn,
        vec![Ok(StreamEvent::UsageDelta(reported(20, 3)))],
        true,
    );
    let mut stream = cancelled.completion_stream(request()).await.unwrap();
    stream.next().await.unwrap().unwrap();
    drop(stream);
    // Empty EOF is a failed attempt, not a successful zero-token response.
    instrument(&f, turn, vec![], false)
        .completion_stream(request())
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    let expected = UsageTotals {
        input_tokens: 30,
        output_tokens: 5,
        total_tokens: 35,
        unreported_fields: Some(devo_protocol::native::usage::UsageField::ALL.to_vec()),
        call_count: 6,
        metered_call_count: 3,
        failed_call_count: 3,
        cancelled_call_count: 1,
        ..UsageTotals::default()
    };
    assert_eq!(
        f.ledger.session_usage(f.session.id).unwrap().total,
        expected
    );
    assert_eq!(
        f.replay().session_usage(f.session.id),
        f.ledger.session_usage(f.session.id)
    );
}

#[test]
fn records_are_idempotent_and_overhead_is_counted_without_changing_context() {
    let f = Fixture::new();
    let turn = TurnId::new();
    f.record(f.session.id, turn, reported(100, 20), None);
    let query_context = f.ledger.latest_query(f.session.id);
    for purpose in [
        UsagePurpose::AutoReview,
        UsagePurpose::Compaction,
        UsagePurpose::TitleGeneration,
        UsagePurpose::Refine,
        UsagePurpose::PythonCellWatch,
    ] {
        let context = RuntimeCallContext::new(
            f.session.id,
            (purpose != UsagePurpose::TitleGeneration).then_some(turn),
            purpose,
        );
        let id = Uuid::now_v7().to_string();
        for _ in 0..2 {
            f.ledger
                .record(
                    &id,
                    &context,
                    "test",
                    &request(),
                    UsageCallOutcome::Succeeded,
                    Some(&reported(1, 2)),
                )
                .unwrap();
        }
    }
    assert_eq!(
        f.ledger.session_usage(f.session.id).unwrap().total,
        UsageTotals {
            input_tokens: 105,
            output_tokens: 30,
            total_tokens: 135,
            call_count: 6,
            metered_call_count: 6,
            ..UsageTotals::default()
        }
    );
    assert_eq!(
        f.ledger.turn_usage(f.session.id, turn).overhead,
        UsageTotals {
            input_tokens: 4,
            output_tokens: 8,
            total_tokens: 12,
            call_count: 4,
            metered_call_count: 4,
            ..UsageTotals::default()
        }
    );
    assert_eq!(f.ledger.latest_query(f.session.id), query_context);
    assert_eq!(
        f.replay().session_usage(f.session.id),
        f.ledger.session_usage(f.session.id)
    );
}

#[test]
fn shared_journal_late_child_and_grandchild_keep_cumulative_and_local_context() {
    let f = Fixture::new();
    let root = f.session.id;
    let child = SessionId::new();
    let grandchild = SessionId::new();
    let first = TurnId::new();
    let second = TurnId::new();
    let child_turn = TurnId::new();
    for (id, parent) in [(child, root), (grandchild, child)] {
        let mut session = f.session.clone();
        session.id = id;
        session.parent_session_id = Some(parent);
        session.agent_path = Some(format!("agent-{id}"));
        f.ledger
            .rollout_store
            .append_session_meta(&session)
            .unwrap();
        let row = crate::persistence::session_index_row_from_record(&session, session.created_at);
        f.ledger
            .db
            .upsert_session(row, Some(&session.rollout_path))
            .unwrap();
    }
    f.record(root, first, reported(100, 0), None);
    f.record(
        child,
        child_turn,
        reported(10, 0),
        Some(UsageAttribution {
            session_id: root,
            turn_id: Some(first),
        }),
    );
    f.record(root, second, reported(200, 0), None);
    assert_eq!(
        f.ledger.session_usage(root).unwrap().total.total_tokens,
        310
    );
    f.record(
        child,
        child_turn,
        reported(10, 0),
        Some(UsageAttribution {
            session_id: root,
            turn_id: Some(first),
        }),
    );
    assert_eq!(
        f.ledger.session_usage(root).unwrap().total.total_tokens,
        320
    );
    assert_eq!(f.ledger.turn_usage(root, first).query.total_tokens, 120);
    assert_eq!(f.ledger.turn_usage(root, second).query.total_tokens, 200);
    assert_eq!(
        f.ledger.turn_usage(child, child_turn).query.total_tokens,
        20
    );
    assert_eq!(f.ledger.latest_query(root).unwrap().query.total_tokens, 200);
    f.record(root, TurnId::new(), reported(100_000, 1000), None);
    f.record(
        child,
        child_turn,
        reported(1000, 100),
        Some(UsageAttribution {
            session_id: root,
            turn_id: Some(first),
        }),
    );
    assert_eq!(
        f.ledger.latest_query(root).unwrap().query.total_tokens,
        101_000
    );
    assert_eq!(
        f.ledger.latest_query(child).unwrap().query.total_tokens,
        1100
    );
    f.record(
        grandchild,
        TurnId::new(),
        reported(3, 1),
        Some(UsageAttribution {
            session_id: child,
            turn_id: Some(child_turn),
        }),
    );
    assert_eq!(f.ledger.turn_usage(root, first).query.total_tokens, 1224);
    let replay = f.replay();
    for id in [root, child, grandchild] {
        assert_eq!(replay.session_usage(id), f.ledger.session_usage(id));
        assert_eq!(replay.latest_query(id), f.ledger.latest_query(id));
    }
}

#[path = "completion_tests.rs"]
mod completion_tests;
#[path = "replay_tests.rs"]
mod replay_tests;

#[path = "legacy_boundary_tests.rs"]
mod legacy_boundary_tests;
