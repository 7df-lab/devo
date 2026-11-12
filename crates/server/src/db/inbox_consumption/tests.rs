use super::*;
use pretty_assertions::assert_eq;

fn setup() -> (tempfile::TempDir, Database, SessionId) {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path().join("consume.db")).unwrap();
    let session = SessionId::new();
    db.conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO sessions(id,cwd,created_at,updated_at) VALUES (?1,'.',0,0)",
            params![session.as_str()],
        )
        .unwrap();
    (dir, db, session)
}
fn item() -> PendingInputItem {
    PendingInputItem::new(
        PendingInputKind::UserText {
            text: "done".into(),
        },
        None,
        chrono::Utc::now(),
    )
}

#[test]
fn consume_before_admission_is_durable_idempotent_and_session_scoped() {
    let (dir, db, session) = setup();
    let completion = item();
    let key = completion.admission_key();
    assert_eq!(
        db.consume_inbox_completion(&session, &key, &completion.id)
            .unwrap(),
        InboxConsumption::Consumed
    );
    assert_eq!(
        db.consume_inbox_completion(&session, &key, &completion.id)
            .unwrap(),
        InboxConsumption::AlreadyConsumed
    );
    assert!(!db.admit_inbox(&session, &completion).unwrap());
    drop(db);
    let db = Database::open(dir.path().join("consume.db")).unwrap();
    assert!(!db.admit_inbox(&session, &completion).unwrap());
    assert_eq!(
        db.consume_inbox_completion(&session, &key, &completion.id)
            .unwrap(),
        InboxConsumption::AlreadyConsumed
    );
    let other = SessionId::new();
    db.conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO sessions(id,cwd,created_at,updated_at) VALUES (?1,'.',0,0)",
            params![other.as_str()],
        )
        .unwrap();
    assert!(db.admit_inbox(&other, &completion).unwrap());
}

#[test]
fn retract_pending_preserves_fifo_and_claimed_items_are_not_retracted() {
    let (_dir, db, session) = setup();
    let first = item();
    let middle = item();
    let last = item();
    for input in [&first, &middle, &last] {
        assert!(db.admit_inbox(&session, input).unwrap());
    }
    assert_eq!(
        db.consume_inbox_completion(&session, &middle.admission_key(), &middle.id)
            .unwrap(),
        InboxConsumption::Consumed
    );
    assert_eq!(
        db.list_pending(&session, QueueType::Turn)
            .unwrap()
            .iter()
            .map(|i| i.id)
            .collect::<Vec<_>>(),
        vec![first.id, last.id]
    );
    assert!(!db.admit_inbox(&session, &middle).unwrap());
    db.claim_queue_steer(&session, &first, &TurnId::new())
        .unwrap();
    assert_eq!(
        db.consume_inbox_completion(&session, &first.admission_key(), &first.id)
            .unwrap(),
        InboxConsumption::AlreadyDelivered
    );
    db.recover_interrupted_queue_steers(&session).unwrap();
    assert_eq!(
        db.consume_inbox_completion(&session, &first.admission_key(), &first.id)
            .unwrap(),
        InboxConsumption::AlreadyDelivered
    );
    assert_eq!(
        db.list_pending(&session, QueueType::Turn)
            .unwrap()
            .iter()
            .map(|i| i.id)
            .collect::<Vec<_>>(),
        vec![first.id, last.id]
    );
}

#[test]
fn database_failure_rolls_back_tombstone_and_withdrawal() {
    let (_dir, db, session) = setup();
    let completion = item();
    assert!(db.admit_inbox(&session, &completion).unwrap());
    let before = serde_json::to_value(db.list_pending(&session, QueueType::Turn).unwrap()).unwrap();
    db.conn.lock().unwrap().execute_batch("CREATE TRIGGER reject_completion_delete BEFORE DELETE ON pending_messages BEGIN SELECT RAISE(ABORT,'test failure'); END;").unwrap();
    assert!(
        db.consume_inbox_completion(&session, &completion.admission_key(), &completion.id)
            .is_err()
    );
    assert_eq!(
        serde_json::to_value(db.list_pending(&session, QueueType::Turn).unwrap()).unwrap(),
        before
    );
    db.conn
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_completion_delete;")
        .unwrap();
    assert_eq!(
        db.consume_inbox_completion(&session, &completion.admission_key(), &completion.id)
            .unwrap(),
        InboxConsumption::Consumed
    );
}
