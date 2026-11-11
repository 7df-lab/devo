//! Inbox admission receipts survive consumption, so retries cannot re-admit an event.
use super::*;

impl Database {
    pub fn inbox_receipt(
        &self,
        session_id: &SessionId,
        key: &str,
    ) -> Result<Option<(QueueItemId, Option<TurnId>)>> {
        use rusqlite::OptionalExtension;
        let conn = self.conn.lock().expect("database mutex poisoned");
        let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='inbox_receipts')", [], |row| row.get(0))?;
        if !exists {
            return Ok(None);
        }
        Ok(conn.query_row("SELECT r.pending_input_id, c.turn_id FROM inbox_receipts r LEFT JOIN queue_steer_receipts c ON r.session_id=c.session_id AND r.pending_input_id=c.pending_input_id WHERE r.session_id=?1 AND r.event_key=?2",
            params![session_id.as_str(), key], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)))
            .optional()?.map(|(item, turn)| (QueueItemId::from_string(item), turn.map(TurnId::from_string))))
    }

    pub fn admit_inbox(&self, session_id: &SessionId, item: &PendingInputItem) -> Result<bool> {
        let mut conn = self.conn.lock().expect("database mutex poisoned");
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS inbox_receipts (
            session_id TEXT NOT NULL, event_key TEXT NOT NULL,
            pending_input_id TEXT NOT NULL, PRIMARY KEY(session_id, event_key)
        )",
        )?;
        let event_key = item.admission_key();
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO inbox_receipts VALUES (?1,?2,?3)",
            params![session_id.as_str(), event_key, item.id.to_string()],
        )?;
        if inserted == 0 {
            return Ok(false);
        }
        let (kind, content) = pending_kind_parts(&item.kind);
        tx.execute("INSERT INTO pending_messages
            (session_id,queue_type,kind,content,pending_input_id,metadata,created_at,position)
            SELECT ?1,'turn',?2,?3,?4,?5,?6,
            (SELECT COALESCE(MAX(position),0)+1 FROM pending_messages WHERE session_id=?1 AND queue_type IN ('turn','steer'))",
            params![session_id.as_str(),kind,content,item.id.to_string(),
                item.metadata.as_ref().map(serde_json::Value::to_string),item.created_at.timestamp()])?;
        tx.commit()?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devo_protocol::PendingInputKind;
    use pretty_assertions::assert_eq;

    #[test]
    fn admission_after_claim_preserves_fifo_on_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path().join("order.db")).unwrap();
        let session = SessionId::new();
        db.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO sessions (id,cwd,created_at,updated_at) VALUES (?1,'.',0,0)",
                params![session.as_str()],
            )
            .unwrap();
        let item = |text: &str| {
            PendingInputItem::new(
                PendingInputKind::UserText { text: text.into() },
                None,
                chrono::Utc::now(),
            )
        };
        let first = item("first");
        let middle = item("middle");
        db.admit_inbox(&session, &first).unwrap();
        db.admit_inbox(&session, &middle).unwrap();
        db.remove_pending_by_id(&session, QueueType::Turn, &first.id)
            .unwrap();
        db.claim_queue_steer(&session, &middle, &TurnId::new())
            .unwrap();
        let last = item("last");
        db.admit_inbox(&session, &last).unwrap();
        db.recover_interrupted_queue_steers(&session).unwrap();
        assert_eq!(
            db.list_pending(&session, QueueType::Turn)
                .unwrap()
                .iter()
                .map(|item| item.id)
                .collect::<Vec<_>>(),
            vec![middle.id, last.id]
        );
    }

    #[test]
    fn inbox_receipts_survive_consumption_and_restart_without_reordering() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("inbox.db");
        let db = Database::open(path.clone()).unwrap();
        let session = SessionId::new();
        db.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO sessions (id,cwd,created_at,updated_at) VALUES (?1,'.',0,0)",
                params![session.as_str()],
            )
            .unwrap();
        let now = chrono::Utc::now();
        let first = PendingInputItem::new(
            PendingInputKind::UserText {
                text: "first".into(),
            },
            Some(serde_json::json!({"clientUserMessageId":"first"})),
            now,
        );
        let mut second = PendingInputItem::new(
            PendingInputKind::UserText {
                text: "second".into(),
            },
            None,
            now - chrono::Duration::days(1),
        );
        second.set_delivery(devo_protocol::native::host_message::HostMessageDelivery::Steer);
        assert!(db.admit_inbox(&session, &first).unwrap());
        assert!(db.admit_inbox(&session, &second).unwrap());
        assert!(!db.admit_inbox(&session, &first).unwrap());
        assert_eq!(
            db.list_pending(&session, QueueType::Turn)
                .unwrap()
                .iter()
                .map(|i| i.id)
                .collect::<Vec<_>>(),
            vec![first.id, second.id]
        );
        db.remove_pending_by_id(&session, QueueType::Turn, &first.id)
            .unwrap();
        drop(db);
        let db = Database::open(path).unwrap();
        assert!(!db.admit_inbox(&session, &first).unwrap());
        assert_eq!(
            db.list_pending(&session, QueueType::Turn)
                .unwrap()
                .iter()
                .map(|i| i.id)
                .collect::<Vec<_>>(),
            vec![second.id]
        );
    }
}
