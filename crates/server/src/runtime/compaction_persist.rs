//! Shared compaction summary + snapshot persistence helpers.
//!
//! Manual `/compact` and query-loop auto/proactive compaction both need to:
//! - derive preserved item ids from the compacted response suffix
//! - build a durable `ContextCompaction` summary item
//! - append a `CompactionSnapshot` so resume can rebuild `prompt_messages`

use std::sync::Arc;

use chrono::Utc;
use devo_core::CompactionSnapshotLine;
use devo_core::ResponseItem;
use devo_protocol::approx_tokens_from_byte_count;
use devo_protocol::native::ids::{ItemId, SessionId, TurnId};
use devo_protocol::native::item::ContextOccupancy;
use devo_protocol::native::item::Item;
use devo_protocol::native::item::UserInput;
use devo_protocol::native::turn::TurnKind;

use super::ServerRuntime;
use crate::execution::PersistedTurnItem;
use crate::persisted_native_item::PersistedNativeItem;
use crate::persisted_native_item::context_compaction_item;
use crate::persisted_native_item::history_entry_from_native_item;
use crate::persistence::RolloutStore;

/// Match the compacted preserve suffix against the prompt-visible journal tail.
///
/// The suffix and the journal describe the same history in different shapes:
/// the journal records `Reasoning` items the compactor's response items skip,
/// collapses an `update_plan` call into a single `Plan` item where the
/// compactor sees a ToolCall + ToolCallOutput pair, and so on. Exact-equality
/// matching therefore fails on real sessions and the snapshot would lose the
/// preserved suffix (resume then rebuilds the prompt from the summary alone).
/// Instead both sides are reduced to comparable anchors — tool traffic by its
/// unique call id, text by exact role+text — and aligned backwards. Anchorless
/// entries (`Reason`) and shape shifts are tolerated; a suffix that cannot be
/// aligned monotonically still yields an empty list (previous behavior).
pub(crate) fn preserved_item_ids_from_compacted(
    persisted_turn_items: &[PersistedTurnItem],
    compacted_items: &[ResponseItem],
) -> Vec<devo_protocol::native::ids::ItemId> {
    let preserved = compacted_items.get(1..).unwrap_or(&[]);
    if preserved.is_empty() {
        return Vec::new();
    }

    // Journal anchors, in prompt order.
    let mut journal: Vec<(devo_protocol::native::ids::ItemId, PromptAnchor)> = Vec::new();
    for item in persisted_turn_items {
        if !crate::persistence::prompt_visible_persisted_turn_item(item) {
            continue;
        }
        match &item.item {
            Item::UserMessage { content, .. } => {
                let text = content
                    .iter()
                    .filter_map(|part| match part {
                        UserInput::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                journal.push((item.item_id, PromptAnchor::UserText(text)));
            }
            Item::AssistantMessage { text, .. } => {
                journal.push((item.item_id, PromptAnchor::AssistantText(text.clone())));
            }
            // An `update_plan` call surfaces as one Plan item carrying the
            // call id; the compactor's suffix anchors the same call on both
            // its request and output, which collapse onto this one item.
            Item::Plan {
                call_id: Some(call_id),
                ..
            } => journal.push((item.item_id, PromptAnchor::ToolCall(call_id.clone()))),
            Item::Plan { entries, .. } => {
                let text = entries
                    .iter()
                    .map(|entry| entry.step.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                journal.push((item.item_id, PromptAnchor::AssistantText(text)));
            }
            Item::HostedToolCall { output, .. } => {
                let text = output
                    .as_ref()
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                journal.push((item.item_id, PromptAnchor::AssistantText(text)));
            }
            Item::ContextCompaction { summary, .. } => {
                journal.push((
                    item.item_id,
                    PromptAnchor::AssistantText(summary.clone().unwrap_or_default()),
                ));
            }
            Item::Reasoning { .. } => {}
            Item::ToolCall { call_id, .. } | Item::ToolResult { call_id, .. } => {
                journal.push((item.item_id, PromptAnchor::ToolCall(call_id.clone())));
            }
            Item::CommandExecution { call_id, .. } => {
                journal.push((item.item_id, PromptAnchor::ToolCall(call_id.clone())));
            }
            Item::Approval { .. }
            | Item::FileChange { .. }
            | Item::UserInputRequest { .. }
            | Item::SubAgent { .. }
            | Item::BackgroundTask { .. }
            | Item::GoalProgress { .. }
            | Item::Refinement { .. }
            | Item::Warning { .. }
            | Item::BranchSummary { .. } => {}
        }
    }

    // Suffix anchors: same reduction over the compactor's verbatim items.
    let suffix: Vec<PromptAnchor> = preserved.iter().filter_map(compacted_item_anchor).collect();
    if suffix.is_empty() {
        return Vec::new();
    }

    // Backwards alignment. Matching always moves down the journal, so a
    // repeated anchor resolves to its newest unmatched occurrence — the one
    // the suffix actually came from.
    let mut matched: Vec<devo_protocol::native::ids::ItemId> = Vec::new();
    let mut cursor = journal.len();
    let mut previous: Option<(&PromptAnchor, usize)> = None;
    for target in suffix.iter().rev() {
        // A tool request and its output anchor on the same call id. When the
        // journal carries them as ONE item (CommandExecution, Plan), the
        // second anchor is a collapse onto the item the first already
        // matched. When the journal carries them as SEPARATE items (ordinary
        // ToolCall + ToolResult), the request is its own item and must be
        // matched as well — the resume-side prompt rebuild substitutes the
        // preserved journal items for the compactor's verbatim suffix, so a
        // dropped request item would leave an orphan tool result in the
        // rebuilt prompt. The two cases are told apart by whether an earlier
        // unmatched journal occurrence of the anchor exists.
        let collapsed = previous.is_some_and(|(prev_target, prev_index)| {
            matches!(target, PromptAnchor::ToolCall(_))
                && prev_target == target
                && prev_index <= cursor
                && journal[..prev_index]
                    .iter()
                    .all(|(_, anchor)| anchor != target)
        });
        if !collapsed {
            let found = journal[..cursor]
                .iter()
                .rposition(|(_, anchor)| anchor == target);
            let Some(index) = found else {
                return Vec::new();
            };
            let (item_id, _) = &journal[index];
            if matched.contains(item_id) {
                // The suffix would need to reuse a journal item — not a suffix.
                return Vec::new();
            }
            matched.push(*item_id);
            cursor = index;
            previous = Some((target, index));
        }
    }
    matched.reverse();
    matched
}

/// A comparable prompt anchor for suffix alignment.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PromptAnchor {
    /// Tool request or output, keyed by its unique call id.
    ToolCall(String),
    UserText(String),
    AssistantText(String),
}

fn compacted_item_anchor(item: &ResponseItem) -> Option<PromptAnchor> {
    match item {
        // Reasoning has no journal anchor: the compactor keeps it verbatim,
        // but it is not needed to locate the suffix boundary.
        ResponseItem::Reason { .. } => None,
        ResponseItem::Message(message) => {
            let text = message
                .content
                .iter()
                .filter_map(|block| match block {
                    devo_core::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            if text.is_empty() {
                return None;
            }
            match message.role {
                devo_core::Role::User => Some(PromptAnchor::UserText(text)),
                _ => Some(PromptAnchor::AssistantText(text)),
            }
        }
        ResponseItem::ToolCall { id, .. } => Some(PromptAnchor::ToolCall(id.clone())),
        ResponseItem::ToolCallOutput { tool_use_id, .. } => {
            Some(PromptAnchor::ToolCall(tool_use_id.clone()))
        }
    }
}

/// Build the durable Native summary item from compacted history.
///
/// `trigger` distinguishes a user-requested `/compact` (`Manual`) from
/// query-loop auto compaction (`AutoThreshold`) on the persisted item.
pub(crate) fn summary_item_from_compacted(
    compacted_items: &[ResponseItem],
    trigger: devo_protocol::native::item::CompactionTrigger,
) -> Item {
    let summary_text = compacted_items
        .first()
        .and_then(|item| match item {
            ResponseItem::Message(message) => {
                message.content.iter().find_map(|block| match block {
                    devo_core::ContentBlock::Text { text } => Some(text.clone()),
                    devo_core::ContentBlock::Reasoning { .. }
                    | devo_core::ContentBlock::ProviderReasoning { .. }
                    | devo_core::ContentBlock::ToolUse { .. }
                    | devo_core::ContentBlock::HostedToolUse { .. }
                    | devo_core::ContentBlock::ToolResult { .. }
                    | devo_core::ContentBlock::Image { .. } => None,
                })
            }
            ResponseItem::Reason { text } => Some(text.clone()),
            ResponseItem::ToolCall { .. } | ResponseItem::ToolCallOutput { .. } => None,
        })
        .unwrap_or_default();
    context_compaction_item(summary_text, trigger)
}

/// Construct the rollout compaction snapshot line.
///
/// Bridges Native → legacy UUID only at the packed `CompactionSnapshotLine`
/// durable-record boundary (not for live registry lookups).
pub(crate) fn build_compaction_snapshot_line(
    session_id: &SessionId,
    turn_id: &TurnId,
    summary_item_id: &ItemId,
    preserved_item_ids: Vec<ItemId>,
    context_occupancy: Option<ContextOccupancy>,
) -> CompactionSnapshotLine {
    CompactionSnapshotLine {
        timestamp: Utc::now(),
        session_id: *session_id,
        turn_id: *turn_id,
        summary_item_id: *summary_item_id,
        preserved_item_ids: preserved_item_ids.clone(),
        context_occupancy,
    }
}

/// Inputs needed to append a compaction summary item and its snapshot.
pub(crate) struct CompactionSummaryPersist {
    pub(crate) session_id: SessionId,
    pub(crate) turn_id: TurnId,
    pub(crate) summary_item_id: ItemId,
    pub(crate) item_seq: u64,
    pub(crate) summary_item: Item,
    pub(crate) snapshot: CompactionSnapshotLine,
    /// Transcript-tree parent of the summary item (the leaf before the
    /// compaction). Without it the summary rows strand off the active path.
    pub(crate) parent_id: Option<ItemId>,
    /// Leaf epoch AFTER advancing the leaf to the summary item.
    pub(crate) leaf_epoch: u64,
}

/// Append the summary item and compaction snapshot to the durable rollout.
///
/// The tree edge, item line, snapshot, and session-leaf pointer all go through
/// one `append_rollout_lines` call so a failure cannot leave the summary item
/// durably appended without its snapshot or its tree linkage.
pub(crate) fn append_compaction_summary_and_snapshot(
    rollout_store: &RolloutStore,
    rollout_path: &std::path::Path,
    persist: CompactionSummaryPersist,
) -> anyhow::Result<()> {
    use devo_protocol::native::item::ItemState;
    use devo_protocol::native::wire_projector::typed_item_envelope;

    let CompactionSummaryPersist {
        session_id,
        turn_id,
        summary_item_id,
        item_seq,
        summary_item,
        snapshot,
        parent_id,
        leaf_epoch,
    } = persist;
    let mut envelope = typed_item_envelope(
        session_id,
        turn_id,
        summary_item_id,
        item_seq,
        &summary_item,
        ItemState::Completed,
        Utc::now(),
        None,
    );
    envelope.parent_id = parent_id;
    rollout_store.append_compaction_summary_and_snapshot_lines_at(
        rollout_path,
        envelope,
        snapshot,
        parent_id,
        leaf_epoch,
    )?;
    Ok(())
}

/// Build the in-memory journal entry for a compaction summary item.
pub(crate) fn compaction_persisted_turn_item(
    turn_id: devo_protocol::native::ids::TurnId,
    turn_kind: TurnKind,
    item_id: devo_protocol::native::ids::ItemId,
    summary_item: Item,
) -> PersistedTurnItem {
    PersistedNativeItem::new(turn_id, turn_kind, item_id, summary_item)
}

impl ServerRuntime {
    /// Persist an in-turn (auto/proactive) compaction summary + snapshot.
    ///
    /// Must not block on the session-actor mailbox: the actor is waiting on the
    /// turn event stream. Mutate inline scratch under the stream lock, then write
    /// rollout after releasing the lock.
    pub(crate) async fn persist_in_turn_compaction(
        self: &Arc<Self>,
        session_id: SessionId,
        turn_id: TurnId,
        summary_item_id: ItemId,
        compacted_items: &[ResponseItem],
    ) -> Option<u64> {
        let Some(stream) = self.active_stream_state(session_id).await else {
            tracing::warn!(
                session_id = %session_id,
                turn_id = %turn_id,
                "in-turn compaction persist skipped: no active stream"
            );
            return None;
        };

        let spawn_stable_items = self
            .active_turns
            .spawn_snapshot_for_session(session_id)
            .await
            .map(|snapshot| snapshot.stable_items)
            .unwrap_or_default();

        let rollout = {
            let mut stream = stream.lock().await;
            let Some(inline) = stream.turn_inline.as_mut() else {
                tracing::warn!(
                    session_id = %session_id,
                    turn_id = %turn_id,
                    "in-turn compaction persist skipped: no inline state"
                );
                return None;
            };
            if inline.turn_id != turn_id {
                tracing::warn!(
                    session_id = %session_id,
                    turn_id = %turn_id,
                    inline_turn_id = %inline.turn_id,
                    "in-turn compaction persist skipped: turn mismatch"
                );
                return None;
            }

            let mut journal = spawn_stable_items;
            journal.extend(inline.persisted_turn_items.iter().cloned());
            let preserved_item_ids = preserved_item_ids_from_compacted(&journal, compacted_items);
            let mut summary_item = summary_item_from_compacted(
                compacted_items,
                devo_protocol::native::item::CompactionTrigger::AutoThreshold,
            );

            let prompt_bytes = compacted_items
                .iter()
                .map(|item| serde_json::to_string(item).map_or(0, |json| json.len()))
                .sum::<usize>();
            let conversation_tokens = approx_tokens_from_byte_count(prompt_bytes);

            let model = inline
                .summary
                .model_name()
                .and_then(|slug| {
                    inline
                        .hook_context
                        .runtime_context
                        .model_catalog
                        .get(slug)
                        .or_else(|| self.deps.model_catalog.get(slug))
                })
                .or_else(|| {
                    inline.summary.model_binding_id().and_then(|binding| {
                        inline
                            .hook_context
                            .runtime_context
                            .model_catalog
                            .get(binding)
                            .or_else(|| self.deps.model_catalog.get(binding))
                    })
                });
            let window = inline
                .summary
                .settings
                .effective_context_window
                .or_else(|| model.map(super::context_occupancy::resolved_compaction_limit))
                .unwrap_or(0);
            let previous_occupancy = inline.summary.last_context_occupancy.clone();
            let occupancy = super::context_occupancy::occupancy_after_compaction(
                window,
                previous_occupancy.as_ref(),
                conversation_tokens,
                None,
            );
            inline.summary.last_context_occupancy = Some(occupancy.clone());
            inline.summary.last_query_total_tokens = occupancy.total_tokens as usize;
            inline.summary.prompt_token_estimate =
                conversation_tokens.try_into().unwrap_or(usize::MAX);

            // Record the occupancy deltas on the durable item so tree badges
            // and later readers see real numbers, not unmeasured zeros.
            if let Item::ContextCompaction { before, after, .. } = &mut summary_item {
                *before = crate::persisted_native_item::context_usage_from_occupancy(
                    previous_occupancy.as_ref(),
                );
                *after = Some(crate::persisted_native_item::context_usage_from_occupancy(
                    Some(&occupancy),
                ));
            }

            let item_seq = inline.allocate_item_seq();
            let snapshot = build_compaction_snapshot_line(
                &session_id,
                &turn_id,
                &summary_item_id,
                preserved_item_ids,
                Some(occupancy),
            );
            inline.latest_compaction_snapshot = Some(snapshot.clone());
            inline
                .persisted_turn_items
                .push(compaction_persisted_turn_item(
                    inline.turn_id,
                    inline.turn_kind,
                    summary_item_id,
                    summary_item.clone(),
                ));
            if let Some(history_item) = history_entry_from_native_item(&summary_item) {
                inline.history_items.push(history_item);
            }

            // Chain the summary item into the transcript tree: the next item
            // persisted this turn must parent onto it, not skip over it —
            // otherwise active-path scoping drops the compaction row.
            let parent_id = inline.transcript_leaf_id;
            inline.transcript_leaf_id = Some(summary_item_id);
            inline.leaf_epoch = inline.leaf_epoch.saturating_add(1);
            let leaf_epoch = inline.leaf_epoch;

            inline.rollout_path.clone().map(|path| {
                (
                    path,
                    item_seq,
                    summary_item,
                    snapshot,
                    parent_id,
                    leaf_epoch,
                )
            })
        };

        let (rollout_path, item_seq, summary_item, snapshot, parent_id, leaf_epoch) = rollout?;
        append_compaction_summary_and_snapshot(
            &self.rollout_store,
            &rollout_path,
            CompactionSummaryPersist {
                session_id,
                turn_id,
                summary_item_id,
                item_seq,
                summary_item,
                snapshot,
                parent_id,
                leaf_epoch,
            },
        )
        .map_err(
            |error| tracing::warn!(%session_id, %error, "compaction snapshot persistence failed"),
        )
        .ok()?;
        Some(item_seq)
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn preserved_item_ids_match_complete_command_execution_pair() {
        let command_item_id = ItemId::new();
        let command_input = serde_json::json!({ "cmd": "printf ok" });
        let command_output = serde_json::Value::String("ok".to_string());
        let persisted_turn_items = vec![PersistedNativeItem::new(
            TurnId::new(),
            TurnKind::Regular,
            command_item_id,
            Item::CommandExecution {
                call_id: "call-1".to_string(),
                command: "printf ok".to_string(),
                argv: None,
                cwd: Default::default(),
                input: Some(command_input.clone()),
                output: Some(command_output.clone()),
                exit_code: None,
                execution_handle: None,
                is_error: false,
                execution_mode: devo_protocol::native::item::ExecutionMode::Foreground,
                origin: devo_protocol::native::item::ExecOrigin::AgentTool,
                sandbox: None,
            },
        )];
        let compacted_items = vec![
            ResponseItem::Message(devo_core::Message::assistant_text("summary")),
            ResponseItem::ToolCall {
                id: "call-1".to_string(),
                name: "exec_command".to_string(),
                input: command_input,
            },
            ResponseItem::ToolCallOutput {
                tool_use_id: "call-1".to_string(),
                content: "ok".to_string(),
                is_error: false,
            },
        ];

        // The journal's single CommandExecution item covers both the call and
        // its output; it must be listed once, not duplicated.
        assert_eq!(
            preserved_item_ids_from_compacted(&persisted_turn_items, &compacted_items),
            vec![command_item_id]
        );
    }

    #[test]
    fn preserved_item_ids_survive_reasoning_and_plan_shape_shifts() {
        let user_id = ItemId::new();
        let reasoning_id = ItemId::new();
        let plan_id = ItemId::new();
        let assistant_id = ItemId::new();
        let persisted_turn_items = vec![
            PersistedNativeItem::new(
                TurnId::new(),
                TurnKind::Regular,
                user_id,
                Item::UserMessage {
                    client_user_message_id: None,
                    content: vec![UserInput::Text {
                        text: "update the plan".to_string(),
                    }],
                    entry: Default::default(),
                },
            ),
            PersistedNativeItem::new(
                TurnId::new(),
                TurnKind::Regular,
                reasoning_id,
                Item::Reasoning {
                    text: "thinking about the plan".to_string(),
                    provider_payload_ref: None,
                },
            ),
            PersistedNativeItem::new(
                TurnId::new(),
                TurnKind::Regular,
                plan_id,
                Item::Plan {
                    call_id: Some("call-plan".to_string()),
                    entries: vec![
                        devo_protocol::native::item::PlanEntry {
                            step: "step one".to_string(),
                            status: devo_protocol::native::item::PlanStepStatus::Completed,
                        },
                        devo_protocol::native::item::PlanEntry {
                            step: "step two".to_string(),
                            status: devo_protocol::native::item::PlanStepStatus::InProgress,
                        },
                    ],
                },
            ),
            PersistedNativeItem::new(
                TurnId::new(),
                TurnKind::Regular,
                assistant_id,
                Item::AssistantMessage {
                    text: "plan updated".to_string(),
                },
            ),
        ];
        // The compactor's suffix keeps the user message, the reasoning, the
        // update_plan call as a ToolCall + ToolCallOutput pair, and the
        // assistant reply. Reasoning has no anchor; the pair collapses onto
        // the Plan item via its call id.
        let compacted_items = vec![
            ResponseItem::Message(devo_core::Message::assistant_text("summary")),
            ResponseItem::Message(devo_core::Message::user("update the plan")),
            ResponseItem::Reason {
                text: "thinking about the plan".to_string(),
            },
            ResponseItem::ToolCall {
                id: "call-plan".to_string(),
                name: "update_plan".to_string(),
                input: serde_json::json!({}),
            },
            ResponseItem::ToolCallOutput {
                tool_use_id: "call-plan".to_string(),
                content: "plan updated".to_string(),
                is_error: false,
            },
            ResponseItem::Message(devo_core::Message::assistant_text("plan updated")),
        ];

        assert_eq!(
            preserved_item_ids_from_compacted(&persisted_turn_items, &compacted_items),
            vec![user_id, plan_id, assistant_id]
        );
    }

    #[test]
    fn preserved_item_ids_keep_separate_tool_request_and_result_items() {
        // Ordinary tools journal as TWO items — ToolCall (request) and
        // ToolResult (output) — sharing one call-id anchor. The request item
        // must be preserved too: the resume-side prompt rebuild substitutes
        // the preserved journal items for the compactor's verbatim suffix, so
        // dropping the request leaves an orphan tool result in the rebuilt
        // prompt (rejected by the provider as a tool_result without a
        // preceding tool_use).
        let user_id = ItemId::new();
        let request_id = ItemId::new();
        let result_id = ItemId::new();
        let assistant_id = ItemId::new();
        let persisted_turn_items = vec![
            PersistedNativeItem::new(
                TurnId::new(),
                TurnKind::Regular,
                user_id,
                Item::UserMessage {
                    client_user_message_id: None,
                    content: vec![UserInput::Text {
                        text: "run the tool".to_string(),
                    }],
                    entry: Default::default(),
                },
            ),
            PersistedNativeItem::new(
                TurnId::new(),
                TurnKind::Regular,
                request_id,
                Item::ToolCall {
                    call_id: "call-9".to_string(),
                    tool_name: "bash".to_string(),
                    source: devo_protocol::native::item::ToolSource::Builtin,
                    server_name: None,
                    input: Some(serde_json::json!({ "command": "echo hi" })),
                },
            ),
            PersistedNativeItem::new(
                TurnId::new(),
                TurnKind::Regular,
                result_id,
                Item::ToolResult {
                    call_id: "call-9".to_string(),
                    output: serde_json::Value::String("hi".to_string()),
                    display_content: None,
                    is_error: false,
                    truncated: false,
                },
            ),
            PersistedNativeItem::new(
                TurnId::new(),
                TurnKind::Regular,
                assistant_id,
                Item::AssistantMessage {
                    text: "done".to_string(),
                },
            ),
        ];
        let compacted_items = vec![
            ResponseItem::Message(devo_core::Message::assistant_text("summary")),
            ResponseItem::Message(devo_core::Message::user("run the tool")),
            ResponseItem::ToolCall {
                id: "call-9".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({ "command": "echo hi" }),
            },
            ResponseItem::ToolCallOutput {
                tool_use_id: "call-9".to_string(),
                content: "hi".to_string(),
                is_error: false,
            },
            ResponseItem::Message(devo_core::Message::assistant_text("done")),
        ];

        assert_eq!(
            preserved_item_ids_from_compacted(&persisted_turn_items, &compacted_items),
            vec![user_id, request_id, result_id, assistant_id]
        );
    }

    #[test]
    fn preserved_item_ids_reject_unalignable_suffix() {
        let user_id = ItemId::new();
        let persisted_turn_items = vec![PersistedNativeItem::new(
            TurnId::new(),
            TurnKind::Regular,
            user_id,
            Item::UserMessage {
                client_user_message_id: None,
                content: vec![UserInput::Text {
                    text: "real message".to_string(),
                }],
                entry: Default::default(),
            },
        )];
        let compacted_items = vec![
            ResponseItem::Message(devo_core::Message::assistant_text("summary")),
            ResponseItem::Message(devo_core::Message::user("not in the journal")),
        ];
        assert_eq!(
            preserved_item_ids_from_compacted(&persisted_turn_items, &compacted_items),
            Vec::<ItemId>::new()
        );
    }
}
