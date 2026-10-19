//! Acknowledged execution records, independent of lossy UI event delivery.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::ResponseItem;

/// One atomic execution fact. Calls are accepted only after complete model assembly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ExecutionRecord {
    OutputArtifacts {
        artifacts: Vec<devo_tools::output_store::OutputArtifact>,
    },
    ModelCompleted {
        items: Vec<ResponseItem>,
        stop_reason: Option<crate::StopReason>,
    },
    Recovery {
        state: RecoveryState,
    },
    IntentBatch {
        calls: Vec<ResponseItem>,
    },
    Outcomes {
        results: Vec<ResponseItem>,
    },
    PromptCheckpoint {
        items: Vec<ResponseItem>,
        #[serde(default)]
        counters: Option<ExecutionCounters>,
    },
    /// Delta form of [`ExecutionRecord::PromptCheckpoint`] for the common
    /// append-only case: `items` replace the replay tail starting at `from`,
    /// so a journal leg only pays for what changed instead of re-embedding
    /// the whole prompt. Replay semantics generalize the full replace —
    /// `truncate(from)` then extend — and `from` beyond the current replay
    /// length is rejected as corruption.
    LegCheckpoint {
        from: usize,
        items: Vec<ResponseItem>,
        #[serde(default)]
        counters: Option<ExecutionCounters>,
    },
}

/// Accounting at a durable prompt boundary; continuing does not reset usage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionCounters {
    pub turn_count: usize,
    pub total_input_tokens: usize,
    pub total_output_tokens: usize,
    pub total_tokens: usize,
    pub total_cache_creation_tokens: usize,
    pub total_cache_read_tokens: usize,
    pub last_input_tokens: usize,
    pub last_turn_tokens: usize,
}

impl ExecutionCounters {
    pub fn capture(session: &crate::SessionState) -> Self {
        Self {
            turn_count: session.turn_count,
            total_input_tokens: session.total_input_tokens,
            total_output_tokens: session.total_output_tokens,
            total_tokens: session.total_tokens,
            total_cache_creation_tokens: session.total_cache_creation_tokens,
            total_cache_read_tokens: session.total_cache_read_tokens,
            last_input_tokens: session.last_input_tokens,
            last_turn_tokens: session.last_turn_tokens,
        }
    }

    pub fn restore(&self, session: &mut crate::SessionState) {
        session.turn_count = self.turn_count;
        session.total_input_tokens = self.total_input_tokens;
        session.total_output_tokens = self.total_output_tokens;
        session.total_tokens = self.total_tokens;
        session.total_cache_creation_tokens = self.total_cache_creation_tokens;
        session.total_cache_read_tokens = self.total_cache_read_tokens;
        session.last_input_tokens = self.last_input_tokens;
        session.last_turn_tokens = self.last_turn_tokens;
    }
}

/// An explicit user decision survives restart independently of turn status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryState {
    pub revision: u64,
    pub attempt: u32,
    pub disposition: RecoveryDisposition,
    pub reason: String,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryDisposition {
    Available,
    Resuming,
    Canceled,
}

/// Durable acknowledgment for a turn's execution facts.
///
/// Implementations must serialize writes, synchronize storage before returning
/// success, and reject conflicting commits for a call identity. Failure must be
/// returned to the caller; sending an observation event is not acknowledgment.
/// A turn must not dispatch an intent or continue past an outcome until committed.
#[allow(clippy::double_must_use)]
#[async_trait]
pub trait ToolIntentJournal: Send + Sync {
    async fn commit(&self, record: ExecutionRecord) -> anyhow::Result<()>;

    /// Read the acknowledged state without executing pending calls.
    async fn replay(&self) -> anyhow::Result<ExecutionReplay>;
}

/// Fold acknowledged records without executing any calls during replay.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ExecutionReplay {
    pub artifacts: std::collections::HashMap<String, devo_tools::output_store::OutputArtifact>,
    pub counters: Option<ExecutionCounters>,
    pub completed: bool,
    pub stop_reason: Option<crate::StopReason>,
    pub recovery: Option<RecoveryState>,
    pub items: Vec<ResponseItem>,
    pub has_checkpoint: bool,
    committed: std::collections::HashMap<String, ResponseItem>,
}

impl ExecutionReplay {
    /// Validate and apply a fact atomically, rejecting conflicting identities.
    pub fn apply(&mut self, record: &ExecutionRecord) -> anyhow::Result<()> {
        let (prefix, entries) = match record {
            ExecutionRecord::OutputArtifacts { artifacts } => {
                for artifact in artifacts {
                    if let Some(previous) = self.artifacts.get(&artifact.id) {
                        anyhow::ensure!(
                            previous.file_identity == artifact.file_identity
                                && previous.path == artifact.path
                                && previous.owner == artifact.owner
                                && previous.bytes <= artifact.bytes,
                            "conflicting output artifact reference"
                        );
                    }
                }
                for artifact in artifacts {
                    self.artifacts.insert(artifact.id.clone(), artifact.clone());
                }
                return Ok(());
            }
            ExecutionRecord::ModelCompleted { items, stop_reason } => {
                if self.completed {
                    anyhow::ensure!(
                        self.items == *items && self.stop_reason == *stop_reason,
                        "conflicting final model response"
                    );
                } else {
                    self.items.clone_from(items);
                    self.has_checkpoint = true;
                    self.completed = true;
                    self.stop_reason = stop_reason.clone();
                }
                return Ok(());
            }
            ExecutionRecord::Recovery { state } => {
                if let Some(previous) = &self.recovery {
                    anyhow::ensure!(
                        state.revision > previous.revision || state == previous,
                        "conflicting recovery revision"
                    );
                }
                self.recovery = Some(state.clone());
                return Ok(());
            }
            ExecutionRecord::IntentBatch { calls } => ("call", calls),
            ExecutionRecord::Outcomes { results } => ("result", results),
            ExecutionRecord::PromptCheckpoint { items, counters } => {
                self.counters.clone_from(counters);
                self.items.clone_from(items);
                self.has_checkpoint = true;
                return Ok(());
            }
            ExecutionRecord::LegCheckpoint {
                from,
                items,
                counters,
            } => {
                self.counters.clone_from(counters);
                anyhow::ensure!(
                    *from <= self.items.len(),
                    "leg checkpoint base {from} exceeds replay length {}",
                    self.items.len()
                );
                self.items.truncate(*from);
                self.items.extend(items.iter().cloned());
                self.has_checkpoint = true;
                return Ok(());
            }
        };
        // Validate the whole batch first so a conflicting record leaves the
        // replay untouched; only then mutate in place (no full-clone per
        // record).
        let mut seen: std::collections::HashMap<String, &ResponseItem> =
            std::collections::HashMap::with_capacity(entries.len());
        let mut accepted: Vec<(String, &ResponseItem)> = Vec::with_capacity(entries.len());
        for item in entries {
            let id = match (prefix, item) {
                ("call", ResponseItem::ToolCall { id, .. }) => id,
                ("result", ResponseItem::ToolCallOutput { tool_use_id, .. }) => tool_use_id,
                _ => anyhow::bail!("invalid execution record item"),
            };
            let key = format!("{prefix}:{id}");
            let previous = self
                .committed
                .get(&key)
                .or_else(|| seen.get(key.as_str()).copied());
            if let Some(previous) = previous {
                anyhow::ensure!(previous == item, "conflicting execution record for {key}");
                continue;
            }
            seen.insert(key.clone(), item);
            accepted.push((key, item));
        }
        for (key, item) in accepted {
            if !self.items.contains(item) {
                self.items.push(item.clone());
            }
            self.committed.insert(key, item.clone());
        }
        Ok(())
    }

    /// Missing outcomes are uncertainty reports, never instructions to rerun.
    pub fn interrupted_outcomes(&self) -> Vec<ResponseItem> {
        let completed = self
            .items
            .iter()
            .filter_map(ResponseItem::tool_call_output_id)
            .collect::<std::collections::HashSet<_>>();
        self.items.iter().filter_map(|item| {
            let ResponseItem::ToolCall { id, .. } = item else { return None };
            (!completed.contains(id.as_str())).then(|| ResponseItem::ToolCallOutput {
                tool_use_id: id.clone(),
                content: "Tool execution was interrupted. Execution may have occurred; verify its outcome before retrying. This call was not automatically rerun.".into(),
                is_error: true,
            })
        }).collect()
    }
}

/// Returns the execution record variant for a compact internal line owned by
/// `turn_id`. Non-matching or whitespace-formatted lines return `None`; the
/// normal typed reader still handles those lines, so this is only an optional
/// scan accelerator.
fn execution_record_variant_for_turn<'a>(
    line: &'a str,
    turn_id: &devo_protocol::native::ids::TurnId,
) -> Option<&'a str> {
    let turn_field = "\"turnId\":\"";
    let turn_start = line.find(turn_field)? + turn_field.len();
    let turn_end = turn_start + turn_id.as_str().len();
    if line.get(turn_start..turn_end)? != turn_id.as_str()
        || line.as_bytes().get(turn_end) != Some(&b'\"')
    {
        return None;
    }
    let entry_field = "\"entry\":";
    let entry_start = line[turn_end..].find(entry_field)? + turn_end + entry_field.len();
    let entry = line[entry_start..].strip_prefix("{\"type\":\"execution\",\"record\":{")?;
    let record_tag = entry.strip_prefix("\"type\":\"")?;
    let tag_end = record_tag.find('\"')?;
    Some(&record_tag[..tag_end])
}

/// Checks whether an earlier prompt checkpoint is superseded by the last full
/// prompt checkpoint. Leg checkpoints before that base are superseded too.
fn is_superseded_checkpoint_line(
    line: &str,
    line_index: usize,
    last_prompt_checkpoint_line: Option<usize>,
    turn_id: &devo_protocol::native::ids::TurnId,
) -> bool {
    last_prompt_checkpoint_line.is_some_and(|last| {
        line_index < last
            && matches!(
                execution_record_variant_for_turn(line, turn_id),
                Some("promptCheckpoint" | "legCheckpoint")
            )
    })
}

fn is_balanced_json_line(line: &str) -> bool {
    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escaped = false;
    for byte in line.bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'\"' {
                in_string = false;
            }
        } else {
            match byte {
                b'\"' => in_string = true,
                b'{' | b'[' => depth += 1,
                b'}' | b']' => {
                    depth -= 1;
                    if depth < 0 {
                        return false;
                    }
                }
                _ => {}
            }
        }
    }
    depth == 0 && !in_string && !escaped
}

fn last_prompt_checkpoint_line(
    path: &std::path::Path,
    turn_id: &devo_protocol::native::ids::TurnId,
) -> anyhow::Result<Option<usize>> {
    use std::io::BufRead;
    let mut last = None;
    for (line_index, line) in std::io::BufReader::new(std::fs::File::open(path)?)
        .lines()
        .enumerate()
    {
        let line = line?;
        if execution_record_variant_for_turn(&line, turn_id) == Some("promptCheckpoint")
            && is_balanced_json_line(&line)
        {
            last = Some(line_index);
        }
    }
    Ok(last)
}

/// Loads acknowledged facts for one turn. Unrelated rollout lines are skipped
/// by a substring prefilter; prompt and leg checkpoints before the last full
/// prompt checkpoint are also skipped because that full snapshot supersedes
/// them. Callers that need full-journal validation must replay the rollout
/// separately. A truncated final line is tolerated.
pub fn read_execution_replay(
    path: &std::path::Path,
    turn_id: &devo_protocol::native::ids::TurnId,
) -> anyhow::Result<ExecutionReplay> {
    use crate::{InternalRecord, RolloutLine, RolloutLineReadError, parse_rollout_line};
    use std::io::BufRead;
    let last_prompt_checkpoint_line = last_prompt_checkpoint_line(path, turn_id)?;
    let mut replay = ExecutionReplay::default();
    let mut lines = std::io::BufReader::new(std::fs::File::open(path)?)
        .lines()
        .enumerate()
        .peekable();
    while let Some((line_index, line)) = lines.next() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if is_superseded_checkpoint_line(&line, line_index, last_prompt_checkpoint_line, turn_id) {
            continue;
        }
        // Substring pre-filter: only this turn's execution records or any
        // compaction snapshot can affect the replay. Item and turn lines may
        // carry the same turn id but are irrelevant here, so require the
        // nested execution tag as well. Compaction snapshots still invalidate
        // checkpoints regardless of owner. False positives are parsed and
        // discarded by the typed match below; they cannot fabricate state.
        let execution_for_turn = line.contains("\"execution\"") && line.contains(turn_id.as_str());
        let compaction_snapshot = line.contains("\"compactionSnapshot\"");
        if !execution_for_turn && !compaction_snapshot {
            continue;
        }
        match parse_rollout_line(&line) {
            Ok(line) => match line {
                RolloutLine::Internal {
                    turn_id: Some(owner),
                    entry: InternalRecord::Execution { record },
                    ..
                } if owner.as_str() == turn_id.as_str() => replay.apply(&record)?,
                RolloutLine::CompactionSnapshot { .. } => replay.has_checkpoint = false,
                _ => {}
            },
            Err(RolloutLineReadError::TruncatedTail) => {
                let mut only_blank = true;
                while let Some((_, next)) = lines.peek() {
                    match next {
                        Ok(line) if line.trim().is_empty() => {
                            let _ = lines.next();
                        }
                        Ok(_) => {
                            only_blank = false;
                            break;
                        }
                        Err(_) => {
                            only_blank = false;
                            break;
                        }
                    }
                }
                if only_blank {
                    break;
                }
                return Err(RolloutLineReadError::TruncatedTail.into());
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(replay)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn call_item(id: &str) -> ResponseItem {
        ResponseItem::ToolCall {
            id: id.into(),
            name: "write".into(),
            input: serde_json::json!({ "value": id }),
        }
    }

    fn replay_of(records: &[ExecutionRecord]) -> ExecutionReplay {
        let mut replay = ExecutionReplay::default();
        for record in records {
            replay.apply(record).expect("apply test record");
        }
        replay
    }

    /// A delta checkpoint replays to exactly the state of the equivalent
    /// full checkpoint — replay consumers cannot tell the encodings apart.
    #[test]
    fn leg_checkpoint_replays_to_full_checkpoint_state() {
        let prompt_a = vec![call_item("a"), call_item("b")];
        let tail = vec![call_item("c")];
        let delta_replay = replay_of(&[
            ExecutionRecord::PromptCheckpoint {
                items: prompt_a.clone(),
                counters: None,
            },
            ExecutionRecord::LegCheckpoint {
                from: prompt_a.len(),
                items: tail,
                counters: None,
            },
        ]);
        let full_replay = replay_of(&[ExecutionRecord::PromptCheckpoint {
            items: vec![call_item("a"), call_item("b"), call_item("c")],
            counters: None,
        }]);
        assert_eq!(delta_replay, full_replay);
    }

    /// The delta replaces the tail from `from`, matching the full
    /// checkpoint's replace semantics when the prompt shrinks above `from`.
    #[test]
    fn leg_checkpoint_replaces_tail_not_appends() {
        let replay = replay_of(&[
            ExecutionRecord::PromptCheckpoint {
                items: vec![call_item("a"), call_item("b"), call_item("c")],
                counters: None,
            },
            ExecutionRecord::LegCheckpoint {
                from: 1,
                items: vec![call_item("x")],
                counters: None,
            },
        ]);
        assert_eq!(replay.items, vec![call_item("a"), call_item("x")]);
    }

    /// A base beyond the replay length means the journal was reordered or
    /// truncated; fail closed rather than replay a fabricated prompt.
    #[test]
    fn leg_checkpoint_beyond_replay_length_is_rejected() {
        let mut replay = replay_of(&[ExecutionRecord::PromptCheckpoint {
            items: vec![call_item("a")],
            counters: None,
        }]);
        let result = replay.apply(&ExecutionRecord::LegCheckpoint {
            from: 5,
            items: vec![call_item("b")],
            counters: None,
        });
        assert!(result.is_err());
    }

    /// Duplicate entries inside one batch commit once; a conflicting entry
    /// rejects the whole record without mutating the replay.
    #[test]
    fn batch_deduplicates_and_rejects_conflicts_atomically() {
        let call = call_item("a");
        let mut replay = ExecutionReplay::default();
        replay
            .apply(&ExecutionRecord::IntentBatch {
                calls: vec![call.clone(), call.clone()],
            })
            .unwrap();
        assert_eq!(replay.items, vec![call.clone()]);

        let conflicting = ResponseItem::ToolCall {
            id: "a".into(),
            name: "write".into(),
            input: serde_json::json!({ "value": "changed" }),
        };
        let before = replay.clone();
        assert!(
            replay
                .apply(&ExecutionRecord::IntentBatch {
                    calls: vec![conflicting],
                })
                .is_err()
        );
        assert_eq!(replay, before);

        // A fresh entry followed by a malformed one must not leak the fresh
        // entry into the replay — the whole batch is atomic.
        let fresh = call_item("b");
        let before = replay.clone();
        assert!(
            replay
                .apply(&ExecutionRecord::IntentBatch {
                    calls: vec![
                        fresh.clone(),
                        ResponseItem::Message(devo_protocol::Message::user("not a tool call"),)
                    ],
                })
                .is_err()
        );
        assert_eq!(replay, before);
        // ...and the fresh entry still commits cleanly once the batch is valid.
        replay
            .apply(&ExecutionRecord::IntentBatch {
                calls: vec![fresh.clone()],
            })
            .unwrap();
        assert_eq!(replay.items, vec![call, fresh]);
    }

    /// Journals containing delta checkpoints parse back to the same replay
    /// state the writer held — the on-disk encoding round-trips.
    #[test]
    fn leg_checkpoint_round_trips_through_rollout_file() {
        let turn_id = devo_protocol::native::ids::TurnId::from_string("turn_replayprobe".into());
        let session_id =
            devo_protocol::native::ids::SessionId::from_string("ses_replayprobe".into());
        let records = vec![
            ExecutionRecord::IntentBatch {
                calls: vec![call_item("a")],
            },
            ExecutionRecord::PromptCheckpoint {
                items: vec![call_item("a"), call_item("b")],
                counters: Some(ExecutionCounters {
                    turn_count: 1,
                    total_input_tokens: 10,
                    total_output_tokens: 5,
                    total_tokens: 15,
                    total_cache_creation_tokens: 0,
                    total_cache_read_tokens: 0,
                    last_input_tokens: 10,
                    last_turn_tokens: 15,
                }),
            },
            ExecutionRecord::LegCheckpoint {
                from: 1,
                items: vec![call_item("x")],
                counters: Some(ExecutionCounters {
                    turn_count: 2,
                    total_input_tokens: 20,
                    total_output_tokens: 8,
                    total_tokens: 28,
                    total_cache_creation_tokens: 0,
                    total_cache_read_tokens: 4,
                    last_input_tokens: 20,
                    last_turn_tokens: 28,
                }),
            },
        ];
        let mut text = String::new();
        for record in &records {
            let line = crate::RolloutLine::Internal {
                v: 2,
                timestamp: chrono::Utc::now(),
                session_id,
                turn_id: Some(turn_id),
                seq: 0,
                entry: crate::InternalRecord::Execution {
                    record: record.clone(),
                },
            };
            text.push_str(&serde_json::to_string(&line).expect("serialize line"));
            text.push('\n');
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rollout.jsonl");
        std::fs::write(&path, text).expect("write rollout");

        let replay = read_execution_replay(&path, &turn_id).expect("read execution replay");
        assert_eq!(replay, replay_of(&records));
    }

    /// Trace: L2-DES-CONTEXT-004
    #[test]
    fn replay_is_idempotent_and_rejects_conflicting_arguments() {
        let call = ResponseItem::ToolCall {
            id: "a".into(),
            name: "write".into(),
            input: serde_json::json!({"value": 1}),
        };
        let fact = ExecutionRecord::IntentBatch {
            calls: vec![call.clone()],
        };
        let mut replay = ExecutionReplay::default();
        replay.apply(&fact).unwrap();
        replay.apply(&fact).unwrap();
        assert_eq!(replay.items, vec![call.clone()]);
        let bad = ExecutionRecord::IntentBatch {
            calls: vec![ResponseItem::ToolCall {
                id: "a".into(),
                name: "write".into(),
                input: serde_json::json!({"value": 2}),
            }],
        };
        assert!(replay.apply(&bad).is_err());
        assert_eq!(replay.items, vec![call]);
        let outcomes = replay.interrupted_outcomes();
        assert_eq!(outcomes.len(), 1);
        replay
            .apply(&ExecutionRecord::Outcomes {
                results: outcomes.clone(),
            })
            .unwrap();
        replay
            .apply(&ExecutionRecord::Outcomes { results: outcomes })
            .unwrap();
        assert_eq!(replay.interrupted_outcomes(), Vec::new());
    }

    #[test]
    fn latest_full_checkpoint_skips_superseded_snapshots_and_tolerates_crash_tail() {
        let now = chrono::Utc::now();
        let turn = devo_protocol::native::ids::TurnId::from_string("turn_latestcheckpoint".into());
        let session_id =
            devo_protocol::native::ids::SessionId::from_string("ses_latestcheckpoint".into());
        let counters = |total| ExecutionCounters {
            turn_count: 1,
            total_input_tokens: total,
            total_output_tokens: 0,
            total_tokens: total,
            total_cache_creation_tokens: 0,
            total_cache_read_tokens: 0,
            last_input_tokens: total,
            last_turn_tokens: total,
        };
        let records = vec![
            ExecutionRecord::IntentBatch {
                calls: vec![call_item("before")],
            },
            ExecutionRecord::PromptCheckpoint {
                items: vec![call_item("first"), call_item("second")],
                counters: Some(counters(2)),
            },
            ExecutionRecord::LegCheckpoint {
                from: 1,
                items: vec![call_item("delta")],
                counters: Some(counters(3)),
            },
            ExecutionRecord::PromptCheckpoint {
                items: vec![call_item("final")],
                counters: Some(counters(4)),
            },
            ExecutionRecord::IntentBatch {
                calls: vec![call_item("after")],
            },
        ];
        let lines = records
            .iter()
            .map(|record| crate::RolloutLine::Internal {
                v: 2,
                timestamp: now,
                session_id,
                turn_id: Some(turn),
                seq: 0,
                entry: crate::InternalRecord::Execution {
                    record: record.clone(),
                },
            })
            .collect::<Vec<_>>();
        let serialized = lines
            .iter()
            .map(|line| serde_json::to_string(line).expect("serialize line"))
            .collect::<Vec<_>>();
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rollout.jsonl");
        let mut text = serialized.join("\n");
        text.push('\n');
        std::fs::write(&path, &text).expect("write rollout");

        let last_checkpoint = last_prompt_checkpoint_line(&path, &turn).expect("scan lines");
        assert_eq!(last_checkpoint, Some(3));
        assert!(is_superseded_checkpoint_line(
            &serialized[1],
            1,
            last_checkpoint,
            &turn
        ));
        assert!(is_superseded_checkpoint_line(
            &serialized[2],
            2,
            last_checkpoint,
            &turn
        ));
        assert!(!is_superseded_checkpoint_line(
            &serialized[3],
            3,
            last_checkpoint,
            &turn
        ));

        text.push_str(&format!(
            "{{\"kind\":\"internal\",\"turnId\":\"{turn}\",\"entry\":{{\"type\":\"execution\",\"record\":{{\"type\":\"promptCheckpoint\",\"items\":["
        ));
        std::fs::write(&path, text).expect("append crash tail");
        assert_eq!(
            read_execution_replay(&path, &turn).expect("read replay"),
            replay_of(&records)
        );
    }

    /// The replay pre-filter must not change semantics: execution records of
    /// other turns never apply, item lines never apply, and a compaction
    /// snapshot from ANY turn still invalidates acknowledged checkpoints.
    #[test]
    fn execution_replay_prefilter_preserves_semantics() {
        use devo_protocol::native::item::{Item, ItemEnvelope, ItemState};

        let now = chrono::Utc::now();
        let turn = devo_protocol::native::ids::TurnId::from_string("turn_prefilter".into());
        let other = devo_protocol::native::ids::TurnId::from_string("turn_unrelated".into());
        let session_id = devo_protocol::native::ids::SessionId::from_string("ses_prefilter".into());

        let exec_line = |owner: &devo_protocol::native::ids::TurnId, record: ExecutionRecord| {
            crate::RolloutLine::Internal {
                v: 2,
                timestamp: now,
                session_id,
                turn_id: Some(*owner),
                seq: 0,
                entry: crate::InternalRecord::Execution { record },
            }
        };
        let counters = |total: usize| ExecutionCounters {
            turn_count: 1,
            total_input_tokens: total,
            total_output_tokens: 0,
            total_tokens: total,
            total_cache_creation_tokens: 0,
            total_cache_read_tokens: 0,
            last_input_tokens: total,
            last_turn_tokens: total,
        };
        let checkpoint = |names: &[&str], total: usize| ExecutionRecord::PromptCheckpoint {
            items: names.iter().map(|name| call_item(name)).collect(),
            counters: Some(counters(total)),
        };
        let item_line = crate::RolloutLine::Item {
            v: 2,
            timestamp: now,
            item: ItemEnvelope {
                id: devo_protocol::native::ids::ItemId::new(),
                session_id,
                turn_id: turn,
                seq: 1,
                revision: 1,
                created_at: now,
                updated_at: now,
                state: ItemState::Completed,
                item: Item::AssistantMessage {
                    text: "execution text is not a durable execution record".into(),
                },
                parent_id: None,
            },
        };
        let snapshot_line = crate::RolloutLine::CompactionSnapshot {
            v: 2,
            timestamp: now,
            session_id,
            turn_id: other,
            summary_item_id: devo_protocol::native::ids::ItemId::new(),
            preserved_item_ids: Vec::new(),
            context_occupancy: None,
        };

        let write = |name: &str, lines: &[crate::RolloutLine]| {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.keep().join(name);
            let mut text = String::new();
            for line in lines {
                text.push_str(&serde_json::to_string(line).expect("serialize line"));
                text.push('\n');
            }
            std::fs::write(&path, text).expect("write rollout");
            path
        };

        // A foreign turn's checkpoint must not leak in, an owning turn's item
        // line must be ignored, and the foreign compaction snapshot must
        // still clear `has_checkpoint`.
        let with_snapshot = write(
            "with_snapshot.jsonl",
            &[
                exec_line(
                    &turn,
                    ExecutionRecord::IntentBatch {
                        calls: vec![call_item("a")],
                    },
                ),
                exec_line(&turn, checkpoint(&["a", "b"], 10)),
                item_line.clone(),
                exec_line(&other, checkpoint(&["c"], 99)),
                snapshot_line,
            ],
        );
        let replay = read_execution_replay(&with_snapshot, &turn).expect("read replay");
        assert_eq!(replay.items, vec![call_item("a"), call_item("b")]);
        assert_eq!(replay.counters, Some(counters(10)));
        assert!(!replay.has_checkpoint);

        // Without the snapshot the same journal yields an acknowledged
        // checkpoint.
        let without_snapshot = write(
            "without_snapshot.jsonl",
            &[
                exec_line(
                    &turn,
                    ExecutionRecord::IntentBatch {
                        calls: vec![call_item("a")],
                    },
                ),
                exec_line(&turn, checkpoint(&["a", "b"], 10)),
                item_line,
                exec_line(&other, checkpoint(&["c"], 99)),
            ],
        );
        let replay = read_execution_replay(&without_snapshot, &turn).expect("read replay");
        assert_eq!(replay.items, vec![call_item("a"), call_item("b")]);
        assert_eq!(replay.counters, Some(counters(10)));
        assert!(replay.has_checkpoint);
    }
}
