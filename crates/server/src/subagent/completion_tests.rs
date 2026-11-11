use super::*;
use pretty_assertions::assert_eq;

fn terminal(child: SessionId, turn: devo_protocol::TurnId, text: &str) -> AgentOutputEvent {
    AgentOutputEvent {
        sequence: 0,
        child_session_id: child,
        agent_path: "root/worker".into(),
        turn_id: Some(turn),
        kind: AgentOutputEventKind::Status,
        text: Some(text.into()),
        status: Some("completed".into()),
        created_at: Utc::now(),
    }
}

fn snapshot(status: &AgentOutputEvent, sequence: u64) -> AgentOutputEvent {
    AgentOutputEvent {
        sequence,
        kind: AgentOutputEventKind::AssistantMessage,
        status: None,
        ..status.clone()
    }
}

#[tokio::test]
async fn completed_snapshot_supplies_output_when_provider_emits_no_deltas() {
    let buffer = SubagentOutputBuffer::new();
    let child = SessionId::new();
    let status = terminal(child, devo_protocol::TurnId::new(), "complete answer");
    buffer.push(status.clone()).await;
    let (events, next_sequence, timed_out) = buffer
        .wait_after(
            /*after_sequence*/ 0,
            &[child],
            Duration::ZERO,
            /*cancel*/ None,
        )
        .await;
    assert_eq!(
        (events, next_sequence, timed_out),
        (
            vec![
                snapshot(&status, /*sequence*/ 1),
                AgentOutputEvent {
                    sequence: 2,
                    text: None,
                    ..status
                },
            ],
            3,
            false,
        )
    );
}

#[tokio::test]
async fn completed_snapshot_repairs_deltas_dropped_while_buffer_is_locked() {
    let buffer = SubagentOutputBuffer::new();
    let child = SessionId::new();
    let status = terminal(child, devo_protocol::TurnId::new(), "alpha beta");
    let mut delta = snapshot(&status, /*sequence*/ 0);
    delta.text = Some("alpha ".into());
    assert!(buffer.try_push_text_delta(delta.clone()));
    let guard = buffer.inner.lock().await;
    delta.text = Some("beta".into());
    assert!(!buffer.try_push_text_delta(delta));
    drop(guard);
    buffer.push(status.clone()).await;
    let (events, next_sequence, timed_out) = buffer
        .wait_after(
            /*after_sequence*/ 0,
            &[child],
            Duration::ZERO,
            /*cancel*/ None,
        )
        .await;
    assert_eq!(
        (events, next_sequence, timed_out),
        (
            vec![
                snapshot(&status, /*sequence*/ 1),
                AgentOutputEvent {
                    sequence: 2,
                    text: None,
                    ..status
                },
            ],
            3,
            false,
        )
    );
}

#[tokio::test]
async fn completed_snapshot_repairs_interleaved_output_after_an_observed_cursor() {
    let buffer = SubagentOutputBuffer::new();
    let child = SessionId::new();
    let sibling = SessionId::new();
    let status = terminal(child, devo_protocol::TurnId::new(), "full child answer");
    let sibling_status = terminal(sibling, devo_protocol::TurnId::new(), "sibling answer");
    let mut fragment = snapshot(&status, /*sequence*/ 0);
    fragment.text = Some("partial ".into());
    assert!(buffer.try_push_text_delta(fragment.clone()));
    assert!(buffer.try_push_text_delta(snapshot(&sibling_status, /*sequence*/ 0)));
    fragment.text = Some("child output".into());
    assert!(buffer.try_push_text_delta(fragment));
    buffer.push(sibling_status.clone()).await;
    let (observed, next_sequence, timed_out) = buffer
        .wait_after(
            /*after_sequence*/ 0,
            &[child, sibling],
            Duration::ZERO,
            /*cancel*/ None,
        )
        .await;
    assert_eq!((observed.len(), next_sequence, timed_out), (4, 5, false));
    let cursor = observed.last().expect("sibling terminal").sequence;

    buffer.push(status.clone()).await;
    let (events, next_sequence, timed_out) = buffer
        .wait_after(cursor, &[child], Duration::ZERO, /*cancel*/ None)
        .await;
    assert_eq!(
        (events, next_sequence, timed_out),
        (
            vec![
                snapshot(&status, /*sequence*/ 5),
                AgentOutputEvent {
                    sequence: 6,
                    text: None,
                    ..status.clone()
                },
            ],
            7,
            false,
        )
    );
    let (history, _, _) = buffer
        .wait_after(
            /*after_sequence*/ 0,
            &[child],
            Duration::ZERO,
            /*cancel*/ None,
        )
        .await;
    assert_eq!(
        history,
        vec![
            snapshot(&status, /*sequence*/ 5),
            AgentOutputEvent {
                sequence: 6,
                text: None,
                ..status
            },
        ],
        "the full history must not retain duplicate partial snapshots"
    );
}
