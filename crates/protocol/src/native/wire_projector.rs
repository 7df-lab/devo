//! Native wire helpers for first-party session-stream fan-out.
//!
//! Builds typed item envelopes and turn/session projections from runtime
//! metadata. Legacy ItemKind + JSON bag projection was deleted once emit
//! became Native-only.

use chrono::{DateTime, Utc};

use super::event::ServerNotification;
use super::ids::{ItemId, SessionId, TurnId};
use super::item::{ItemEnvelope, ItemState};

/// Builds the native typed envelope for one item event.
///
/// First-party emit always supplies Native ids and a Native `Item`.
/// `projected_at` stamps `updated_at` (and `created_at` when absent).
#[allow(clippy::too_many_arguments)]
pub fn typed_item_envelope(
    session_id: SessionId,
    turn_id: TurnId,
    item_id: ItemId,
    seq: u64,
    native_item: &crate::native::item::Item,
    state: ItemState,
    projected_at: DateTime<Utc>,
    created_at: Option<DateTime<Utc>>,
) -> ItemEnvelope {
    ItemEnvelope {
        id: item_id,
        session_id,
        turn_id,
        seq,
        revision: 1,
        created_at: created_at.unwrap_or(projected_at),
        updated_at: projected_at,
        state,
        item: native_item.clone(),
        parent_id: None,
    }
}

/// Serialize a Native [`ServerNotification`] into wire `(method, params)`.
pub fn wire_from_server_notification(
    notification: &ServerNotification,
) -> (String, serde_json::Value) {
    let tagged = serde_json::to_value(notification).expect("serialize ServerNotification");
    let method = tagged
        .get("method")
        .and_then(|method| method.as_str())
        .expect("ServerNotification method")
        .to_string();
    let params = tagged
        .get("params")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    (method, params)
}

/// Wrap an emit-site [`ItemEnvelope`] as the Native item lifecycle notification.
pub fn item_lifecycle_server_notification(
    envelope: &ItemEnvelope,
    completed: bool,
) -> ServerNotification {
    if completed {
        ServerNotification::ItemCompleted {
            item: Box::new(envelope.clone()),
        }
    } else {
        ServerNotification::ItemStarted {
            item: Box::new(envelope.clone()),
        }
    }
}

/// Running vs waiting birth state for an item start notification.
pub fn item_started_state(native_item: &crate::native::item::Item) -> ItemState {
    if matches!(
        native_item,
        crate::native::item::Item::Approval { decision: None, .. }
    ) {
        ItemState::Waiting
    } else {
        ItemState::Running
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::item::Item;
    use chrono::TimeZone;
    use pretty_assertions::assert_eq;
    use uuid::Uuid;

    #[test]
    fn typed_item_envelope_preserves_started_at_as_created_at() {
        let started = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let completed = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 14).unwrap();
        let envelope = typed_item_envelope(
            SessionId::from_legacy_uuid(Uuid::nil()),
            TurnId::from_legacy_uuid(Uuid::from_u128(1)),
            ItemId::from_legacy_uuid(Uuid::from_u128(2)),
            1,
            &Item::Reasoning {
                text: "thinking".into(),
                provider_payload_ref: None,
            },
            ItemState::Completed,
            completed,
            Some(started),
        );
        assert_eq!(envelope.created_at, started);
        assert_eq!(envelope.updated_at, completed);
    }

    #[test]
    fn typed_notification_projects_item_started_and_completed() {
        let session_id = crate::SessionId::new();
        let turn_id = crate::TurnId::new();
        let item_id = crate::ItemId::new();
        let native_item = Item::AssistantMessage { text: "hi".into() };
        let envelope = typed_item_envelope(
            session_id,
            turn_id,
            item_id,
            7,
            &native_item,
            ItemState::Completed,
            Utc::now(),
            None,
        );

        let (method, value) = wire_from_server_notification(&item_lifecycle_server_notification(
            &envelope, /*completed*/ true,
        ));
        assert_eq!(method, "item/completed");
        let projected: crate::native::item::ItemEnvelope =
            serde_json::from_value(value.get("item").cloned().expect("item"))
                .expect("deserialize native item envelope");
        assert_eq!(projected.id.as_str(), item_id.to_string());
        assert_eq!(projected.session_id.as_str(), session_id.to_string());
        assert_eq!(projected.turn_id.as_str(), turn_id.to_string());
        assert_eq!((projected.seq, projected.revision), (7, 1));
        assert_eq!(projected.state, ItemState::Completed);
        assert_eq!(projected.item, Item::AssistantMessage { text: "hi".into() });
    }

    /// Trace: L2-DES-APP-009
    /// Verifies: emit-site item-delta helper wires Native delta notifications
    /// carrying `chunk_index` / `base_revision`.
    #[test]
    fn item_delta_notification_wires_chunk_index() {
        let session_id = crate::native::ids::SessionId::new();
        let item_id = crate::native::ids::ItemId::new();
        let delta_notification = |kind, chunk_index| {
            crate::item_delta_notification(kind, session_id, item_id, chunk_index, "chunk")
        };

        let (method, value) = wire_from_server_notification(&delta_notification(
            crate::ItemDeltaKind::AgentMessageDelta,
            7,
        ));
        assert_eq!(method, "item/assistantMessage/delta");
        let delta: crate::native::event::ItemDelta =
            serde_json::from_value(value).expect("typed delta payload");
        assert_eq!(delta.chunk_index, 7);
        assert_eq!(delta.base_revision, 1);
        assert_eq!(delta.delta, "chunk");

        let (method, _) = wire_from_server_notification(&delta_notification(
            crate::ItemDeltaKind::CommandExecutionOutputDelta,
            0,
        ));
        assert_eq!(method, "item/commandExecution/outputDelta");

        let (method, _) = wire_from_server_notification(&delta_notification(
            crate::ItemDeltaKind::ReasoningTextDelta,
            3,
        ));
        assert_eq!(method, "item/reasoning/delta");

        let (method, _) =
            wire_from_server_notification(&delta_notification(crate::ItemDeltaKind::PlanDelta, 0));
        assert_eq!(method, "item/plan/delta");
    }

    /// Trace: L2-DES-APP-009
    /// Verifies: turn lifecycle notifications wire as native turn
    /// notifications; terminal states flow through `turn/completed`.
    #[test]
    fn turn_lifecycle_events_project_to_native_turns() {
        let session_id = crate::SessionId::new();
        let turn_id = crate::TurnId::new();
        let turn = crate::native::turn::Turn {
            id: turn_id,
            session_id,
            sequence: 3,
            kind: crate::native::turn::TurnKind::Regular,
            status: crate::native::turn::TurnStatus::InProgress,
            model: crate::native::model::ModelBinding {
                provider: "binding-1".into(),
                model: "kimi-k3".into(),
                variant: None,
                reasoning_effort: Some(crate::ReasoningEffort::High),
            },
            collaboration_mode: None,
            started_at: Utc::now(),
            completed_at: None,
            error: None,
            usage: None,
        };

        let (method, value) = wire_from_server_notification(&ServerNotification::TurnStarted {
            turn: Box::new(turn.clone()),
        });
        assert_eq!(method, "turn/started");
        let projected: crate::native::turn::Turn =
            serde_json::from_value(value["turn"].clone()).expect("native turn");
        assert_eq!(projected.sequence, 3);
        assert_eq!(
            projected.status,
            crate::native::turn::TurnStatus::InProgress
        );
        assert_eq!(projected.model.provider, "binding-1");
        assert_eq!(
            projected.model.reasoning_effort,
            Some(crate::ReasoningEffort::High)
        );

        let mut failed_turn = turn.clone();
        failed_turn.status = crate::native::turn::TurnStatus::Failed;
        failed_turn.error = Some(crate::native::error::AgentError::new("E_BROKE", "it broke"));
        let (method, value) = wire_from_server_notification(&ServerNotification::TurnCompleted {
            turn: Box::new(failed_turn),
        });
        assert_eq!(method, "turn/completed");
        let projected: crate::native::turn::Turn =
            serde_json::from_value(value["turn"].clone()).expect("native turn");
        assert_eq!(projected.status, crate::native::turn::TurnStatus::Failed);
        assert_eq!(
            projected.error.expect("error").message,
            "it broke".to_string()
        );
    }

    #[test]
    fn live_session_status_and_delete_events_use_native_shapes() {
        let session_id = crate::SessionId::new();
        let native_session_id = session_id;
        let (method, value) =
            wire_from_server_notification(&ServerNotification::session_status_changed(
                native_session_id,
                crate::native::session::SessionStatus::Active,
                /*active_turn_id*/ None,
            ));
        assert_eq!(method, "session/statusChanged");
        assert_eq!(
            value,
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "status": "active",
                "flags": [],
                "activeTurnId": null,
                "activity": "working",
            })
        );

        let child_id = crate::SessionId::new();
        let (method, value) = wire_from_server_notification(&ServerNotification::SessionDeleted {
            session_id: native_session_id,
            deleted_session_ids: vec![native_session_id, child_id],
        });
        assert_eq!(method, "session/deleted");
        assert_eq!(
            value,
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "deletedSessionIds": [session_id.to_string(), child_id.to_string()],
            })
        );
    }

    /// Trace: L2-DES-APP-009
    /// Verifies: context usage notifications wire as `context/usageUpdated`
    /// carrying the occupancy unchanged.
    #[test]
    fn context_usage_projects_to_native_notification() {
        let occupancy = crate::native::item::ContextOccupancy {
            total_tokens: 12345,
            context_window_tokens: 200_000,
            categories: Vec::new(),
        };
        let session_id = crate::SessionId::new();
        let (method, value) =
            wire_from_server_notification(&ServerNotification::ContextUsageUpdated {
                session_id,
                occupancy: occupancy.clone(),
            });
        assert_eq!(method, "context/usageUpdated");
        let projected: crate::native::item::ContextOccupancy =
            serde_json::from_value(value["occupancy"].clone()).expect("occupancy payload");
        assert_eq!(projected, occupancy);
    }

    /// Trace: L2-DES-APP-009
    /// Verifies: the mid-turn usage meter projects to native
    /// `turn/usage/updated` with per-query totals, the last-query meter, and
    /// the context window (ratified vocabulary).
    #[test]
    fn turn_usage_meter_projects_to_native_notification() {
        let session_id = crate::SessionId::new();
        let turn_id = crate::TurnId::new();
        let notification = ServerNotification::TurnUsageUpdated {
            session_id,
            turn_id,
            usage: crate::native::usage::TurnUsage {
                query: crate::native::usage::UsageTotals {
                    total_tokens: 162,
                    input_tokens: 100,
                    output_tokens: 40,
                    reasoning_tokens: 7,
                    cache_read_input_tokens: 10,
                    cache_creation_input_tokens: 5,
                    call_count: 0,
                    metered_call_count: 0,
                    failed_call_count: 0,
                    cancelled_call_count: 0,
                    estimated_cost: None,
                },
                overhead: crate::native::usage::UsageTotals {
                    total_tokens: 0,
                    input_tokens: 0,
                    output_tokens: 0,
                    reasoning_tokens: 0,
                    cache_read_input_tokens: 0,
                    cache_creation_input_tokens: 0,
                    call_count: 0,
                    metered_call_count: 0,
                    failed_call_count: 0,
                    cancelled_call_count: 0,
                    estimated_cost: None,
                },
            },
            last_query_input_tokens: 96,
            session_totals: Some(crate::native::usage::UsageTotals {
                total_tokens: 700,
                input_tokens: 500,
                output_tokens: 200,
                reasoning_tokens: 0,
                cache_read_input_tokens: 50,
                cache_creation_input_tokens: 0,
                call_count: 0,
                metered_call_count: 0,
                failed_call_count: 0,
                cancelled_call_count: 0,
                estimated_cost: None,
            }),
            context_window: Some(200_000),
        };
        let (method, value) = wire_from_server_notification(&notification);
        assert_eq!(method, "turn/usage/updated");
        assert_eq!(value["turnId"].as_str(), Some(turn_id.to_string().as_str()));
        assert_eq!(value["usage"]["query"]["inputTokens"].as_u64(), Some(100));
        assert_eq!(value["usage"]["query"]["totalTokens"].as_u64(), Some(162));
        assert_eq!(value["usage"]["query"]["reasoningTokens"].as_u64(), Some(7));
        assert_eq!(value["lastQueryInputTokens"].as_u64(), Some(96));
        assert_eq!(value["contextWindow"].as_u64(), Some(200_000));
    }

    /// Trace: L2-DES-APP-009
    /// Verifies: provider retry wires as native `model/queryRetrying` with
    /// provider/model/phase carried through.
    #[test]
    fn provider_retry_projects_with_provider_model_phase() {
        let session_id = crate::SessionId::new();
        let turn_id = crate::TurnId::new();
        let mut error = crate::native::error::AgentError::new(
            crate::native::error::codes::PROVIDER_TEMPORARY_FAILURE.to_string(),
            "rate limited".to_string(),
        );
        error.retryable = true;
        error.retry_after_ms = Some(1500);
        let (method, value) =
            wire_from_server_notification(&ServerNotification::ModelQueryRetrying {
                session_id,
                turn_id,
                attempt: 2,
                max_attempts: 5,
                next_delay_ms: 1500,
                error,
                provider: Some("openai".to_string()),
                model: Some("gpt-5".to_string()),
                phase: Some(crate::native::event::ModelQueryRetryPhase::Scheduled),
            });
        assert_eq!(method, "model/queryRetrying");
        assert_eq!(value["attempt"].as_u64(), Some(2));
        assert_eq!(value["maxAttempts"].as_u64(), Some(5));
        assert_eq!(value["nextDelayMs"].as_u64(), Some(1500));
        assert_eq!(value["provider"].as_str(), Some("openai"));
        assert_eq!(value["model"].as_str(), Some("gpt-5"));
        assert_eq!(value["phase"].as_str(), Some("scheduled"));
        assert_eq!(
            value["error"]["errorCode"].as_str(),
            Some("PROVIDER_TEMPORARY_FAILURE")
        );
        assert_eq!(value["error"]["message"].as_str(), Some("rate limited"));
        assert_eq!(value["error"]["retryable"].as_bool(), Some(true));
    }

    /// Trace: L2-DES-APP-009
    /// Verifies: plan updates project as a full native `Plan` item on
    /// `item/updated` (replace-by-revision, no plan-delta granularity).
    #[test]
    fn plan_update_projects_as_full_plan_item() {
        let turn_id = crate::TurnId::new();
        let session_id = crate::SessionId::new();
        let now = Utc::now();
        let item_id = crate::ItemId::new();
        let notification = ServerNotification::ItemUpdated {
            item: Box::new(ItemEnvelope {
                id: item_id,
                session_id,
                turn_id,
                seq: 0,
                revision: 1,
                created_at: now,
                updated_at: now,
                state: ItemState::Running,
                item: crate::native::item::Item::Plan {
                    call_id: None,
                    entries: vec![
                        crate::native::item::PlanEntry {
                            step: "explore".to_string(),
                            status: crate::native::item::PlanStepStatus::Completed,
                        },
                        crate::native::item::PlanEntry {
                            step: "implement".to_string(),
                            status: crate::native::item::PlanStepStatus::InProgress,
                        },
                        crate::native::item::PlanEntry {
                            step: "verify".to_string(),
                            status: crate::native::item::PlanStepStatus::Pending,
                        },
                    ],
                },
                parent_id: None,
            }),
        };
        let (method, value) = wire_from_server_notification(&notification);
        assert_eq!(method, "item/updated");
        let item = &value["item"];
        assert_eq!(item["id"].as_str(), Some(item_id.as_str()));
        let entries = item["item"]["entries"].as_array().expect("plan entries");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0]["status"].as_str(), Some("completed"));
        assert_eq!(entries[1]["status"].as_str(), Some("inProgress"));
        assert_eq!(entries[2]["status"].as_str(), Some("pending"));
        assert_eq!(entries[1]["step"].as_str(), Some("implement"));
    }
}
