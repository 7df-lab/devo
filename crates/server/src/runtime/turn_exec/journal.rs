//! Persist-first tool facts written outside the session actor.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use devo_core::durable_execution::{
    ExecutionRecord, ExecutionReplay, ToolIntentJournal, read_execution_replay,
};
use devo_core::{InternalRecord, RolloutLine};
use devo_protocol::native::ids::{SessionId, TurnId};
use tokio::sync::Mutex;

use super::super::ServerRuntime;

pub(crate) struct RolloutToolJournal {
    runtime: Arc<ServerRuntime>,
    path: PathBuf,
    session_id: SessionId,
    turn_id: TurnId,
    committed: Mutex<Option<ExecutionReplay>>,
}

impl RolloutToolJournal {
    pub(crate) fn new(
        runtime: Arc<ServerRuntime>,
        path: PathBuf,
        session_id: SessionId,
        turn_id: TurnId,
    ) -> Self {
        Self {
            runtime,
            path,
            session_id,
            turn_id,
            committed: Mutex::new(None),
        }
    }
}

#[async_trait]
impl ToolIntentJournal for RolloutToolJournal {
    async fn replay(&self) -> anyhow::Result<ExecutionReplay> {
        let path = self.path.clone();
        let turn_id = self.turn_id;
        tokio::task::spawn_blocking(move || read_execution_replay(&path, &turn_id)).await?
    }

    async fn commit(&self, record: ExecutionRecord) -> anyhow::Result<()> {
        let mut committed = self.committed.lock().await;
        if committed.is_none() {
            let path = self.path.clone();
            let turn_id = self.turn_id;
            *committed = Some(
                tokio::task::spawn_blocking(move || read_execution_replay(&path, &turn_id))
                    .await??,
            );
        }
        let previous = committed.as_ref().expect("journal initialized");
        let record = encode_prompt_checkpoint(previous, record);
        let mut updated = previous.clone();
        updated.apply(&record)?;
        if updated == *previous {
            return Ok(());
        }
        let runtime = Arc::clone(&self.runtime);
        let path = self.path.clone();
        let line = RolloutLine::Internal {
            v: 2,
            timestamp: chrono::Utc::now(),
            session_id: self.session_id,
            turn_id: Some(self.turn_id),
            seq: 0,
            entry: InternalRecord::Execution { record },
        };
        tokio::task::spawn_blocking(move || {
            runtime
                .rollout_store
                .append_rollout_lines(&path, vec![line])
        })
        .await??;
        *committed = Some(updated);
        Ok(())
    }
}

/// Encodes a prompt checkpoint as a delta against the last acknowledged
/// replay when the new prompt appends to it, so a per-leg checkpoint costs
/// bytes for the new tail instead of the whole conversation. Anything that
/// rewrites or reorders the prompt (steers, notes, compaction) keeps the
/// full checkpoint.
fn encode_prompt_checkpoint(
    previous: &ExecutionReplay,
    record: ExecutionRecord,
) -> ExecutionRecord {
    match record {
        ExecutionRecord::PromptCheckpoint {
            ref items,
            ref counters,
        } if previous.items.len() <= items.len()
            && items.starts_with(previous.items.as_slice()) =>
        {
            ExecutionRecord::LegCheckpoint {
                from: previous.items.len(),
                items: items[previous.items.len()..].to_vec(),
                counters: counters.clone(),
            }
        }
        record => record,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn call_item(id: &str) -> devo_core::ResponseItem {
        devo_core::ResponseItem::ToolCall {
            id: id.into(),
            name: "write".into(),
            input: serde_json::json!({ "value": id }),
        }
    }

    fn checkpoint_replay(items: Vec<devo_core::ResponseItem>) -> ExecutionReplay {
        let mut replay = ExecutionReplay::default();
        replay
            .apply(&ExecutionRecord::PromptCheckpoint {
                items,
                counters: None,
            })
            .expect("checkpoint applies");
        replay
    }

    /// An appending prompt checkpoint encodes as a delta over the tail the
    /// journal already acknowledged.
    #[test]
    fn appending_checkpoint_encodes_as_delta() {
        let previous = checkpoint_replay(vec![call_item("a"), call_item("b")]);
        let encoded = encode_prompt_checkpoint(
            &previous,
            ExecutionRecord::PromptCheckpoint {
                items: vec![call_item("a"), call_item("b"), call_item("c")],
                counters: None,
            },
        );
        assert_eq!(
            encoded,
            ExecutionRecord::LegCheckpoint {
                from: 2,
                items: vec![call_item("c")],
                counters: None,
            }
        );
    }

    /// Applying the encoded delta yields exactly the state a full
    /// checkpoint would have produced, so skipping the full write is
    /// invisible to replay consumers.
    #[test]
    fn encoded_delta_replays_equal_to_full_checkpoint() {
        let previous = checkpoint_replay(vec![call_item("a"), call_item("b")]);
        let encoded = encode_prompt_checkpoint(
            &previous,
            ExecutionRecord::PromptCheckpoint {
                items: vec![call_item("a"), call_item("b"), call_item("c")],
                counters: None,
            },
        );
        let mut delta_replay = previous.clone();
        delta_replay.apply(&encoded).expect("delta applies");
        let full_replay = checkpoint_replay(vec![call_item("a"), call_item("b"), call_item("c")]);
        assert_eq!(delta_replay, full_replay);
    }

    /// A prompt that rewrites (rather than appends to) the acknowledged
    /// items — steers, notes, compaction — keeps the full checkpoint.
    #[test]
    fn rewriting_checkpoint_keeps_full_encoding() {
        let previous = checkpoint_replay(vec![call_item("a"), call_item("b")]);
        let rewritten = ExecutionRecord::PromptCheckpoint {
            items: vec![call_item("b"), call_item("a")],
            counters: None,
        };
        let encoded = encode_prompt_checkpoint(&previous, rewritten.clone());
        assert_eq!(encoded, rewritten);
    }
}
