use super::*;
use pretty_assertions::assert_eq;

pub(super) fn legacy_turn(session: SessionId, turn_id: TurnId, input: u64) -> RolloutLine {
    use devo_protocol::native::turn::{Turn, TurnKind, TurnStatus};
    let now = Utc::now();
    RolloutLine::Turn {
        v: 2,
        timestamp: now,
        extras: None,
        turn: Box::new(Turn {
            id: turn_id,
            session_id: session,
            sequence: 1,
            kind: TurnKind::Regular,
            status: TurnStatus::Completed,
            model: ModelBinding {
                provider: "test".into(),
                model: "test".into(),
                variant: None,
                reasoning_effort: None,
            },
            collaboration_mode: None,
            started_at: now,
            completed_at: Some(now),
            error: None,
            usage: Some(TurnUsage {
                query: UsageTotals {
                    input_tokens: input,
                    total_tokens: input,
                    ..UsageTotals::default()
                },
                overhead: UsageTotals::default(),
            }),
        }),
    }
}

#[test]
fn legacy_baseline_survives_resume_without_counting_new_turn_summary_twice() {
    let f = Fixture::new();
    let session = f.session.id;
    let old = TurnId::new();
    // Old call details cannot be invented. The pre-record turn is a baseline.
    f.ledger
        .rollout_store
        .append_rollout_lines(&f.session.rollout_path, vec![legacy_turn(session, old, 50)])
        .unwrap();
    f.ledger.load_session(session).unwrap();
    f.record(session, old, reported(20, 3), None);
    f.ledger
        .rollout_store
        .append_rollout_lines(&f.session.rollout_path, vec![legacy_turn(session, old, 73)])
        .unwrap();
    let next = TurnId::new();
    f.record(session, next, reported(10, 2), None);
    f.ledger
        .rollout_store
        .append_rollout_lines(
            &f.session.rollout_path,
            vec![legacy_turn(session, next, 12)],
        )
        .unwrap();
    let replay = f.replay();
    let expected = UsageTotals {
        unreported_fields: Some(
            devo_protocol::native::usage::UsageField::ALL
                .into_iter()
                .filter(|field| *field != devo_protocol::native::usage::UsageField::TotalTokens)
                .collect(),
        ),
        input_tokens: 80,
        output_tokens: 5,
        total_tokens: 85,
        call_count: 2,
        metered_call_count: 2,
        ..UsageTotals::default()
    };
    assert_eq!(replay.session_usage(session).unwrap().total, expected);
    assert_eq!(
        replay.session_usage(session).unwrap().legacy,
        Some(UsageTotals {
            unreported_fields: Some(
                devo_protocol::native::usage::UsageField::ALL
                    .into_iter()
                    .filter(|field| *field != devo_protocol::native::usage::UsageField::TotalTokens)
                    .collect()
            ),
            input_tokens: 50,
            total_tokens: 50,
            ..UsageTotals::default()
        })
    );
    assert_eq!(replay.turn_usage(session, old).query.total_tokens, 73);
    // Context rollback and supersession do not erase billed calls.
    f.ledger
        .rollout_store
        .append_rollout_lines(
            &f.session.rollout_path,
            vec![RolloutLine::SessionRollback {
                v: 2,
                timestamp: Utc::now(),
                session_id: session,
                retained_turn_ids: vec![],
                retained_item_ids: vec![],
                latest_turn_id: None,
            }],
        )
        .unwrap();
    assert_eq!(f.replay().session_usage(session).unwrap().total, expected);
}

#[test]
fn concurrent_attempts_replay_exactly_once_with_provider_totals_and_cache_subsets() {
    let f = Fixture::new();
    let session = f.session.id;
    let turn = TurnId::new();
    std::thread::scope(|scope| {
        for _ in 0..12 {
            scope.spawn(|| {
                f.record(
                    session,
                    turn,
                    Usage {
                        terminal_sample: false,
                        unreported_fields: Some(Vec::new()),
                        reported: true,
                        input_tokens: 100,
                        output_tokens: 10,
                        total_tokens: Some(115),
                        reasoning_output_tokens: Some(5),
                        cache_read_input_tokens: Some(30),
                        cache_creation_input_tokens: Some(20),
                    },
                    None,
                )
            });
        }
    });
    let expected = UsageTotals {
        input_tokens: 1200,
        output_tokens: 120,
        total_tokens: 1380,
        reasoning_tokens: 60,
        cache_read_input_tokens: 360,
        cache_creation_input_tokens: 240,
        call_count: 12,
        metered_call_count: 12,
        ..UsageTotals::default()
    };
    assert_eq!(f.ledger.session_usage(session).unwrap().total, expected);
    assert_eq!(f.replay().session_usage(session).unwrap().total, expected);
}

#[tokio::test]
async fn final_without_usage_keeps_last_reported_sample_and_unknown_total_keeps_context() {
    use devo_protocol::native::usage::UsageField;
    let f = Fixture::new();
    let turn = TurnId::new();
    let provider = instrument(
        &f,
        turn,
        vec![
            Ok(StreamEvent::UsageDelta(reported(100, 20))),
            Ok(StreamEvent::MessageDone {
                response: response(Usage::default()),
            }),
        ],
        false,
    );
    let events = provider
        .completion_stream(request())
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    assert!(
        matches!(events.last().unwrap().as_ref().unwrap(), StreamEvent::MessageDone { response } if response.usage == reported(100, 20))
    );
    assert_eq!(
        f.ledger
            .session_usage(f.session.id)
            .unwrap()
            .total
            .total_tokens,
        120
    );
    let context = f.ledger.latest_query(f.session.id);
    let partial = Usage {
        reported: true,
        ..Usage::default()
    }
    .with_field_coverage(Some(0), None);
    f.record(f.session.id, turn, partial, None);
    let total = f.ledger.session_usage(f.session.id).unwrap().total;
    assert_eq!((total.call_count, total.metered_call_count), (2, 2));
    assert!(
        total
            .unreported_fields
            .unwrap()
            .contains(&UsageField::TotalTokens)
    );
    assert_eq!(f.ledger.latest_query(f.session.id), context);
    assert_eq!(f.replay().latest_query(f.session.id), context);
}

#[test]
fn legacy_ancestor_inclusive_baseline_does_not_double_count_child_lumps_or_old_records() {
    let f = Fixture::new();
    let root = f.session.id;
    let child = SessionId::new();
    let mut session = f.session.clone();
    session.id = child;
    session.parent_session_id = Some(root);
    session.agent_path = Some("child".into());
    f.ledger
        .rollout_store
        .append_session_meta(&session)
        .unwrap();
    let row = crate::persistence::session_index_row_from_record(&session, session.created_at);
    f.ledger
        .db
        .upsert_session(row, Some(&session.rollout_path))
        .unwrap();
    let child_turn = TurnId::new();
    f.record(child, child_turn, reported(20, 0), None);
    f.ledger
        .rollout_store
        .append_rollout_lines(
            &session.rollout_path,
            vec![legacy_turn(child, child_turn, 20)],
        )
        .unwrap();
    // Root is from before query instrumentation, with child spend already in it.
    f.ledger
        .rollout_store
        .append_rollout_lines(
            &session.rollout_path,
            vec![legacy_turn(root, TurnId::new(), 120)],
        )
        .unwrap();
    let replay = f.replay();
    assert_eq!(replay.session_usage(root).unwrap().total.total_tokens, 120);
    assert_eq!(
        replay
            .session_usage(root)
            .unwrap()
            .legacy
            .unwrap()
            .total_tokens,
        100
    );
    assert_eq!(replay.session_usage(child).unwrap().total.total_tokens, 20);

    let second_child = SessionId::new();
    session.id = second_child;
    f.ledger
        .rollout_store
        .append_session_meta(&session)
        .unwrap();
    let mut old_child = legacy_turn(second_child, TurnId::new(), 30);
    if let RolloutLine::Turn { turn, .. } = &mut old_child {
        turn.started_at = session.created_at;
    }
    f.ledger
        .rollout_store
        .append_rollout_lines(&session.rollout_path, vec![old_child])
        .unwrap();
    // The unmetered historical child lump is covered by the opaque root baseline,
    // even if its terminal journal entry landed later. Never add it twice.
    assert_eq!(
        f.replay().session_usage(root).unwrap().total.total_tokens,
        120
    );
    assert_eq!(
        f.replay()
            .session_usage(second_child)
            .unwrap()
            .total
            .total_tokens,
        30
    );
}

#[test]
fn resumed_child_attempt_keeps_original_turn_owner_after_new_parent_message() {
    let f = Fixture::new();
    let original = UsageAttribution {
        session_id: SessionId::new(),
        turn_id: Some(TurnId::new()),
    };
    let child_turn = TurnId::new();
    f.record(
        f.session.id,
        child_turn,
        reported(10, 0),
        Some(original.clone()),
    );
    let replay = f.replay();
    replay.register_owner(
        f.session.id,
        UsageAttribution {
            session_id: original.session_id,
            turn_id: Some(TurnId::new()),
        },
    );
    let guard = CallAttemptGuard::new(
        replay,
        RuntimeCallContext::new(f.session.id, Some(child_turn), UsagePurpose::TurnQuery),
        "test".into(),
        request(),
    );
    assert_eq!(guard.context.parent, Some(original));
    drop(guard);
}

#[test]
fn overhead_only_terminal_summary_is_not_a_second_charge_after_restart() {
    let f = Fixture::new();
    let session = f.session.id;
    let turn = TurnId::new();
    f.ledger
        .record(
            "compact-attempt",
            &RuntimeCallContext::new(session, Some(turn), UsagePurpose::Compaction),
            "test",
            &request(),
            UsageCallOutcome::Succeeded,
            Some(&reported(30, 4)),
        )
        .unwrap();
    let mut terminal = legacy_turn(session, turn, 0);
    if let RolloutLine::Turn { turn, .. } = &mut terminal {
        turn.usage = Some(f.ledger.turn_usage(session, turn.id));
        turn.status = devo_protocol::native::turn::TurnStatus::Interrupted;
    }
    f.ledger
        .rollout_store
        .append_rollout_lines(&f.session.rollout_path, vec![terminal])
        .unwrap();
    let replay = f.replay();
    assert_eq!(
        replay.session_usage(session).unwrap(),
        f.ledger.session_usage(session).unwrap()
    );
    assert_eq!(replay.turn_usage(session, turn).overhead.total_tokens, 34);
    assert_eq!(replay.session_usage(session).unwrap().legacy, None);
}

#[test]
fn reader_replays_new_calls_after_another_ledger_appends_to_the_journal() {
    let f = Fixture::new();
    f.record(f.session.id, TurnId::new(), reported(10, 2), None);
    let reader = f.replay();
    assert_eq!(
        reader
            .session_usage(f.session.id)
            .unwrap()
            .total
            .total_tokens,
        12
    );
    f.record(f.session.id, TurnId::new(), reported(20, 3), None);
    reader.load_session(f.session.id).unwrap();
    assert_eq!(
        reader
            .session_usage(f.session.id)
            .unwrap()
            .total
            .total_tokens,
        35
    );
    reader.load_session(f.session.id).unwrap();
    assert_eq!(
        reader.session_usage(f.session.id).unwrap().total.call_count,
        2
    );
}

#[test]
fn old_fork_journal_copied_turn_usage_is_context_not_new_spend() {
    let f = Fixture::new();
    let mut projection = projection::Projection::default();
    let mut session =
        crate::persistence::session_index_row_from_record(&f.session, f.session.created_at)
            .into_native_session();
    session.fork_from_id = Some(SessionId::new());
    session.created_at = Utc::now() + chrono::Duration::seconds(1);
    projection.observe(RolloutLine::SessionMeta {
        v: 2,
        timestamp: session.created_at,
        session: Box::new(session),
        extras: None,
    });
    projection.observe(legacy_turn(f.session.id, TurnId::new(), 100));
    assert_eq!(
        projection.session(f.session.id).unwrap().total,
        UsageTotals::default()
    );
}

#[test]
fn identical_query_counters_after_compaction_are_a_new_context_anchor() {
    let f = Fixture::new();
    let session = f.session.id;
    f.record(session, TurnId::new(), reported(100, 20), None);
    f.ledger
        .rollout_store
        .append_rollout_lines(
            &f.session.rollout_path,
            vec![RolloutLine::CompactionSnapshot {
                v: 2,
                timestamp: Utc::now(),
                session_id: session,
                turn_id: TurnId::new(),
                summary_item_id: devo_protocol::native::ids::ItemId::new(),
                preserved_item_ids: Vec::new(),
                context_occupancy: None,
            }],
        )
        .unwrap();
    let reader = f.replay();
    assert!(!reader.query_after_context_checkpoint(session));
    f.record(session, TurnId::new(), reported(100, 20), None);
    reader.load_session(session).unwrap();
    assert!(reader.query_after_context_checkpoint(session));
}

#[test]
fn total_only_call_does_not_erase_the_last_known_input_on_replay() {
    let f = Fixture::new();
    f.record(f.session.id, TurnId::new(), reported(100, 20), None);
    f.record(
        f.session.id,
        TurnId::new(),
        Usage {
            reported: true,
            total_tokens: Some(90),
            ..Usage::default()
        }
        .with_field_coverage(None, None),
        None,
    );
    let replay = f.replay();
    assert_eq!(
        replay
            .latest_query(f.session.id)
            .unwrap()
            .display_total_tokens(),
        90
    );
    assert_eq!(replay.latest_input(f.session.id), Some(100));
}
