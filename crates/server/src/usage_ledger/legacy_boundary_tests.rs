use super::*;
use pretty_assertions::assert_eq;

fn dated_record(
    session: SessionId,
    turn: TurnId,
    input: usize,
    parent: Option<UsageAttribution>,
    at: chrono::DateTime<Utc>,
) -> RolloutLine {
    RolloutLine::Internal {
        v: 2,
        timestamp: at,
        session_id: session,
        turn_id: Some(turn),
        seq: 0,
        entry: devo_core::InternalRecord::UsageRecord {
            record: UsageRecord {
                call_id: Uuid::now_v7().to_string(),
                session_id: session,
                turn_id: Some(turn),
                parent,
                purpose: UsagePurpose::TurnQuery,
                model: ModelBinding {
                    provider: "test".into(),
                    model: "test".into(),
                    variant: None,
                    reasoning_effort: None,
                },
                outcome: UsageCallOutcome::Succeeded,
                usage: Some(token_usage(&reported(input, 0))),
                estimated_cost: None,
                recorded_at: at,
            },
        },
    }
}

fn at(mut line: RolloutLine, timestamp: chrono::DateTime<Utc>) -> RolloutLine {
    if let RolloutLine::Turn {
        turn,
        timestamp: time,
        ..
    } = &mut line
    {
        *time = timestamp;
        turn.started_at = timestamp;
    }
    line
}

#[test]
fn resumed_legacy_baseline_keeps_original_child_coverage_boundary() {
    let f = Fixture::new();
    let root = f.session.id;
    let child = SessionId::new();
    let root_turn = TurnId::new();
    let child_turn = TurnId::new();
    let t1 = Utc::now() - chrono::Duration::seconds(5);
    let t2 = t1 + chrono::Duration::seconds(1);
    let t3 = t2 + chrono::Duration::seconds(1);
    let t4 = t3 + chrono::Duration::seconds(1);
    let mut projection = projection::Projection::default();
    projection.register_session(child, Some(root), t1);
    projection.observe(at(
        super::replay_tests::legacy_turn(root, root_turn, 50),
        t1,
    ));
    projection.observe(dated_record(child, child_turn, 20, None, t2));
    projection.observe(at(
        super::replay_tests::legacy_turn(child, child_turn, 20),
        t2,
    ));
    projection.observe(dated_record(root, root_turn, 23, None, t3));
    projection.observe(at(
        super::replay_tests::legacy_turn(root, root_turn, 73),
        t4,
    ));
    let usage = projection.session(root).unwrap();
    assert_eq!(usage.total.total_tokens, 93);
    assert_eq!(usage.total.call_count, 2);
    assert_eq!(usage.legacy.unwrap().total_tokens, 50);
}

#[test]
fn independent_child_journal_order_cannot_hide_an_older_root_baseline() {
    let temp = TempDir::new().unwrap();
    let root = SessionId::new();
    let child = SessionId::new();
    let turn = TurnId::new();
    let t1 = Utc::now() - chrono::Duration::seconds(2);
    let parent = at(super::replay_tests::legacy_turn(root, turn, 50), t1);
    let child_record = dated_record(
        child,
        TurnId::new(),
        20,
        Some(UsageAttribution {
            session_id: root,
            turn_id: Some(turn),
        }),
        t1 + chrono::Duration::seconds(1),
    );
    let root_path = temp.path().join("root.jsonl");
    let child_path = temp.path().join("child.jsonl");
    std::fs::write(
        &root_path,
        format!(
            "{}\n{}\n",
            serde_json::to_string(&parent).unwrap(),
            serde_json::to_string(&at(
                super::replay_tests::legacy_turn(root, turn, 70),
                t1 + chrono::Duration::seconds(2)
            ))
            .unwrap()
        ),
    )
    .unwrap();
    std::fs::write(
        &child_path,
        format!("{}\n", serde_json::to_string(&child_record).unwrap()),
    )
    .unwrap();
    let mut results = Vec::new();
    for paths in [[&root_path, &child_path], [&child_path, &root_path]] {
        let mut projection = projection::Projection::default();
        for path in paths {
            journal::JournalCursor::default()
                .read_new(path, |line| projection.observe(line))
                .unwrap();
        }
        let usage = projection.session(root).unwrap();
        assert_eq!(usage.total.total_tokens, 70);
        assert_eq!(usage.total.call_count, 1);
        results.push(usage);
    }
    assert_eq!(results[0], results[1]);
}
