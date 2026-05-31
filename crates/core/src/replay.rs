//! Replay projection builder.
//!
//! Implements L3-BEH-CORE-001 §5. Consumes raw DurableRecords from the
//! JSONL store and builds SessionMetadata, TurnProjections, and other
//! projections needed by the server at session load time.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use devo_protocol::{ItemId, SessionId, TurnId, TurnKind, TurnStatus, TurnUsage};

use crate::durable_record::DurableRecord;

// ── Projection Types ────────────────────────────────────────────────

/// Full replay projection for a session.
#[derive(Debug, Clone)]
pub struct ReplayProjection {
    pub session_id: SessionId,
    pub metadata: SessionProjectionMeta,
    pub turns: Vec<TurnProjection>,
    pub pending_items: Vec<PendingItemProjection>,
    pub usage_totals: UsageTotals,
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
    pub total_cache_read_tokens: i64,
    pub total_cache_creation_tokens: i64,
    pub total_reasoning_tokens: i64,
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
    let mut usage_totals = UsageTotals::default();

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
                        _ => {}
                    }
                }
            }

            _ => { /* other records don't affect core projection */ }
        }
    }

    // Sort turns by sequence
    let mut all_turns: Vec<TurnProjection> = turn_map.into_values().collect();
    all_turns.sort_by_key(|t| t.sequence);

    // Resolve unterminated state
    let pending: Vec<PendingItemProjection> = pending_items.into_values().collect();
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
    }
}

fn accumulate_usage(totals: &mut UsageTotals, usage: &Option<TurnUsage>) {
    if let Some(u) = usage {
        totals.total_input_tokens += u.input_tokens as i64;
        totals.total_output_tokens += u.output_tokens as i64;
        totals.total_cache_creation_tokens += u.cache_creation_input_tokens.unwrap_or(0) as i64;
        totals.total_cache_read_tokens += u.cache_read_input_tokens.unwrap_or(0) as i64;
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable_record::*;
    use chrono::Utc;
    use devo_protocol::TurnId;

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
                thinking: None,
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
            thinking: None,
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
                thinking: None,
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
                thinking: None,
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
}
