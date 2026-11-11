//! Atomic queue-to-steer admission and conservative crash recovery.
//!
//! Receipts establish admission, never consumption. They outlive settlement so
//! a delayed retry cannot inject the same queue identity into another turn.

use std::collections::HashSet;

use rusqlite::{OptionalExtension, TransactionBehavior};

use super::*;

impl Database {
    /// Persists an edited Turn entry and the complete known queue order together.
    /// Unknown durable Turn rows (DB-first enqueues) retain their relative order
    /// after the supplied IDs. Missing targets leave every row untouched.
    pub fn update_queue_entry(
        &self,
        session_id: &SessionId,
        item: &PendingInputItem,
        ordered_ids: &[QueueItemId],
    ) -> Result<bool> {
        anyhow::ensure!(
            ordered_ids.contains(&item.id),
            "updated entry missing from queue order"
        );
        let mut conn = self.conn.lock().expect("database mutex poisoned");
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (kind, content) = pending_kind_parts(&item.kind);
        let changed = tx.execute(
            "UPDATE pending_messages SET kind = ?3, content = ?4, metadata = ?5
             WHERE session_id = ?1 AND pending_input_id = ?2 AND queue_type = 'turn'",
            params![
                session_id.as_str(),
                item.id.to_string(),
                kind,
                content,
                item.metadata.as_ref().map(serde_json::Value::to_string)
            ],
        )?;
        if changed == 0 {
            return Ok(false);
        }
        anyhow::ensure!(
            changed == 1,
            "duplicate pending queue identity during update"
        );
        reorder_pending_turn_rows(&tx, session_id, ordered_ids)?;
        tx.commit().context("failed to commit queue entry update")?;
        Ok(true)
    }

    /// Re-arms a held input with a fresh identity while retaining its old receipt.
    /// Also reconciles an unresolved Steer row after failed live settlement.
    /// Callers must first verify the in-memory item requires explicit review.
    pub fn retry_held_queue_steer(
        &self,
        session_id: &SessionId,
        old_id: &QueueItemId,
        replacement: &PendingInputItem,
        ordered_ids: &[QueueItemId],
    ) -> Result<bool> {
        anyhow::ensure!(
            replacement.id != *old_id,
            "retry requires a fresh queue identity"
        );
        anyhow::ensure!(
            ordered_ids.contains(&replacement.id),
            "retry identity missing from queue order"
        );
        anyhow::ensure!(
            !ordered_ids.contains(old_id),
            "old retry identity remains in queue order"
        );
        let mut conn = self.conn.lock().expect("database mutex poisoned");
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut metadata = replacement.metadata.clone();
        if let Some(serde_json::Value::Object(object)) = &mut metadata {
            object.remove("queueSteerRecoveryRequired");
        }
        let (kind, content) = pending_kind_parts(&replacement.kind);
        let changed = tx.execute(
            "UPDATE pending_messages
             SET pending_input_id = ?3, kind = ?4, content = ?5, metadata = ?6, created_at = ?7,
                 position = CASE WHEN queue_type = 'steer' THEN
                     (SELECT COALESCE(MAX(position), 0) + 1 FROM pending_messages
                      WHERE session_id = ?1 AND queue_type = 'turn') ELSE position END,
                 queue_type = 'turn'
             WHERE session_id = ?1 AND pending_input_id = ?2
               AND ((queue_type = 'turn' AND json_type(metadata, '$.queueSteerRecoveryRequired') = 'true')
                    OR (queue_type = 'steer' AND EXISTS
                        (SELECT 1 FROM queue_steer_receipts WHERE session_id = ?1 AND pending_input_id = ?2)))
               AND NOT EXISTS (SELECT 1 FROM pending_messages
                               WHERE session_id = ?1 AND pending_input_id = ?3)
               AND NOT EXISTS (SELECT 1 FROM queue_steer_receipts
                               WHERE session_id = ?1 AND pending_input_id = ?3)",
            params![
                session_id.as_str(),
                old_id.to_string(),
                replacement.id.to_string(),
                kind,
                content,
                metadata.as_ref().map(serde_json::Value::to_string),
                replacement.created_at.timestamp()
            ],
        )?;
        anyhow::ensure!(changed <= 1, "duplicate held queue identity");
        if changed == 0 {
            return Ok(false);
        }
        reorder_pending_turn_rows(&tx, session_id, ordered_ids)?;
        tx.commit().context("failed to commit held queue retry")?;
        Ok(changed == 1)
    }

    /// Discards explicitly reviewed input, including a receipt-backed Steer row
    /// left by failed live settlement. Admission receipts remain durable.
    /// Callers must first verify the in-memory item requires explicit review.
    pub fn discard_held_queue_steer(
        &self,
        session_id: &SessionId,
        queue_item_id: &QueueItemId,
    ) -> Result<bool> {
        let mut conn = self.conn.lock().expect("database mutex poisoned");
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "DELETE FROM pending_messages
             WHERE session_id = ?1 AND pending_input_id = ?2
               AND ((queue_type = 'turn' AND json_type(metadata, '$.queueSteerRecoveryRequired') = 'true')
                    OR (queue_type = 'steer' AND EXISTS
                        (SELECT 1 FROM queue_steer_receipts WHERE session_id = ?1 AND pending_input_id = ?2)))",
            params![session_id.as_str(), queue_item_id.to_string()],
        )?;
        anyhow::ensure!(changed <= 1, "duplicate held queue identity");
        tx.commit().context("failed to commit held queue discard")?;
        Ok(changed == 1)
    }

    /// Returns the originally admitted turn, including after settlement/restart.
    pub fn queue_steer_receipt(
        &self,
        session_id: &SessionId,
        queue_item_id: &QueueItemId,
    ) -> Result<Option<TurnId>> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let turn_id: Option<String> = conn
            .query_row(
                "SELECT turn_id FROM queue_steer_receipts
                 WHERE session_id = ?1 AND pending_input_id = ?2",
                params![session_id.as_str(), queue_item_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .context("failed to read queue steer receipt")?;
        Ok(turn_id.map(TurnId::from_string))
    }

    /// Atomically transfers an existing Turn item and records its admission.
    /// The caller holds queue admission locks and supplies the complete updated
    /// payload. A missing item or an already-admitted identity returns false.
    pub fn claim_queue_steer(
        &self,
        session_id: &SessionId,
        item: &PendingInputItem,
        turn_id: &TurnId,
    ) -> Result<bool> {
        let mut conn = self.conn.lock().expect("database mutex poisoned");
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (kind, content) = pending_kind_parts(&item.kind);
        let changed = tx.execute(
            "UPDATE pending_messages
             SET queue_type = 'steer', kind = ?3, content = ?4, metadata = ?5,
                 created_at = ?6
             WHERE session_id = ?1 AND pending_input_id = ?2 AND queue_type = 'turn'
               AND NOT EXISTS (SELECT 1 FROM queue_steer_receipts
                               WHERE session_id = ?1 AND pending_input_id = ?2)",
            params![
                session_id.as_str(),
                item.id.to_string(),
                kind,
                content,
                item.metadata.as_ref().map(serde_json::Value::to_string),
                item.created_at.timestamp()
            ],
        )?;
        if changed == 0 {
            return Ok(false);
        }
        anyhow::ensure!(changed == 1, "duplicate pending queue identity");
        tx.execute(
            "INSERT INTO queue_steer_receipts (session_id, pending_input_id, turn_id)
             VALUES (?1, ?2, ?3)",
            params![session_id.as_str(), item.id.to_string(), turn_id.as_str()],
        )?;
        tx.commit()
            .context("failed to commit queue steer admission")?;
        Ok(true)
    }

    /// Holds unresolved admissions on restore. No transcript/text heuristic can
    /// prove whether an interrupted turn consumed them; explicit user action is
    /// required before these items may run again.
    pub fn recover_interrupted_queue_steers(&self, session_id: &SessionId) -> Result<()> {
        let mut conn = self.conn.lock().expect("database mutex poisoned");
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let items = {
            let mut stmt = tx.prepare(
                "SELECT kind, content, pending_input_id, metadata, created_at
                 FROM pending_messages AS pending
                 WHERE session_id = ?1 AND queue_type = ?2
                   AND EXISTS (SELECT 1 FROM queue_steer_receipts AS receipt
                               WHERE receipt.session_id = pending.session_id
                                 AND receipt.pending_input_id = pending.pending_input_id)
                 ORDER BY position ASC, id ASC",
            )?;
            collect_pending_rows(&mut stmt, session_id, QueueType::Steer)?
        };
        for item in items {
            let mut metadata = match item.metadata {
                Some(serde_json::Value::Object(object)) => object,
                None => serde_json::Map::new(),
                Some(value) => {
                    let mut object = serde_json::Map::new();
                    object.insert("queueSteerOriginalMetadata".into(), value);
                    object
                }
            };
            metadata.insert("queueSteerRecoveryRequired".into(), true.into());
            tx.execute(
                "UPDATE pending_messages
                 SET queue_type = 'turn', metadata = ?3
                 WHERE session_id = ?1 AND pending_input_id = ?2 AND queue_type = 'steer'",
                params![
                    session_id.as_str(),
                    item.id.to_string(),
                    serde_json::Value::Object(metadata).to_string()
                ],
            )?;
        }
        tx.commit().context("failed to commit queue steer recovery")
    }

    /// Settles finalization against the complete, ordered live pending queue.
    /// Callers must hold the queue locks until commit. Remaining steer inputs
    /// occur in `pending`; consumed inputs do not. Receipts are retained in both
    /// cases. Ordinary Turn rows absent from the snapshot survive at the end:
    /// enqueue can persist a row before acquiring the caller's queue lock.
    /// The entire operation rolls back on any write failure.
    pub fn settle_queue_steers(
        &self,
        session_id: &SessionId,
        pending: &[PendingInputItem],
    ) -> Result<()> {
        let mut identities = HashSet::new();
        anyhow::ensure!(
            pending.iter().all(|item| identities.insert(item.id)),
            "duplicate pending queue identity during steer settlement"
        );
        let mut conn = self.conn.lock().expect("database mutex poisoned");
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Entries still present in the live queue were never consumed. Release
        // only their execution claims; immutable inbox admission receipts remain.
        // Held/uncertain entries retain claims and require explicit retry.
        for item in pending {
            if !item
                .metadata
                .as_ref()
                .and_then(|m| m.get("queueSteerRecoveryRequired"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            {
                tx.execute("DELETE FROM queue_steer_receipts WHERE session_id=?1 AND pending_input_id=?2
                    AND EXISTS(SELECT 1 FROM pending_messages WHERE session_id=?1 AND pending_input_id=?2 AND queue_type='steer')",
                    params![session_id.as_str(), item.id.to_string()])?;
            }
        }
        tx.execute(
            "DELETE FROM pending_messages WHERE session_id = ?1 AND queue_type = 'steer'",
            params![session_id.as_str()],
        )?;
        for (position, item) in pending.iter().enumerate() {
            let (kind, content) = pending_kind_parts(&item.kind);
            let changed = tx.execute(
                "UPDATE pending_messages
                 SET kind = ?2, content = ?3, metadata = ?5, created_at = ?6, position = ?7
                 WHERE session_id = ?1 AND queue_type = 'turn' AND pending_input_id = ?4",
                params![
                    session_id.as_str(),
                    kind,
                    content,
                    item.id.to_string(),
                    item.metadata.as_ref().map(serde_json::Value::to_string),
                    item.created_at.timestamp(),
                    position as i64
                ],
            )?;
            anyhow::ensure!(
                changed <= 1,
                "duplicate pending queue identity during settlement"
            );
            if changed == 0 {
                tx.execute(
                "INSERT INTO pending_messages
                 (session_id, queue_type, kind, content, pending_input_id, metadata, created_at, position)
                 VALUES (?1, 'turn', ?2, ?3, ?4, ?5, ?6, ?7)",
                params![session_id.as_str(), kind, content, item.id.to_string(),
                    item.metadata.as_ref().map(serde_json::Value::to_string),
                    item.created_at.timestamp(), position as i64],
            )?;
            }
        }
        let ordered_ids: Vec<_> = pending.iter().map(|item| item.id).collect();
        reorder_pending_turn_rows(&tx, session_id, &ordered_ids)?;
        tx.commit()
            .context("failed to commit queue steer settlement")
    }
}

// Preserve DB-first enqueues absent from the locked in-memory snapshot.
fn reorder_pending_turn_rows(
    tx: &rusqlite::Transaction<'_>,
    session_id: &SessionId,
    ordered_ids: &[QueueItemId],
) -> Result<()> {
    let identities: HashSet<_> = ordered_ids.iter().copied().collect();
    anyhow::ensure!(
        identities.len() == ordered_ids.len(),
        "duplicate pending queue order identity"
    );
    let extras = {
        let mut stmt = tx.prepare(
            "SELECT id, pending_input_id FROM pending_messages
             WHERE session_id = ?1 AND queue_type = 'turn' ORDER BY position ASC, id ASC",
        )?;
        stmt.query_map(params![session_id.as_str()], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter(|(_, id)| {
            !id.as_ref()
                .is_some_and(|id| identities.contains(&QueueItemId::from_string(id.clone())))
        })
        .collect::<Vec<_>>()
    };
    // In-flight steer claims still precede every pending turn. Keep their
    // durable positions ahead of edits/retries so crash recovery cannot invert
    // the queue after pending positions are compacted.
    let base: i64 = tx.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM pending_messages WHERE session_id=?1 AND queue_type='steer'",
        params![session_id.as_str()], |row| row.get(0),
    )?;
    for (position, id) in ordered_ids.iter().enumerate() {
        tx.execute(
            "UPDATE pending_messages SET position = ?3
             WHERE session_id = ?1 AND queue_type = 'turn' AND pending_input_id = ?2",
            params![session_id.as_str(), id.to_string(), base + position as i64],
        )?;
    }
    for (offset, (row_id, _)) in extras.iter().enumerate() {
        tx.execute(
            "UPDATE pending_messages SET position = ?2 WHERE id = ?1",
            params![row_id, base + (ordered_ids.len() + offset) as i64],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use tempfile::TempDir;

    fn database() -> (Database, TempDir, SessionId) {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join("queue.db")).unwrap();
        let session_id = SessionId::new();
        db.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO sessions (id, cwd, created_at, updated_at) VALUES (?1, '.', 0, 0)",
                params![session_id.as_str()],
            )
            .unwrap();
        (db, dir, session_id)
    }

    fn item(text: &str) -> PendingInputItem {
        PendingInputItem::new(
            PendingInputKind::UserText { text: text.into() },
            Some(json!({"clientUserMessageId": text, "other": {"nested": [1, 2]}})),
            Utc.timestamp_opt(1_700_000_000, 0).single().unwrap(),
        )
    }

    fn snapshot(db: &Database, session: &SessionId, queue: QueueType) -> serde_json::Value {
        serde_json::to_value(db.list_pending(session, queue).unwrap()).unwrap()
    }

    #[test]
    fn claims_are_session_scoped_idempotent_and_durable() {
        let (db, dir, session) = database();
        let input = item("same text");
        let turn = TurnId::new();
        assert!(!db.claim_queue_steer(&session, &input, &turn).unwrap());
        assert_eq!(db.queue_steer_receipt(&session, &input.id).unwrap(), None);
        db.push_pending(&session, QueueType::Turn, &input).unwrap();
        assert!(db.claim_queue_steer(&session, &input, &turn).unwrap());
        assert!(
            !db.claim_queue_steer(&session, &input, &TurnId::new())
                .unwrap()
        );
        assert_eq!(snapshot(&db, &session, QueueType::Turn), json!([]));
        assert_eq!(snapshot(&db, &session, QueueType::Steer), json!([input]));
        let other = SessionId::new();
        assert_eq!(db.queue_steer_receipt(&other, &input.id).unwrap(), None);
        db.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO sessions (id, cwd, created_at, updated_at) VALUES (?1, '.', 0, 0)",
                params![other.as_str()],
            )
            .unwrap();
        db.push_pending(&other, QueueType::Turn, &input).unwrap();
        let other_turn = TurnId::new();
        assert!(db.claim_queue_steer(&other, &input, &other_turn).unwrap());
        drop(db);
        let db = Database::open(dir.path().join("queue.db")).unwrap();
        assert_eq!(
            db.queue_steer_receipt(&session, &input.id).unwrap(),
            Some(turn)
        );
        assert_eq!(
            db.queue_steer_receipt(&other, &input.id).unwrap(),
            Some(other_turn)
        );
        assert!(
            !db.claim_queue_steer(&session, &input, &TurnId::new())
                .unwrap()
        );
    }

    #[test]
    fn claim_transfers_complete_supplied_payload() {
        let (db, _dir, session) = database();
        let mut input = item("old");
        db.push_pending(&session, QueueType::Turn, &input).unwrap();
        input.kind = PendingInputKind::UserInput {
            input: vec![
                devo_protocol::native::item::UserInput::Text {
                    text: "original input".into(),
                },
                devo_protocol::native::item::UserInput::Image {
                    uri: "https://example.com/image.png".into(),
                    mime_type: Some("image/png".into()),
                    detail: None,
                },
            ],
            display_text: "display".into(),
            prompt_text: "prompt with attachment".into(),
            prompt_messages: vec!["context".into()],
            prompt_images: vec![devo_protocol::PromptImagePart {
                mime_type: "image/png".into(),
                data_base64: "image data".into(),
            }],
        };
        input.metadata = Some(json!({"clientUserMessageId": "new", "custom": [1, 2]}));
        db.claim_queue_steer(&session, &input, &TurnId::new())
            .unwrap();
        assert_eq!(snapshot(&db, &session, QueueType::Steer), json!([input]));
    }

    #[test]
    fn recovery_holds_only_admitted_inputs_without_inferring_consumption() {
        let (db, dir, session) = database();
        let queued = item("queued");
        let admitted = item("same visible text");
        let plain_steer = item("same visible text");
        db.push_pending(&session, QueueType::Turn, &queued).unwrap();
        db.push_pending(&session, QueueType::Turn, &admitted)
            .unwrap();
        db.push_pending(&session, QueueType::Steer, &plain_steer)
            .unwrap();
        let turn = TurnId::new();
        db.claim_queue_steer(&session, &admitted, &turn).unwrap();
        drop(db);
        let db = Database::open(dir.path().join("queue.db")).unwrap();
        db.recover_interrupted_queue_steers(&session).unwrap();
        db.recover_interrupted_queue_steers(&session).unwrap();
        let mut held = admitted.clone();
        held.metadata.as_mut().unwrap()["queueSteerRecoveryRequired"] = json!(true);
        assert_eq!(
            snapshot(&db, &session, QueueType::Turn),
            json!([queued, held])
        );
        assert_eq!(
            snapshot(&db, &session, QueueType::Steer),
            json!([plain_steer])
        );
        assert_eq!(
            db.queue_steer_receipt(&session, &admitted.id).unwrap(),
            Some(turn)
        );
        assert!(
            !db.claim_queue_steer(&session, &admitted, &TurnId::new())
                .unwrap()
        );
    }

    #[test]
    fn settlement_preserves_full_order_and_receipts_for_consumed_and_pending() {
        let (db, dir, session) = database();
        let first = item("ordinary first");
        let consumed = item("consumed");
        let remaining = item("remaining");
        let last = item("ordinary last");
        let turn = TurnId::new();
        for input in [&first, &consumed, &remaining, &last] {
            db.push_pending(&session, QueueType::Turn, input).unwrap();
        }
        for input in [&consumed, &remaining] {
            db.claim_queue_steer(&session, input, &turn).unwrap();
        }
        let pending = [last, remaining.clone(), first];
        db.settle_queue_steers(&session, &pending).unwrap();
        assert_eq!(snapshot(&db, &session, QueueType::Turn), json!(pending));
        assert_eq!(snapshot(&db, &session, QueueType::Steer), json!([]));
        drop(db);
        let db = Database::open(dir.path().join("queue.db")).unwrap();
        db.recover_interrupted_queue_steers(&session).unwrap();
        assert_eq!(snapshot(&db, &session, QueueType::Turn), json!(pending));
        assert_eq!(
            db.queue_steer_receipt(&session, &consumed.id).unwrap(),
            Some(turn)
        );
        assert!(
            !db.claim_queue_steer(&session, &consumed, &TurnId::new())
                .unwrap()
        );
        assert_eq!(
            db.queue_steer_receipt(&session, &remaining.id).unwrap(),
            None
        );
        assert!(
            db.claim_queue_steer(&session, &remaining, &TurnId::new())
                .unwrap()
        );
        db.delete_session(&session).unwrap();
        assert_eq!(
            db.queue_steer_receipt(&session, &consumed.id).unwrap(),
            None
        );
    }

    #[test]
    fn admission_receipt_failure_rolls_back_transfer() {
        let (db, _dir, session) = database();
        let input = item("queue");
        db.push_pending(&session, QueueType::Turn, &input).unwrap();
        db.conn
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER reject_receipt BEFORE INSERT ON queue_steer_receipts
             BEGIN SELECT RAISE(ABORT, 'receipt failure'); END;",
            )
            .unwrap();
        assert!(
            db.claim_queue_steer(&session, &input, &TurnId::new())
                .is_err()
        );
        assert_eq!(snapshot(&db, &session, QueueType::Turn), json!([input]));
        assert_eq!(snapshot(&db, &session, QueueType::Steer), json!([]));
        assert_eq!(db.queue_steer_receipt(&session, &input.id).unwrap(), None);
    }

    #[test]
    fn settlement_failure_restores_both_queues_for_recovery() {
        let (db, _dir, session) = database();
        let ordinary = item("ordinary");
        let admitted = item("admitted");
        db.push_pending(&session, QueueType::Turn, &ordinary)
            .unwrap();
        db.push_pending(&session, QueueType::Turn, &admitted)
            .unwrap();
        db.claim_queue_steer(&session, &admitted, &TurnId::new())
            .unwrap();
        db.conn
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER reject_pending BEFORE INSERT ON pending_messages
             WHEN NEW.position = 1 BEGIN SELECT RAISE(ABORT, 'pending failure'); END;",
            )
            .unwrap();
        assert!(
            db.settle_queue_steers(&session, &[ordinary.clone(), admitted.clone()])
                .is_err()
        );
        assert_eq!(snapshot(&db, &session, QueueType::Turn), json!([ordinary]));
        assert_eq!(snapshot(&db, &session, QueueType::Steer), json!([admitted]));
        db.recover_interrupted_queue_steers(&session).unwrap();
        let mut held = admitted;
        held.metadata.as_mut().unwrap()["queueSteerRecoveryRequired"] = json!(true);
        assert_eq!(
            snapshot(&db, &session, QueueType::Turn),
            json!([ordinary, held])
        );
    }

    #[test]
    fn explicit_retry_requires_fresh_identity_and_keeps_position_and_receipt() {
        let (db, _dir, session) = database();
        let ordinary = item("ordinary");
        let admitted = item("admitted");
        db.push_pending(&session, QueueType::Turn, &ordinary)
            .unwrap();
        db.push_pending(&session, QueueType::Turn, &admitted)
            .unwrap();
        let turn = TurnId::new();
        db.claim_queue_steer(&session, &admitted, &turn).unwrap();
        db.recover_interrupted_queue_steers(&session).unwrap();
        let mut replacement = db
            .list_pending(&session, QueueType::Turn)
            .unwrap()
            .pop()
            .unwrap();
        assert!(
            db.retry_held_queue_steer(
                &session,
                &admitted.id,
                &replacement,
                &[ordinary.id, replacement.id]
            )
            .is_err()
        );
        replacement.id = QueueItemId::new();
        assert!(
            !db.retry_held_queue_steer(&session, &ordinary.id, &replacement, &[replacement.id])
                .unwrap()
        );
        assert!(
            db.retry_held_queue_steer(
                &session,
                &admitted.id,
                &replacement,
                &[ordinary.id, replacement.id]
            )
            .unwrap()
        );
        assert!(
            !db.retry_held_queue_steer(
                &session,
                &admitted.id,
                &replacement,
                &[ordinary.id, replacement.id]
            )
            .unwrap()
        );
        replacement
            .metadata
            .as_mut()
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("queueSteerRecoveryRequired");
        assert_eq!(
            snapshot(&db, &session, QueueType::Turn),
            json!([ordinary, replacement])
        );
        assert_eq!(
            db.queue_steer_receipt(&session, &admitted.id).unwrap(),
            Some(turn)
        );
        assert_eq!(
            db.queue_steer_receipt(&session, &replacement.id).unwrap(),
            None
        );
        assert!(
            db.claim_queue_steer(&session, &replacement, &TurnId::new())
                .unwrap()
        );
    }

    #[test]
    fn duplicate_identities_cannot_partially_claim_or_settle() {
        let (db, _dir, session) = database();
        let input = item("duplicate");
        for _ in 0..2 {
            db.push_pending(&session, QueueType::Turn, &input).unwrap();
        }
        let before = snapshot(&db, &session, QueueType::Turn);
        assert!(
            db.claim_queue_steer(&session, &input, &TurnId::new())
                .is_err()
        );
        assert!(
            db.settle_queue_steers(&session, &[input.clone(), input.clone()])
                .is_err()
        );
        assert_eq!(snapshot(&db, &session, QueueType::Turn), before);
        assert_eq!(snapshot(&db, &session, QueueType::Steer), json!([]));
        assert_eq!(db.queue_steer_receipt(&session, &input.id).unwrap(), None);
    }

    #[test]
    fn concurrent_connections_admit_an_identity_only_once() {
        let (db, dir, session) = database();
        let input = item("concurrent");
        db.push_pending(&session, QueueType::Turn, &input).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let connection = Database::open(dir.path().join("queue.db")).unwrap();
                let barrier = Arc::clone(&barrier);
                let input = input.clone();
                std::thread::spawn(move || {
                    let turn = TurnId::new();
                    barrier.wait();
                    let claimed = connection
                        .claim_queue_steer(&session, &input, &turn)
                        .unwrap();
                    (turn, claimed)
                })
            })
            .collect();
        let winners: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .filter_map(|(turn, claimed)| claimed.then_some(turn))
            .collect();
        assert_eq!(winners.len(), 1);
        assert_eq!(
            db.queue_steer_receipt(&session, &input.id).unwrap(),
            Some(winners[0])
        );
        assert_eq!(snapshot(&db, &session, QueueType::Steer), json!([input]));
        assert_eq!(snapshot(&db, &session, QueueType::Turn), json!([]));
    }

    #[test]
    fn settlement_preserves_db_first_enqueues_absent_from_snapshot() {
        let (db, _dir, session) = database();
        let known = item("known");
        let admitted = item("admitted");
        let incoming = item("persisted before queue lock");
        let next_incoming = item("also waiting for queue lock");
        for input in [&known, &admitted] {
            db.push_pending(&session, QueueType::Turn, input).unwrap();
        }
        db.claim_queue_steer(&session, &admitted, &TurnId::new())
            .unwrap();
        let pending = [admitted.clone(), known.clone()];
        for input in [&incoming, &next_incoming] {
            db.push_pending(&session, QueueType::Turn, input).unwrap();
        }
        db.settle_queue_steers(&session, &pending).unwrap();
        assert_eq!(
            snapshot(&db, &session, QueueType::Turn),
            json!([admitted, known, incoming, next_incoming])
        );
        assert_eq!(snapshot(&db, &session, QueueType::Steer), json!([]));
    }

    #[test]
    fn recovery_failure_rolls_back_preceding_recovered_item() {
        let (db, _dir, session) = database();
        let first = item("first");
        let second = item("second");
        for input in [&first, &second] {
            db.push_pending(&session, QueueType::Turn, input).unwrap();
            db.claim_queue_steer(&session, input, &TurnId::new())
                .unwrap();
        }
        db.conn
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER reject_recovery BEFORE UPDATE ON pending_messages
             WHEN NEW.queue_type = 'turn' AND NEW.content = 'second'
             BEGIN SELECT RAISE(ABORT, 'recovery failure'); END;",
            )
            .unwrap();
        assert!(db.recover_interrupted_queue_steers(&session).is_err());
        assert_eq!(snapshot(&db, &session, QueueType::Turn), json!([]));
        assert_eq!(
            snapshot(&db, &session, QueueType::Steer),
            json!([first, second])
        );
    }

    #[test]
    fn live_unresolved_steer_can_be_explicitly_retried_or_discarded() {
        let (db, _dir, session) = database();
        let ordinary = item("ordinary");
        let retry = item("retry");
        let discard = item("discard");
        let plain_steer = item("plain steer");
        let turn = TurnId::new();
        for input in [&ordinary, &retry, &discard] {
            db.push_pending(&session, QueueType::Turn, input).unwrap();
        }
        for input in [&retry, &discard] {
            db.claim_queue_steer(&session, input, &turn).unwrap();
        }
        db.push_pending(&session, QueueType::Steer, &plain_steer)
            .unwrap();
        let mut replacement = retry.clone();
        replacement.id = QueueItemId::new();
        assert!(!db.discard_held_queue_steer(&session, &ordinary.id).unwrap());
        assert!(
            !db.discard_held_queue_steer(&session, &plain_steer.id)
                .unwrap()
        );
        assert!(
            !db.retry_held_queue_steer(
                &session,
                &plain_steer.id,
                &replacement,
                &[ordinary.id, replacement.id]
            )
            .unwrap()
        );
        assert!(
            db.retry_held_queue_steer(
                &session,
                &retry.id,
                &replacement,
                &[ordinary.id, replacement.id]
            )
            .unwrap()
        );
        assert!(db.discard_held_queue_steer(&session, &discard.id).unwrap());
        assert!(!db.discard_held_queue_steer(&session, &discard.id).unwrap());
        assert_eq!(
            snapshot(&db, &session, QueueType::Turn),
            json!([ordinary, replacement])
        );
        assert_eq!(
            snapshot(&db, &session, QueueType::Steer),
            json!([plain_steer])
        );
        assert_eq!(
            db.queue_steer_receipt(&session, &retry.id).unwrap(),
            Some(turn)
        );
        assert_eq!(
            db.queue_steer_receipt(&session, &discard.id).unwrap(),
            Some(turn)
        );
    }

    #[test]
    fn failed_explicit_review_actions_preserve_unresolved_row_and_receipt() {
        let (db, _dir, session) = database();
        let input = item("unresolved");
        let turn = TurnId::new();
        db.push_pending(&session, QueueType::Turn, &input).unwrap();
        db.claim_queue_steer(&session, &input, &turn).unwrap();
        let mut replacement = input.clone();
        replacement.id = QueueItemId::new();
        db.conn
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER reject_retry BEFORE UPDATE ON pending_messages
             BEGIN SELECT RAISE(ABORT, 'retry failure'); END;
             CREATE TRIGGER reject_discard BEFORE DELETE ON pending_messages
             BEGIN SELECT RAISE(ABORT, 'discard failure'); END;",
            )
            .unwrap();
        assert!(
            db.retry_held_queue_steer(&session, &input.id, &replacement, &[replacement.id])
                .is_err()
        );
        assert!(db.discard_held_queue_steer(&session, &input.id).is_err());
        assert_eq!(snapshot(&db, &session, QueueType::Turn), json!([]));
        assert_eq!(snapshot(&db, &session, QueueType::Steer), json!([input]));
        assert_eq!(
            db.queue_steer_receipt(&session, &input.id).unwrap(),
            Some(turn)
        );
        assert_eq!(
            db.queue_steer_receipt(&session, &replacement.id).unwrap(),
            None
        );
    }

    #[test]
    fn queue_edit_is_atomic_preserves_order_and_db_first_enqueues() {
        let (db, _dir, session) = database();
        let first = item("first");
        let mut edited = item("old content");
        let incoming = item("incoming");
        for input in [&first, &edited, &incoming] {
            db.push_pending(&session, QueueType::Turn, input).unwrap();
        }
        edited.kind = PendingInputKind::UserText {
            text: "edited content".into(),
        };
        assert!(
            db.update_queue_entry(&session, &edited, &[edited.id, first.id])
                .unwrap()
        );
        assert_eq!(
            snapshot(&db, &session, QueueType::Turn),
            json!([edited, first, incoming])
        );
        let missing = item("missing");
        assert!(
            !db.update_queue_entry(&session, &missing, &[missing.id, first.id])
                .unwrap()
        );
        assert_eq!(
            snapshot(&db, &session, QueueType::Turn),
            json!([edited, first, incoming])
        );
        db.claim_queue_steer(&session, &edited, &TurnId::new())
            .unwrap();
        assert_eq!(snapshot(&db, &session, QueueType::Steer), json!([edited]));
        assert!(
            !db.update_queue_entry(&session, &edited, &[edited.id])
                .unwrap()
        );
    }

    #[test]
    fn queue_edit_reorder_failure_rolls_back_content_and_positions() {
        let (db, _dir, session) = database();
        let first = item("first");
        let mut edited = item("old content");
        for input in [&first, &edited] {
            db.push_pending(&session, QueueType::Turn, input).unwrap();
        }
        let before = snapshot(&db, &session, QueueType::Turn);
        edited.kind = PendingInputKind::UserText {
            text: "edited content".into(),
        };
        db.conn
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER reject_reorder BEFORE UPDATE OF position ON pending_messages
             WHEN NEW.position = 1 BEGIN SELECT RAISE(ABORT, 'reorder failure'); END;",
            )
            .unwrap();
        assert!(
            db.update_queue_entry(&session, &edited, &[edited.id, first.id])
                .is_err()
        );
        assert_eq!(snapshot(&db, &session, QueueType::Turn), before);
    }

    #[test]
    fn unresolved_retry_preserves_non_tail_order_after_reopen() {
        let (db, dir, session) = database();
        let first = item("first");
        let retry = item("retry in middle");
        let last = item("last");
        let incoming = item("DB-first extra");
        for input in [&first, &retry, &last, &incoming] {
            db.push_pending(&session, QueueType::Turn, input).unwrap();
        }
        let turn = TurnId::new();
        db.claim_queue_steer(&session, &retry, &turn).unwrap();
        let mut replacement = retry.clone();
        replacement.id = QueueItemId::new();
        assert!(
            db.retry_held_queue_steer(
                &session,
                &retry.id,
                &replacement,
                &[first.id, replacement.id, last.id]
            )
            .unwrap()
        );
        drop(db);
        let db = Database::open(dir.path().join("queue.db")).unwrap();
        assert_eq!(
            snapshot(&db, &session, QueueType::Turn),
            json!([first, replacement, last, incoming])
        );
        assert_eq!(snapshot(&db, &session, QueueType::Steer), json!([]));
        assert_eq!(
            db.queue_steer_receipt(&session, &retry.id).unwrap(),
            Some(turn)
        );
    }

    #[test]
    fn retry_reorder_failure_rolls_back_identity_and_queue_type() {
        let (db, _dir, session) = database();
        let ordinary = item("ordinary");
        let retry = item("retry");
        for input in [&ordinary, &retry] {
            db.push_pending(&session, QueueType::Turn, input).unwrap();
        }
        db.claim_queue_steer(&session, &retry, &TurnId::new())
            .unwrap();
        let mut replacement = retry.clone();
        replacement.id = QueueItemId::new();
        db.conn
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER reject_retry_reorder BEFORE UPDATE OF position ON pending_messages
             WHEN NEW.position = 0 BEGIN SELECT RAISE(ABORT, 'retry reorder failure'); END;",
            )
            .unwrap();
        assert!(
            db.retry_held_queue_steer(
                &session,
                &retry.id,
                &replacement,
                &[replacement.id, ordinary.id]
            )
            .is_err()
        );
        assert_eq!(snapshot(&db, &session, QueueType::Turn), json!([ordinary]));
        assert_eq!(snapshot(&db, &session, QueueType::Steer), json!([retry]));
    }
}
