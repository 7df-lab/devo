//! Replay projection builder.
//!
//! Implements L3-BEH-CORE-001 §5. Consumes raw DurableRecords from the
//! JSONL store and builds SessionMetadata, TurnProjections, and other
//! projections needed by the server at session load time.

use std::collections::HashMap;
use std::collections::HashSet;

use chrono::{DateTime, Utc};

use devo_protocol::{ItemId, SessionId, TurnId, TurnKind, TurnStatus, TurnUsage};

use crate::durable_record::DurableRecord;
use crate::durable_record::GoalBudget;
use crate::durable_record::GoalId as DurableGoalId;
use crate::durable_record::GoalProgressType;
use crate::durable_record::GoalStatus as DurableGoalStatus;

// ── Projection Types ────────────────────────────────────────────────

/// Full replay projection for a session.
#[derive(Debug, Clone)]
pub struct ReplayProjection {
    pub session_id: SessionId,
    pub metadata: SessionProjectionMeta,
    pub turns: Vec<TurnProjection>,
    pub pending_items: Vec<PendingItemProjection>,
    pub usage_totals: UsageTotals,
    pub current_goal: Option<GoalReplayProjection>,
    pub goal_context_snapshots: Vec<GoalContextSnapshotProjection>,
}

/// Session-level metadata from replay.
#[derive(Debug, Clone)]
pub struct SessionProjectionMeta {
    pub workspace_root: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub model: Option<String>,
    pub turn_count: usize,
    pub is_active: bool,
}

/// Projected turn state.
#[derive(Debug, Clone)]
pub struct TurnProjection {
    pub turn_id: TurnId,
    pub sequence: u32,
    pub status: TurnStatus,
    pub kind: TurnKind,
    pub model: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub usage: Option<TurnUsage>,
    pub items: Vec<ItemProjection>,
}

/// Projected item state.
#[derive(Debug, Clone)]
pub struct ItemProjection {
    pub item_id: ItemId,
    pub kind: String,
    pub status: String,
    pub content_preview: String,
}

/// A pending (unterminated) item at replay time.
#[derive(Debug, Clone)]
pub struct PendingItemProjection {
    pub item_id: ItemId,
    pub turn_id: TurnId,
    pub kind: String,
}

/// Accumulated usage totals from replay.
#[derive(Debug, Clone, Default)]
pub struct UsageTotals {
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_tokens: i64,
    pub total_cache_read_tokens: i64,
    pub total_cache_creation_tokens: i64,
    pub total_reasoning_tokens: i64,
}

/// Current goal projection reconstructed from durable goal records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalReplayProjection {
    pub goal_id: DurableGoalId,
    pub session_id: SessionId,
    pub prompt: String,
    pub description: Option<String>,
    pub status: DurableGoalStatus,
    pub budget: Option<GoalBudget>,
    pub tokens_used: i64,
    pub turns_used: u32,
    pub time_used_seconds: u64,
    pub progress_summary: Option<String>,
    pub blocker_summary: Option<String>,
    pub verification_summary: Option<String>,
    pub updated_at: DateTime<Utc>,
}

/// Hidden goal context snapshot metadata reconstructed from durable records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalContextSnapshotProjection {
    pub goal_id: DurableGoalId,
    pub session_id: SessionId,
    pub snapshot_id: String,
    pub summary: String,
    pub recorded_at: DateTime<Utc>,
}

// ── Builder ─────────────────────────────────────────────────────────

/// Builds a ReplayProjection from a sequence of DurableRecords.
pub fn build_replay_projection(
    session_id: SessionId,
    records: &[DurableRecord],
) -> ReplayProjection {
    let mut meta = SessionProjectionMeta {
        workspace_root: None,
        created_at: None,
        model: None,
        turn_count: 0,
        is_active: false,
    };

    let _turns: Vec<TurnProjection> = Vec::new();
    let mut turn_map: HashMap<TurnId, TurnProjection> = HashMap::new();
    let mut pending_items: HashMap<ItemId, PendingItemProjection> = HashMap::new();
    let mut superseded_turn_ids: HashSet<TurnId> = HashSet::new();
    let mut usage_totals = UsageTotals::default();
    let mut current_goal: Option<GoalReplayProjection> = None;
    let mut goal_context_snapshots = Vec::new();

    for record in records {
        match record {
            DurableRecord::SessionCreated(r) => {
                meta.workspace_root = Some(r.workspace_root.clone());
                meta.created_at = Some(r.created_at);
            }

            DurableRecord::TurnStarted(r) => {
                let turn = TurnProjection {
                    turn_id: r.turn_id,
                    sequence: r.sequence,
                    status: TurnStatus::Running,
                    kind: r.kind.clone(),
                    model: r.model.clone(),
                    started_at: Some(r.started_at),
                    completed_at: None,
                    usage: None,
                    items: Vec::new(),
                };
                turn_map.insert(r.turn_id, turn);
                meta.turn_count += 1;
                meta.is_active = true;
            }

            DurableRecord::TurnCompleted(r) => {
                let usage = r.terminal.usage.clone();
                if let Some(turn) = turn_map.get_mut(&r.terminal.turn_id) {
                    turn.status = TurnStatus::Completed;
                    turn.completed_at = Some(r.terminal.completed_at);
                    turn.usage = usage.clone();
                    accumulate_usage(&mut usage_totals, &usage);
                }
                meta.is_active = false;
            }

            DurableRecord::TurnFailed(r) => {
                let usage = r.terminal.usage.clone();
                if let Some(turn) = turn_map.get_mut(&r.terminal.turn_id) {
                    turn.status = TurnStatus::Failed;
                    turn.completed_at = Some(r.terminal.completed_at);
                    turn.usage = usage;
                }
                meta.is_active = false;
            }

            DurableRecord::TurnInterrupted(r) => {
                let completed_at = r.terminal.completed_at;
                if let Some(turn) = turn_map.get_mut(&r.terminal.turn_id) {
                    turn.status = TurnStatus::Interrupted;
                    turn.completed_at = Some(completed_at);
                }
                meta.is_active = false;
            }

            DurableRecord::ItemStarted(r) => {
                let kind_str = format!("{:?}", r.kind).to_lowercase();
                let item_id = r.item_id;
                let turn_id = r.turn_id;
                pending_items.insert(
                    item_id,
                    PendingItemProjection {
                        item_id,
                        turn_id,
                        kind: kind_str.clone(),
                    },
                );
                if let Some(turn) = turn_map.get_mut(&turn_id) {
                    turn.items.push(ItemProjection {
                        item_id,
                        kind: kind_str,
                        status: "started".into(),
                        content_preview: String::new(),
                    });
                }
            }

            DurableRecord::ItemContentAppended(r) => {
                let item_id = r.item_id;
                let content = r.content.clone();
                if let Some(turn) = turn_map
                    .values_mut()
                    .find(|t| t.items.iter().any(|i| i.item_id == item_id))
                    && let Some(item) = turn.items.iter_mut().find(|i| i.item_id == item_id)
                    && item.content_preview.len() < 200
                {
                    item.content_preview.push_str(&content);
                }
            }

            DurableRecord::ItemCompleted(r) => {
                let item_id = r.item_id;
                pending_items.remove(&item_id);
                if let Some(turn) = turn_map
                    .values_mut()
                    .find(|t| t.items.iter().any(|i| i.item_id == item_id))
                    && let Some(item) = turn.items.iter_mut().find(|i| i.item_id == item_id)
                {
                    item.status = "completed".into();
                }
            }

            DurableRecord::ItemFailed(r) => {
                let item_id = r.item_id;
                pending_items.remove(&item_id);
                if let Some(turn) = turn_map
                    .values_mut()
                    .find(|t| t.items.iter().any(|i| i.item_id == item_id))
                    && let Some(item) = turn.items.iter_mut().find(|i| i.item_id == item_id)
                {
                    item.status = "failed".into();
                }
            }

            DurableRecord::UsageRecorded(r) => {
                for m in &r.metrics {
                    match m.metric_kind {
                        crate::durable_record::UsageMetricKind::InputTokens => {
                            usage_totals.total_input_tokens += m.value;
                        }
                        crate::durable_record::UsageMetricKind::OutputTokens => {
                            usage_totals.total_output_tokens += m.value;
                        }
                        crate::durable_record::UsageMetricKind::CacheReadInputTokens => {
                            usage_totals.total_cache_read_tokens += m.value;
                        }
                        crate::durable_record::UsageMetricKind::CacheCreationInputTokens => {
                            usage_totals.total_cache_creation_tokens += m.value;
                        }
                        crate::durable_record::UsageMetricKind::ReasoningOutputTokens => {
                            usage_totals.total_reasoning_tokens += m.value;
                        }
                        crate::durable_record::UsageMetricKind::TotalTokens => {
                            usage_totals.total_tokens += m.value;
                        }
                    }
                }
            }

            DurableRecord::GoalCreated(r) => {
                current_goal = Some(GoalReplayProjection {
                    goal_id: r.goal_id,
                    session_id: r.session_id,
                    prompt: r.prompt.clone(),
                    description: r.description.clone(),
                    status: DurableGoalStatus::Active,
                    budget: r.budget.clone(),
                    tokens_used: 0,
                    turns_used: 0,
                    time_used_seconds: 0,
                    progress_summary: None,
                    blocker_summary: None,
                    verification_summary: None,
                    updated_at: r.created_at,
                });
            }

            DurableRecord::GoalReplaced(r) => {
                current_goal = Some(GoalReplayProjection {
                    goal_id: r.goal_id,
                    session_id: r.session_id,
                    prompt: r.prompt.clone(),
                    description: r.description.clone(),
                    status: DurableGoalStatus::Active,
                    budget: None,
                    tokens_used: 0,
                    turns_used: 0,
                    time_used_seconds: 0,
                    progress_summary: None,
                    blocker_summary: None,
                    verification_summary: None,
                    updated_at: r.replaced_at,
                });
            }

            DurableRecord::GoalStatusChanged(r) => {
                if let Some(goal) = current_goal
                    .as_mut()
                    .filter(|goal| goal.goal_id == r.goal_id)
                {
                    goal.status = r.new_status;
                    goal.updated_at = r.changed_at;
                    if let Some(reason) = r.reason.clone() {
                        match r.new_status {
                            DurableGoalStatus::Completed => {
                                goal.verification_summary = Some(reason);
                            }
                            DurableGoalStatus::Blocked | DurableGoalStatus::Failed => {
                                goal.blocker_summary = Some(reason);
                            }
                            DurableGoalStatus::Active
                            | DurableGoalStatus::Paused
                            | DurableGoalStatus::Canceled
                            | DurableGoalStatus::Cleared => {}
                        }
                    }
                }
            }

            DurableRecord::GoalBudgetAccounted(r) => {
                if let Some(goal) = current_goal
                    .as_mut()
                    .filter(|goal| goal.goal_id == r.goal_id)
                {
                    goal.tokens_used += r.budget_delta.max_tokens.unwrap_or_default();
                    goal.turns_used += r.budget_delta.max_turns.unwrap_or_default();
                    goal.time_used_seconds +=
                        r.budget_delta.max_duration_seconds.unwrap_or_default();
                    goal.updated_at = r.recorded_at;
                }
            }

            DurableRecord::GoalProgressRecorded(r) => {
                if let Some(goal) = current_goal
                    .as_mut()
                    .filter(|goal| goal.goal_id == r.goal_id)
                {
                    match r.progress_type {
                        GoalProgressType::Blocked => {
                            goal.blocker_summary = Some(r.summary.clone());
                        }
                        GoalProgressType::Milestone
                        | GoalProgressType::PhaseComplete
                        | GoalProgressType::Note => {
                            goal.progress_summary = Some(r.summary.clone());
                        }
                    }
                    goal.updated_at = r.recorded_at;
                }
            }

            DurableRecord::GoalContextSnapshotRecorded(r) => {
                goal_context_snapshots.push(GoalContextSnapshotProjection {
                    goal_id: r.goal_id,
                    session_id: r.session_id,
                    snapshot_id: r.snapshot_id.clone(),
                    summary: r.summary.clone(),
                    recorded_at: r.recorded_at,
                });
            }

            DurableRecord::GoalCleared(r) => {
                if current_goal
                    .as_ref()
                    .is_some_and(|goal| goal.goal_id == r.goal_id)
                {
                    current_goal = None;
                }
            }

            DurableRecord::TurnSuperseded(r) => {
                superseded_turn_ids.insert(r.superseded_turn_id);
                pending_items.retain(|_, item| item.turn_id != r.superseded_turn_id);
                if meta.is_active
                    && turn_map
                        .get(&r.superseded_turn_id)
                        .is_some_and(|turn| turn.status == TurnStatus::Running)
                {
                    meta.is_active = false;
                }
            }

            _ => { /* other records don't affect core projection */ }
        }
    }

    // Sort turns by sequence
    let mut all_turns: Vec<TurnProjection> = turn_map.into_values().collect();
    all_turns.retain(|turn| !superseded_turn_ids.contains(&turn.turn_id));
    all_turns.sort_by_key(|t| t.sequence);
    meta.turn_count = all_turns.len();

    // Resolve unterminated state
    let pending: Vec<PendingItemProjection> = pending_items
        .into_values()
        .filter(|item| !superseded_turn_ids.contains(&item.turn_id))
        .collect();
    if !pending.is_empty() {
        meta.is_active = true;
    }
    for turn in all_turns.iter_mut() {
        if turn.status == TurnStatus::Running && turn.completed_at.is_none() {
            turn.status = TurnStatus::Interrupted;
        }
    }

    ReplayProjection {
        session_id,
        metadata: meta,
        turns: all_turns,
        pending_items: pending,
        usage_totals,
        current_goal,
        goal_context_snapshots,
    }
}

fn accumulate_usage(totals: &mut UsageTotals, usage: &Option<TurnUsage>) {
    if let Some(u) = usage {
        totals.total_input_tokens += u.input_tokens as i64;
        totals.total_output_tokens += u.output_tokens as i64;
        totals.total_tokens += u.display_total_tokens() as i64;
        totals.total_cache_creation_tokens += u.cache_creation_input_tokens.unwrap_or(0) as i64;
        totals.total_cache_read_tokens += u.cache_read_input_tokens.unwrap_or(0) as i64;
        totals.total_reasoning_tokens += u.reasoning_output_tokens.unwrap_or(0) as i64;
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable_record::*;
    use chrono::Utc;
    use devo_protocol::TurnId;
    use pretty_assertions::assert_eq;

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn empty_records_produces_empty_projection() {
        let projection = build_replay_projection(SessionId::new(), &[]);
        assert!(projection.turns.is_empty());
        assert!(projection.metadata.workspace_root.is_none());
    }

    #[test]
    fn session_created_sets_metadata() {
        let sid = SessionId::new();
        let records = vec![DurableRecord::SessionCreated(SessionCreatedRecord {
            schema_version: 1,
            session_id: sid,
            workspace_root: "/tmp/ws".into(),
            created_at: now(),
        })];
        let projection = build_replay_projection(sid, &records);
        assert_eq!(
            projection.metadata.workspace_root.as_deref(),
            Some("/tmp/ws")
        );
    }

    #[test]
    fn turn_lifecycle_replays() {
        let sid = SessionId::new();
        let tid = TurnId::new();
        let records = vec![
            DurableRecord::SessionCreated(SessionCreatedRecord {
                schema_version: 1,
                session_id: sid,
                workspace_root: "/tmp".into(),
                created_at: now(),
            }),
            DurableRecord::TurnStarted(TurnStartedRecord {
                schema_version: 1,
                session_id: sid,
                turn_id: tid,
                sequence: 0,
                status: TurnStatus::Running,
                kind: TurnKind::Regular,
                resume_of_turn_id: None,
                submitted_by_client_id: None,
                model: Some("test-model".into()),
                reasoning_effort_selection: None,
                reasoning_effort: None,
                started_at: now(),
            }),
            DurableRecord::TurnCompleted(TurnCompletedRecord {
                schema_version: 1,
                terminal: TurnTerminalFields {
                    turn_id: tid,
                    session_id: sid,
                    status: TurnStatus::Completed,
                    usage: Some(TurnUsage {
                        input_tokens: 100,
                        output_tokens: 50,
                        cache_creation_input_tokens: None,
                        cache_read_input_tokens: None,
                        reasoning_output_tokens: None,
                        total_tokens: None,
                    }),
                    workspace_change_set_id: None,
                    completed_at: now(),
                },
            }),
        ];
        let projection = build_replay_projection(sid, &records);
        assert_eq!(projection.turns.len(), 1);
        assert_eq!(projection.turns[0].status, TurnStatus::Completed);
        assert_eq!(projection.usage_totals.total_input_tokens, 100);
        assert!(!projection.metadata.is_active);
    }

    #[test]
    fn unterminated_turn_marked_interrupted() {
        let sid = SessionId::new();
        let tid = TurnId::new();
        let records = vec![DurableRecord::TurnStarted(TurnStartedRecord {
            schema_version: 1,
            session_id: sid,
            turn_id: tid,
            sequence: 0,
            status: TurnStatus::Running,
            kind: TurnKind::Regular,
            resume_of_turn_id: None,
            submitted_by_client_id: None,
            model: None,
            reasoning_effort_selection: None,
            reasoning_effort: None,
            started_at: now(),
        })];
        let projection = build_replay_projection(sid, &records);
        assert_eq!(projection.turns[0].status, TurnStatus::Interrupted);
    }

    #[test]
    fn items_attached_to_turns() {
        let sid = SessionId::new();
        let tid = TurnId::new();
        let iid = ItemId::new();
        let records = vec![
            DurableRecord::TurnStarted(TurnStartedRecord {
                schema_version: 1,
                session_id: sid,
                turn_id: tid,
                sequence: 0,
                status: TurnStatus::Running,
                kind: TurnKind::Regular,
                resume_of_turn_id: None,
                submitted_by_client_id: None,
                model: None,
                reasoning_effort_selection: None,
                reasoning_effort: None,
                started_at: now(),
            }),
            DurableRecord::ItemStarted(ItemStartedRecord {
                schema_version: 1,
                session_id: sid,
                turn_id: tid,
                item_id: iid,
                kind: ItemRecordKind::UserInput,
                role: RecordRole::User,
                content_parts: vec![],
                mentions: vec![],
                visibility: ItemVisibility::Visible,
                created_at: now(),
            }),
        ];
        let projection = build_replay_projection(sid, &records);
        assert_eq!(projection.turns[0].items.len(), 1);
        assert_eq!(projection.turns[0].items[0].item_id, iid);
        assert_eq!(projection.pending_items.len(), 1);
    }

    #[test]
    fn turn_superseded_projects_replacement_branch() {
        let sid = SessionId::new();
        let original_turn_id = TurnId::new();
        let replacement_turn_id = TurnId::new();
        let original_item_id = ItemId::new();
        let replacement_item_id = ItemId::new();
        let edit_id = EditId::new();
        let records = vec![
            DurableRecord::TurnStarted(TurnStartedRecord {
                schema_version: 1,
                session_id: sid,
                turn_id: original_turn_id,
                sequence: 0,
                status: TurnStatus::Running,
                kind: TurnKind::Regular,
                resume_of_turn_id: None,
                submitted_by_client_id: None,
                model: None,
                reasoning_effort_selection: None,
                reasoning_effort: None,
                started_at: now(),
            }),
            DurableRecord::ItemStarted(ItemStartedRecord {
                schema_version: 1,
                session_id: sid,
                turn_id: original_turn_id,
                item_id: original_item_id,
                kind: ItemRecordKind::UserInput,
                role: RecordRole::User,
                content_parts: vec![],
                mentions: vec![],
                visibility: ItemVisibility::Visible,
                created_at: now(),
            }),
            DurableRecord::TurnCompleted(TurnCompletedRecord {
                schema_version: 1,
                terminal: TurnTerminalFields {
                    turn_id: original_turn_id,
                    session_id: sid,
                    status: TurnStatus::Completed,
                    usage: None,
                    workspace_change_set_id: None,
                    completed_at: now(),
                },
            }),
            DurableRecord::MessageEditRecorded(MessageEditRecordedRecord {
                schema_version: 1,
                session_id: sid,
                edit_id,
                target_message_id: original_item_id,
                replacement_message_id: replacement_item_id,
                target_turn_id: Some(original_turn_id),
                replacement_turn_id: Some(replacement_turn_id),
                queue_item_id: None,
                edited_content_parts: vec![ContentPart::Text("edited".into())],
                edited_mentions: vec![],
                workspace_restore_policy: WorkspaceRestorePolicy::Skip,
                edit_state: EditState::Accepted,
                requested_by_client_id: None,
                created_at: now(),
            }),
            DurableRecord::TurnSuperseded(TurnSupersededRecord {
                schema_version: 1,
                session_id: sid,
                superseded_turn_id: original_turn_id,
                replacement_turn_id,
                edit_id,
                restore_id: None,
                reason: "message_edit_previous".into(),
                created_at: now(),
            }),
            DurableRecord::TurnStarted(TurnStartedRecord {
                schema_version: 1,
                session_id: sid,
                turn_id: replacement_turn_id,
                sequence: 1,
                status: TurnStatus::Running,
                kind: TurnKind::Regular,
                resume_of_turn_id: None,
                submitted_by_client_id: None,
                model: None,
                reasoning_effort_selection: None,
                reasoning_effort: None,
                started_at: now(),
            }),
            DurableRecord::ItemStarted(ItemStartedRecord {
                schema_version: 1,
                session_id: sid,
                turn_id: replacement_turn_id,
                item_id: replacement_item_id,
                kind: ItemRecordKind::UserInput,
                role: RecordRole::User,
                content_parts: vec![],
                mentions: vec![],
                visibility: ItemVisibility::Visible,
                created_at: now(),
            }),
        ];

        let projection = build_replay_projection(sid, &records);

        assert_eq!(
            projection
                .turns
                .iter()
                .map(|turn| turn.turn_id)
                .collect::<Vec<_>>(),
            vec![replacement_turn_id]
        );
        assert_eq!(projection.metadata.turn_count, 1);
        assert_eq!(
            projection
                .pending_items
                .iter()
                .map(|item| item.item_id)
                .collect::<Vec<_>>(),
            vec![replacement_item_id]
        );
    }

    #[test]
    fn usage_recorded_accumulates() {
        let sid = SessionId::new();
        let tid = TurnId::new();
        let records = vec![
            DurableRecord::TurnStarted(TurnStartedRecord {
                schema_version: 1,
                session_id: sid,
                turn_id: tid,
                sequence: 0,
                status: TurnStatus::Running,
                kind: TurnKind::Regular,
                resume_of_turn_id: None,
                submitted_by_client_id: None,
                model: None,
                reasoning_effort_selection: None,
                reasoning_effort: None,
                started_at: now(),
            }),
            DurableRecord::UsageRecorded(UsageRecordedRecord {
                schema_version: 1,
                session_id: sid,
                turn_id: tid,
                invocation_id: InvocationId::new(),
                model_binding_id: ModelBindingId::new(),
                canonical_model_slug: "test".into(),
                provider_id: ProviderId::new(),
                invocation_method: InvocationMethod::AnthropicMessages,
                reasoning_effort: None,
                metrics: vec![
                    UsageMetric {
                        metric_kind: UsageMetricKind::InputTokens,
                        value: 200,
                        source: MetricSource::ProviderReported,
                        confidence: MetricConfidence::High,
                        inclusion: MetricInclusion::Included,
                    },
                    UsageMetric {
                        metric_kind: UsageMetricKind::OutputTokens,
                        value: 100,
                        source: MetricSource::ProviderReported,
                        confidence: MetricConfidence::High,
                        inclusion: MetricInclusion::Included,
                    },
                    UsageMetric {
                        metric_kind: UsageMetricKind::ReasoningOutputTokens,
                        value: 30,
                        source: MetricSource::ProviderReported,
                        confidence: MetricConfidence::High,
                        inclusion: MetricInclusion::Included,
                    },
                ],
                context_pressure: ContextPressure {
                    context_size: 5000,
                    effective_limit: 200000,
                    pressure_state: ContextPressureState::Normal,
                    compaction_status: CompactionStatus::NotNeeded,
                },
                recorded_at: now(),
            }),
        ];
        let projection = build_replay_projection(sid, &records);
        assert_eq!(projection.usage_totals.total_input_tokens, 200);
        assert_eq!(projection.usage_totals.total_reasoning_tokens, 30);
    }

    #[test]
    fn goal_durable_records_roundtrip_and_replay_projection() {
        // Trace: L2-DES-GOAL-001
        let sid = SessionId::new();
        let goal_id = GoalId::new();
        let turn_id = TurnId::new();
        let created_at = now();
        let records = vec![
            DurableRecord::GoalCreated(GoalCreatedRecord {
                schema_version: 1,
                goal_id,
                session_id: sid,
                turn_id,
                prompt: "finish goal".into(),
                description: Some("verify fully".into()),
                max_iterations: None,
                budget: Some(GoalBudget {
                    max_turns: Some(3),
                    max_tokens: Some(100),
                    max_duration_seconds: Some(30),
                }),
                created_at,
            }),
            DurableRecord::GoalBudgetAccounted(GoalBudgetAccountedRecord {
                schema_version: 1,
                goal_id,
                session_id: sid,
                turn_id,
                budget_delta: GoalBudget {
                    max_turns: Some(1),
                    max_tokens: Some(12),
                    max_duration_seconds: Some(4),
                },
                remaining_budget: GoalBudget {
                    max_turns: Some(2),
                    max_tokens: Some(88),
                    max_duration_seconds: Some(26),
                },
                recorded_at: created_at + chrono::Duration::seconds(1),
            }),
            DurableRecord::GoalProgressRecorded(GoalProgressRecordedRecord {
                schema_version: 1,
                goal_id,
                session_id: sid,
                summary: "tests pass".into(),
                progress_type: GoalProgressType::Milestone,
                recorded_at: created_at + chrono::Duration::seconds(2),
            }),
            DurableRecord::GoalContextSnapshotRecorded(GoalContextSnapshotRecordedRecord {
                schema_version: 1,
                goal_id,
                session_id: sid,
                snapshot_id: "snapshot-1".into(),
                summary: "continuation prompt".into(),
                recorded_at: created_at + chrono::Duration::seconds(3),
            }),
            DurableRecord::GoalStatusChanged(GoalStatusChangedRecord {
                schema_version: 1,
                goal_id,
                session_id: sid,
                previous_status: GoalStatus::Active,
                new_status: GoalStatus::Completed,
                reason: Some("verified".into()),
                changed_at: created_at + chrono::Duration::seconds(4),
            }),
        ];
        let serialized = serde_json::to_string(&records[0]).expect("serialize");
        let restored: DurableRecord = serde_json::from_str(&serialized).expect("deserialize");
        let DurableRecord::GoalCreated(restored) = restored else {
            panic!("expected goal created record");
        };
        let DurableRecord::GoalCreated(expected) = &records[0] else {
            panic!("expected goal created record");
        };
        assert_eq!(&restored, expected);

        let projection = build_replay_projection(sid, &records);
        let goal = projection.current_goal.expect("goal projection");

        assert_eq!(goal.goal_id, goal_id);
        assert_eq!(goal.prompt, "finish goal");
        assert_eq!(goal.description.as_deref(), Some("verify fully"));
        assert_eq!(goal.status, GoalStatus::Completed);
        assert_eq!(goal.tokens_used, 12);
        assert_eq!(goal.turns_used, 1);
        assert_eq!(goal.time_used_seconds, 4);
        assert_eq!(goal.progress_summary.as_deref(), Some("tests pass"));
        assert_eq!(goal.verification_summary.as_deref(), Some("verified"));
        assert_eq!(projection.goal_context_snapshots.len(), 1);
        assert_eq!(
            projection.goal_context_snapshots[0].snapshot_id,
            "snapshot-1"
        );
    }

    #[test]
    fn goal_cleared_removes_current_goal_projection() {
        // Trace: L2-DES-GOAL-001
        let sid = SessionId::new();
        let goal_id = GoalId::new();
        let created_at = now();
        let records = vec![
            DurableRecord::GoalCreated(GoalCreatedRecord {
                schema_version: 1,
                goal_id,
                session_id: sid,
                turn_id: TurnId::new(),
                prompt: "finish goal".into(),
                description: None,
                max_iterations: None,
                budget: None,
                created_at,
            }),
            DurableRecord::GoalCleared(GoalClearedRecord {
                schema_version: 1,
                goal_id,
                session_id: sid,
                reason: Some("user clear".into()),
                cleared_at: created_at + chrono::Duration::seconds(1),
            }),
        ];

        let projection = build_replay_projection(sid, &records);

        assert_eq!(projection.current_goal, None);
    }
}
