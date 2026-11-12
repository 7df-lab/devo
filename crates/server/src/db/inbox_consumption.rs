//! Atomic withdrawal of an unclaimed host completion. Receipts remain tombstones.
use super::*;
use rusqlite::OptionalExtension;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InboxConsumption {
    Consumed,
    AlreadyConsumed,
    AlreadyDelivered,
}

impl Database {
    /// Caller serializes this transaction with inbox admission and claim under
    /// the owning session's state-change gate. A claim is never withdrawn.
    pub(crate) fn consume_inbox_completion(
        &self,
        session: &SessionId,
        key: &str,
        id: &QueueItemId,
    ) -> Result<InboxConsumption> {
        let mut conn = self.conn.lock().expect("database mutex poisoned");
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS inbox_receipts (
                session_id TEXT NOT NULL, event_key TEXT NOT NULL,
                pending_input_id TEXT NOT NULL, PRIMARY KEY(session_id, event_key));
             CREATE TABLE IF NOT EXISTS inbox_consumptions (
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                event_key TEXT NOT NULL, PRIMARY KEY(session_id, event_key));",
        )?;
        let consumed: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM inbox_consumptions WHERE session_id=?1 AND event_key=?2)",
            params![session.as_str(), key],
            |row| row.get(0),
        )?;
        if consumed {
            return Ok(InboxConsumption::AlreadyConsumed);
        }
        let receipt: Option<String> = tx
            .query_row(
                "SELECT pending_input_id FROM inbox_receipts WHERE session_id=?1 AND event_key=?2",
                params![session.as_str(), key],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(receipt) = receipt {
            anyhow::ensure!(receipt == id.to_string(), "completion identity mismatch");
            let claimed: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM queue_steer_receipts WHERE session_id=?1 AND pending_input_id=?2)",
                params![session.as_str(), receipt], |row| row.get(0),
            )?;
            let pending: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM pending_messages WHERE session_id=?1 AND pending_input_id=?2 AND queue_type='turn')",
                params![session.as_str(), receipt], |row| row.get(0),
            )?;
            if claimed || !pending {
                return Ok(InboxConsumption::AlreadyDelivered);
            }
        }
        tx.execute(
            "INSERT OR IGNORE INTO inbox_receipts VALUES (?1,?2,?3)",
            params![session.as_str(), key, id.to_string()],
        )?;
        tx.execute(
            "INSERT INTO inbox_consumptions VALUES (?1,?2)",
            params![session.as_str(), key],
        )?;
        tx.execute(
            "DELETE FROM pending_messages WHERE session_id=?1 AND pending_input_id=?2 AND queue_type='turn'",
            params![session.as_str(), id.to_string()],
        )?;
        tx.commit()?;
        Ok(InboxConsumption::Consumed)
    }
}

#[cfg(test)]
mod tests;
