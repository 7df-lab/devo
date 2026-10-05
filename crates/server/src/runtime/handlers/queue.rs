//! Handlers for the `session/queue/*` API (devo-api-design/01 §4.3).
//!
//! Queue entries are pre-items: editable, not yet in history, addressed by a
//! stable `queueItemId`. The in-memory `pending_turn_queue` is the source of
//! truth (mirrored in SQLite `pending_messages`); ops serialize per session
//! on the queue mutex, last-write-wins.

use std::collections::VecDeque;

use devo_core::{CollaborationMode, PendingInputItem, PendingInputKind, TurnExecutionMode};
use devo_protocol::native::event::ServerNotification;
use devo_protocol::native::ids::{
    QueueItemId, SessionId as NativeSessionId, TurnId as NativeTurnId,
};
use devo_protocol::native::item::UserInput;
use devo_protocol::native::queue::{QueueChange, QueueEntry};
use devo_protocol::native::rpc_turn::{
    SessionQueueListResult, SessionQueuePushParams, SessionQueuePushResult,
    SessionQueueRemoveResult, SessionQueueUpdateParams, SessionQueueUpdateResult, TurnSteerParams,
    TurnSteerResult,
};
use devo_protocol::native::turn::Turn as NativeTurn;

use super::super::*;

impl ServerRuntime {
    pub(crate) async fn handle_session_queue_push(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionQueuePushParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/queue/push params: {error}"),
                );
            }
        };
        let input_items = match normalize_user_inputs(&params.input) {
            Ok(items) => items,
            Err(message) => {
                return self.error_response(request_id, ProtocolErrorCode::InvalidParams, message);
            }
        };
        if input_items.is_empty() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::EmptyInput,
                "queue push input is empty",
            );
        }
        let legacy_session_id = params.session_id;

        // Idle vs busy is decided by the shared admission path: it starts a
        // new turn when the session is idle and queues otherwise. The
        // request itself stays Native; the internal domain params are not a
        // second wire protocol.
        let response = self
            .handle_turn_start_with_queue_policy_and_client_message_id(
                Some(connection_id),
                request_id.clone(),
                TurnStartParams {
                    session_id: legacy_session_id,
                    input: input_items,
                    model: None,
                    model_binding_id: None,
                    reasoning_effort_selection: None,
                    sandbox: None,
                    approval_policy: None,
                    cwd: None,
                    collaboration_mode: CollaborationMode::default(),
                    execution_mode: TurnExecutionMode::default(),
                },
                TurnStartQueuePolicy::Queue,
                params.client_user_message_id.clone(),
            )
            .await;
        if response.get("error").is_some() {
            return response;
        }
        match response["result"]["disposition"].as_str() {
            Some("started") => {
                let turn = self
                    .active_native_turn(legacy_session_id)
                    .await
                    .expect("a turn just started");
                serde_json::to_value(SuccessResponse {
                    id: request_id,
                    result: SessionQueuePushResult::Started {
                        turn: Box::new(turn),
                    },
                })
                .expect("serialize session/queue/push response")
            }
            Some("queued") => {
                let queued_id = response["result"]["queued_input_id"]
                    .as_str()
                    .expect("queued result carries queued_input_id")
                    .to_owned();
                let position = self
                    .session_turn_reservation_snapshot(legacy_session_id)
                    .await
                    .map(|reservation| {
                        // turn/start enqueues into the shared queue
                        // synchronously (appending to the back), so the
                        // entry's position is the current length.
                        reservation
                            .pending_turn_queue
                            .lock()
                            .expect("pending turn queue mutex should not be poisoned")
                            .len() as u32
                    })
                    .unwrap_or(1);
                // turn/start enqueues into the shared queue synchronously
                // now, but the response entry is still built from the
                // accepted input directly (session/queue/list reflects the
                // queue truth immediately after). `enqueued_at` is
                // approximate.
                let entry = QueueEntry {
                    queue_item_id: QueueItemId::from_string(queued_id.clone()),
                    position,
                    input: params.input.clone(),
                    preview: params
                        .input
                        .iter()
                        .find_map(|part| match part {
                            UserInput::Text { text } => Some(
                                text.lines()
                                    .next()
                                    .unwrap_or_default()
                                    .chars()
                                    .take(80)
                                    .collect(),
                            ),
                            _ => None,
                        })
                        .unwrap_or_default(),
                    enqueued_at: chrono::Utc::now(),
                };
                self.broadcast_queue_updated(
                    legacy_session_id,
                    QueueChange::Added,
                    entry.queue_item_id,
                    None,
                )
                .await;
                serde_json::to_value(SuccessResponse {
                    id: request_id,
                    result: SessionQueuePushResult::Queued {
                        entry: Box::new(entry),
                    },
                })
                .expect("serialize session/queue/push response")
            }
            _ => self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                "unexpected turn/start outcome for queue push",
            ),
        }
    }

    pub(crate) async fn handle_session_queue_list(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_turn::SessionQueueListParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid session/queue/list params: {error}"),
                    );
                }
            };
        let legacy_session_id = params.session_id;
        let Some(reservation) = self
            .session_turn_reservation_snapshot(legacy_session_id)
            .await
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let entries = native_queue_entries(
            &reservation
                .pending_turn_queue
                .lock()
                .expect("pending turn queue mutex should not be poisoned"),
        );
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: SessionQueueListResult { entries },
        })
        .expect("serialize session/queue/list response")
    }

    pub(crate) async fn handle_session_queue_update(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionQueueUpdateParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/queue/update params: {error}"),
                );
            }
        };
        let legacy_session_id = params.session_id;
        let Some(reservation) = self
            .session_turn_reservation_snapshot(legacy_session_id)
            .await
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let pending_id = params.queue_item_id;

        // Resolve the replacement input up front (skill resolution can
        // fail before any state changes).
        let new_kind = match &params.input {
            Some(input) => {
                let input_items = match normalize_user_inputs(input) {
                    Ok(items) => items,
                    Err(message) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::InvalidParams,
                            message,
                        );
                    }
                };
                if input_items.is_empty() {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::EmptyInput,
                        "queue update input is empty",
                    );
                }
                let workspace_root = reservation.summary.cwd.clone();
                let resolved = match reservation
                    .runtime_context
                    .resolve_input_items(&input_items, Some(workspace_root.as_path()))
                {
                    Ok(Some(resolved)) => resolved,
                    Ok(None) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::EmptyInput,
                            "queue update input is empty",
                        );
                    }
                    Err(error) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::InvalidParams,
                            format!("failed to resolve queue update input: {error}"),
                        );
                    }
                };
                let display_text =
                    super::super::items::render_input_items(&input_items).unwrap_or_default();
                Some(PendingInputKind::UserInput {
                    input: input_items,
                    display_text,
                    prompt_text: resolved.prompt_text,
                    prompt_messages: resolved.prompt_messages,
                    prompt_images: resolved.images,
                })
            }
            None => None,
        };

        let entry = {
            let mut queue = reservation
                .pending_turn_queue
                .lock()
                .expect("pending turn queue mutex should not be poisoned");
            let Some(index) = queue.iter().position(|item| item.id == pending_id) else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::QueueItemNotFound,
                    "queue item is no longer queued",
                );
            };
            if let Some(kind) = new_kind {
                queue[index].kind = kind;
            }
            if let Some(position) = params.position {
                let item = queue.remove(index).expect("index just validated");
                let target = (position.saturating_sub(1) as usize).min(queue.len());
                queue.insert(target, item);
            }
            native_queue_entries(&queue)
                .into_iter()
                .find(|entry| entry.queue_item_id == params.queue_item_id)
                .expect("entry just updated")
        };

        if !reservation.ephemeral {
            let ordered: Vec<PendingInputItem> = {
                reservation
                    .pending_turn_queue
                    .lock()
                    .expect("pending turn queue mutex should not be poisoned")
                    .iter()
                    .cloned()
                    .collect()
            };
            let updated = ordered
                .iter()
                .find(|item| item.id == pending_id)
                .expect("entry just updated");
            if let Err(error) =
                self.deps
                    .db
                    .update_pending_content(&legacy_session_id, QueueType::Turn, updated)
            {
                tracing::warn!(
                    session_id = %legacy_session_id,
                    error = %error,
                    "failed to persist queue entry update"
                );
            }
            if params.position.is_some() {
                let ordered_ids: Vec<QueueItemId> = ordered.iter().map(|item| item.id).collect();
                if let Err(error) = self.deps.db.set_pending_positions(
                    &legacy_session_id,
                    QueueType::Turn,
                    &ordered_ids,
                ) {
                    tracing::warn!(
                        session_id = %legacy_session_id,
                        error = %error,
                        "failed to persist queue reorder"
                    );
                }
            }
        }

        self.broadcast_queue_updated(
            legacy_session_id,
            QueueChange::Updated,
            entry.queue_item_id,
            None,
        )
        .await;
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: SessionQueueUpdateResult { entry },
        })
        .expect("serialize session/queue/update response")
    }

    pub(crate) async fn handle_session_queue_remove(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_turn::SessionQueueRemoveParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid session/queue/remove params: {error}"),
                    );
                }
            };
        let legacy_session_id = params.session_id;
        let pending_id = params.queue_item_id;
        // Remove through the shared queue mutex (01 §4.3 last-write-wins),
        // not an actor command: queue ops must stay zero-hop at decision points.
        let Some(reservation) = self
            .session_turn_reservation_snapshot(legacy_session_id)
            .await
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let removed = {
            let mut queue = reservation
                .pending_turn_queue
                .lock()
                .expect("pending turn queue mutex should not be poisoned");
            let Some(index) = queue.iter().position(|item| item.id == pending_id) else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::QueueItemNotFound,
                    "queue item is no longer queued",
                );
            };
            queue.remove(index).is_some()
        };
        if !removed {
            return self.error_response(
                request_id,
                ProtocolErrorCode::QueueItemNotFound,
                "queue item is no longer queued",
            );
        }
        if !reservation.ephemeral
            && let Err(error) =
                self.deps
                    .db
                    .remove_pending_by_id(&legacy_session_id, QueueType::Turn, &pending_id)
        {
            tracing::warn!(
                session_id = %legacy_session_id,
                error = %error,
                "failed to remove queue entry from database"
            );
        }
        self.broadcast_queue_updated(
            legacy_session_id,
            QueueChange::Removed,
            params.queue_item_id,
            None,
        )
        .await;
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: SessionQueueRemoveResult {},
        })
        .expect("serialize session/queue/remove response")
    }

    /// Native `turn/steer`: inject raw input into the active turn. If the turn
    /// ended before admission, degrade into `pending_turn_queue` (never lose
    /// the message) and return `DegradedToQueue`.
    pub(crate) async fn handle_turn_steer(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        self.handle_turn_steer_with_client_user_message_id(
            connection_id,
            request_id,
            params,
            /*client_user_message_id*/ None,
        )
        .await
    }

    pub(crate) async fn handle_turn_steer_with_client_user_message_id(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
        client_user_message_id: Option<String>,
    ) -> serde_json::Value {
        let params: TurnSteerParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid turn/steer params: {error}"),
                );
            }
        };
        let client_user_message_id = client_user_message_id
            .or_else(|| params.client_user_message_id.clone())
            .unwrap_or_else(|| {
                let idempotency_key = &params.idempotency_key;
                format!("turn/steer:{idempotency_key}")
            });
        let input_items = match normalize_user_inputs(&params.input) {
            Ok(items) => items,
            Err(message) => {
                return self.error_response(request_id, ProtocolErrorCode::InvalidParams, message);
            }
        };
        if input_items.is_empty() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::EmptyInput,
                "turn/steer input is empty",
            );
        }
        let Some(display_input) = crate::runtime::items::render_input_items(&input_items) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::EmptyInput,
                "turn/steer input is empty",
            );
        };
        let legacy_session_id = params.session_id;
        let Some(reservation) = self
            .session_turn_reservation_snapshot(legacy_session_id)
            .await
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let workspace_root = reservation.summary.cwd.clone();
        let resolved_input = match reservation
            .runtime_context
            .resolve_input_items(&input_items, Some(workspace_root.as_path()))
        {
            Ok(Some(resolved)) => resolved,
            Ok(None) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::EmptyInput,
                    "turn/steer input is empty",
                );
            }
            Err(error) => {
                let code = match error {
                    devo_core::SkillError::SkillNotFound { .. }
                    | devo_core::SkillError::AmbiguousSkillName { .. }
                    | devo_core::SkillError::SkillDisabled { .. } => {
                        ProtocolErrorCode::InvalidParams
                    }
                    devo_core::SkillError::SkillParseFailed { .. }
                    | devo_core::SkillError::SkillRootUnavailable { .. }
                    | devo_core::SkillError::DuplicateSkillId { .. } => {
                        ProtocolErrorCode::InternalError
                    }
                };
                return self.error_response(
                    request_id,
                    code,
                    format!("failed to resolve turn/steer input: {error}"),
                );
            }
        };

        let now = chrono::Utc::now();
        let item = PendingInputItem::new(
            PendingInputKind::UserInput {
                input: input_items.clone(),
                display_text: display_input.clone(),
                prompt_text: resolved_input.prompt_text.clone(),
                prompt_messages: resolved_input.prompt_messages.clone(),
                prompt_images: resolved_input.images.clone(),
            },
            Some(serde_json::json!({
                "clientUserMessageId": client_user_message_id.clone()
            })),
            now,
        );

        let Some(active_turn) = reservation.active_turn.as_ref() else {
            return self
                .degrade_steer_to_queue(
                    request_id,
                    legacy_session_id,
                    &reservation,
                    item,
                    reservation.ephemeral,
                )
                .await;
        };
        if active_turn.turn_id() != params.expected_turn_id {
            // The client's expectedTurnId may name a turn that already reached
            // a terminal state: either the session went idle and a follow-up
            // turn is now running (post-turn queue drain), or that turn's
            // completion is mid-finalize and the actor still books it as
            // active. Native semantics keep the message either way — degrade
            // into the queue instead of erroring. The hard mismatch error is
            // reserved for a genuinely different live turn.
            let expected_terminal = self
                .recent_terminal_turn_status(params.expected_turn_id)
                .await
                .is_some();
            let active_terminal = self
                .recent_terminal_turn_status(active_turn.turn_id())
                .await
                .is_some();
            if expected_terminal || active_terminal {
                return self
                    .degrade_steer_to_queue(
                        request_id,
                        legacy_session_id,
                        &reservation,
                        item,
                        reservation.ephemeral,
                    )
                    .await;
            }
            return self.error_response(
                request_id,
                ProtocolErrorCode::ExpectedTurnMismatch,
                "active turn did not match expectedTurnId",
            );
        }
        if active_turn.native.kind != devo_protocol::native::turn::TurnKind::Regular {
            // Goal continuations are autonomous and unbounded: degrading the
            // steer into the queue AND preempting the turn guarantees the
            // user's message runs next instead of waiting for a turn boundary
            // that never comes. Compaction turns stay rejected (bounded work;
            // the queue drains naturally after compaction).
            if active_turn.native.kind == devo_protocol::native::turn::TurnKind::GoalContinuation {
                self.preempt_goal_continuation_for_user_input(
                    legacy_session_id,
                    active_turn.turn_id(),
                );
                return self
                    .degrade_steer_to_queue(
                        request_id,
                        legacy_session_id,
                        &reservation,
                        item,
                        reservation.ephemeral,
                    )
                    .await;
            }
            return self.error_response(
                request_id,
                ProtocolErrorCode::ActiveTurnNotSteerable,
                "cannot steer a non-regular turn",
            );
        }
        // A turn parked at an approval checkpoint never consumes steered
        // input — it is blocked awaiting the permission decision, which can
        // outlive any client — so injecting into `steer_input_queue` would
        // strand the message. Preempt the parked turn and degrade into the
        // pending queue instead: the post-turn drain runs the message as a
        // fresh turn (same starvation rule as goal-continuation above).
        if self
            .session_interactive
            .has_pending_approval_for_host(legacy_session_id)
            .await
        {
            self.preempt_approval_parked_turn(legacy_session_id, active_turn.turn_id())
                .await;
            return self
                .degrade_steer_to_queue(
                    request_id,
                    legacy_session_id,
                    &reservation,
                    item,
                    reservation.ephemeral,
                )
                .await;
        }
        let native_session_id = reservation.summary.native.id;
        let native_turn_id = *active_turn.native_turn_id();

        if !reservation.ephemeral
            && let Err(error) =
                self.deps
                    .db
                    .push_pending(&legacy_session_id, QueueType::Steer, &item)
        {
            tracing::warn!(
                session_id = %legacy_session_id,
                error = %error,
                "failed to persist steer input to database"
            );
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist steer input: {error}"),
            );
        }
        reservation
            .steer_input_queue
            .lock()
            .expect("steer input queue mutex should not be poisoned")
            .push_back(item.clone());

        let native_item = crate::runtime::items::native_user_message_item_with_client_message_id(
            display_input,
            &[],
            devo_protocol::native::item::UserMessageEntry::Steer,
            client_user_message_id,
        );
        let (item_id, item_seq) = self
            .start_native_item(native_session_id, native_turn_id, native_item.clone())
            .await;
        self.complete_native_item(
            native_session_id,
            native_turn_id,
            item_id,
            item_seq,
            native_item,
        )
        .await;

        self.emit_notification_to_connection(
            connection_id,
            ServerNotification::ServerRequestResolved {
                session_id: legacy_session_id,
                request_id: "turn-steer-accepted".to_string(),
                turn_id: Some(native_turn_id),
            },
        )
        .await;
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: TurnSteerResult::Injected { item_id },
        })
        .expect("serialize turn/steer response")
    }

    async fn degrade_steer_to_queue(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        legacy_session_id: SessionId,
        reservation: &crate::runtime::session_actor::snapshots::TurnReservationSnapshot,
        item: PendingInputItem,
        ephemeral: bool,
    ) -> serde_json::Value {
        if !ephemeral
            && let Err(error) =
                self.deps
                    .db
                    .push_pending(&legacy_session_id, QueueType::Turn, &item)
        {
            tracing::warn!(
                session_id = %legacy_session_id,
                error = %error,
                "failed to persist degraded steer to database"
            );
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist degraded steer: {error}"),
            );
        }
        reservation
            .pending_turn_queue
            .lock()
            .expect("pending turn queue mutex should not be poisoned")
            .push_back(item.clone());
        let entries = native_queue_entries(
            &reservation
                .pending_turn_queue
                .lock()
                .expect("pending turn queue mutex should not be poisoned"),
        );
        let entry = entries
            .iter()
            .find(|entry| entry.queue_item_id == item.id)
            .cloned()
            .unwrap_or_else(|| QueueEntry {
                queue_item_id: item.id,
                position: entries.len().max(1) as u32,
                input: match &item.kind {
                    PendingInputKind::UserInput { input, .. } => input.clone(),
                    PendingInputKind::UserText { text } => {
                        vec![UserInput::Text { text: text.clone() }]
                    }
                    _ => Vec::new(),
                },
                preview: match &item.kind {
                    PendingInputKind::UserInput { display_text, .. } => display_text
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .chars()
                        .take(80)
                        .collect(),
                    PendingInputKind::UserText { text } => text
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .chars()
                        .take(80)
                        .collect(),
                    _ => String::new(),
                },
                enqueued_at: item.created_at,
            });
        self.broadcast_queue_updated(
            legacy_session_id,
            QueueChange::Added,
            entry.queue_item_id,
            None,
        )
        .await;
        // Degrading onto an idle (or about-to-be-idle) session must also kick
        // the drain: nothing else is guaranteed to fire before the next user
        // action, and a stranded entry keeps every later input queued behind
        // it. `drain_queue_if_idle` no-ops while a turn is still running.
        let drain_runtime = Arc::clone(self);
        let drain_session_id = legacy_session_id;
        tokio::spawn(async move {
            drain_runtime.drain_queue_if_idle(drain_session_id).await;
        });
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: TurnSteerResult::DegradedToQueue { entry },
        })
        .expect("serialize turn/steer degraded response")
    }

    /// Broadcasts one canonical `queue/updated` notification to connections
    /// subscribed to this session via the new subscription API.
    pub(crate) async fn broadcast_queue_updated(
        &self,
        session_id: SessionId,
        change: QueueChange,
        queue_item_id: QueueItemId,
        started_turn_id: Option<NativeTurnId>,
    ) {
        let session_id_string = session_id.to_string();
        let entries = self
            .session_turn_reservation_snapshot(session_id)
            .await
            .map(|reservation| {
                native_queue_entries(
                    &reservation
                        .pending_turn_queue
                        .lock()
                        .expect("pending turn queue mutex should not be poisoned"),
                )
            })
            .unwrap_or_default();
        let notification = ServerNotification::QueueUpdated {
            session_id: NativeSessionId::from_string(session_id_string.clone()),
            change,
            queue_item_id,
            started_turn_id,
            queue: entries,
        };
        let params = serde_json::to_value(&notification)
            .expect("serialize queue/updated notification")
            .get("params")
            .cloned()
            .unwrap_or_default();
        let mut connections = self.connections.lock().await;
        for (connection_id, connection) in connections.iter_mut() {
            let subscribed = connection.event_selectors.iter().any(|selector| {
                matches!(
                    selector,
                    devo_protocol::native::event::StreamSelector::Session { session_id }
                        if session_id.as_str() == session_id_string
                )
            });
            if !subscribed {
                continue;
            }
            let event_seq = connection.next_seq();
            let frame = super::super::outbound::OutboundFrame::notification(
                *connection_id,
                "queue/updated".to_string(),
                event_seq,
                params.clone(),
            );
            let _ = super::super::outbound::enqueue_outbound_notification(
                &connection.outbound_tx,
                frame,
                super::super::outbound::OutboundDeliveryPolicy::Reliable,
                "connection_notifications",
            )
            .await;
        }
    }

    /// The running turn as a canonical `Turn` (for queue/push Started).
    async fn active_native_turn(&self, session_id: SessionId) -> Option<NativeTurn> {
        let reservation = self.session_turn_reservation_snapshot(session_id).await?;
        let turn = reservation.active_turn.as_ref()?;
        Some(turn.native.clone())
    }
}

/// Normalize Native `UserInput` for turn/queue admission: decode Image
/// data-URIs to LocalImage temp paths; reject unsupported audio.
pub(crate) fn normalize_user_inputs(input: &[UserInput]) -> Result<Vec<UserInput>, String> {
    let mut items = Vec::with_capacity(input.len());
    for part in input {
        let item = match part {
            UserInput::Image {
                uri,
                mime_type,
                detail,
            } => {
                let path =
                    super::image_input::write_data_uri_image_to_temp(uri, mime_type.as_deref())?;
                UserInput::LocalImage {
                    path,
                    detail: *detail,
                }
            }
            UserInput::Audio { uri, .. } => {
                return Err(format!("unsupported input modality for queue: {uri}"));
            }
            other => other.clone(),
        };
        items.push(item);
    }
    Ok(items)
}

/// Builds the canonical queue view from the session's in-memory turn queue.
/// `queueItemId` is the stable pending-input id; `position` is 1-based in
/// current queue order; `preview` is the first 80 chars of the display text.
pub(crate) fn native_queue_entries(queue: &VecDeque<PendingInputItem>) -> Vec<QueueEntry> {
    queue
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let input: Vec<UserInput> = match &item.kind {
                PendingInputKind::UserText { text } => vec![UserInput::Text { text: text.clone() }],
                PendingInputKind::UserInput { input, .. } => input.clone(),
                _ => Vec::new(),
            };
            let display_text = match &item.kind {
                PendingInputKind::UserText { text } => text.as_str(),
                PendingInputKind::UserInput { display_text, .. } => display_text.as_str(),
                _ => "",
            };
            QueueEntry {
                queue_item_id: item.id,
                position: (index + 1) as u32,
                input,
                preview: display_text
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .take(80)
                    .collect(),
                enqueued_at: item.created_at,
            }
        })
        .collect()
}
