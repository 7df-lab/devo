//! Handlers for the `subscription/*` API (devo-api-design/08 §4).
//!
//! The core invariant is the barrier/snapshot critical section (documented
//! on `handle_subscription_create`): snapshot + replay covers everything up
//! to the barrier seq, live delivery covers everything after it. The v1
//! notification set is full-snapshot replace-by-id, so the narrow race
//! window left (event committed just before the barrier read, delivered
//! right after registration) is at worst a benign redelivery.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use devo_core::event_projection::{session_stream_id, sessions_stream_id};
use devo_protocol::native::error::AgentError;
use devo_protocol::native::error::codes;
use devo_protocol::native::event::{
    ControlRequestKind, EventCursor, EventEnvelope, PendingControlRequest, SnapshotData,
    StreamSelector, StreamSnapshot, SubscriptionAckParams, SubscriptionCreateParams,
    SubscriptionCreateResult, SubscriptionUnsubscribeParams, SubscriptionUpdateParams,
};
use devo_protocol::native::ids::{
    SessionId as NativeSessionId, SubscriptionId, TurnId as NativeTurnId,
};
use devo_protocol::native::item::{ApprovalTarget, Item, ItemEnvelope, ItemState, UserInput};
use devo_protocol::native::queue::QueueEntry;
use devo_protocol::native::rpc_admin::RuntimePingResult;
use devo_protocol::native::session::SessionStatus;

use super::super::*;
use super::queue::native_queue_entries;
use crate::db::QueueType;

/// One server-side subscription record (registry entry, 08 §4).
#[derive(Debug)]
pub(crate) struct EventSubscription {
    pub(crate) connection_id: u64,
    pub(crate) selectors: Vec<StreamSelector>,
    /// Highest acked seq per stream; monotonic. ack is also the future
    /// basis for truncating the persisted log (no truncation in v1) and for
    /// lease expiry (`last_ack_at`).
    pub(crate) acked: HashMap<String, u64>,
    pub(crate) last_ack_at: Option<DateTime<Utc>>,
}

/// Computes the whitelisted stream id for one selector (08 §2). P4 clients
/// use the same helpers (`devo_core::conversation::event_projection`).
pub(crate) fn selector_stream_id(selector: &StreamSelector) -> String {
    match selector {
        StreamSelector::SessionsByCwd { cwd } => sessions_stream_id(&cwd.to_string_lossy()),
        StreamSelector::Session { session_id } => session_stream_id(session_id),
        StreamSelector::BackgroundTask { item_id } => format!("task:{item_id}"),
    }
}

/// Whether a live server notification matches any of the connection's
/// new-style subscription selectors. Unioned with the legacy subscription
/// filter at the fan-out (legacy clients are unaffected).
pub(crate) fn notification_matches_selectors(
    selectors: &[StreamSelector],
    notification: &devo_protocol::native::event::ServerNotification,
    event_cwd: Option<&std::path::Path>,
) -> bool {
    use devo_protocol::native::event::ServerNotification;
    use devo_protocol::native::notification_bus::notification_native_session_id;

    selectors.iter().any(|selector| match selector {
        StreamSelector::Session { session_id } => notification_native_session_id(notification)
            .is_some_and(|id| id.as_str() == session_id.as_str()),
        StreamSelector::SessionsByCwd { cwd } => {
            matches!(
                notification,
                ServerNotification::SessionCreated { .. }
                    | ServerNotification::SessionArchived { .. }
                    | ServerNotification::SessionDeleted { .. }
                    | ServerNotification::SessionMetadataUpdated { .. }
            ) && event_cwd == Some(cwd.as_path())
        }
        StreamSelector::BackgroundTask { .. } => false,
    })
}

impl ServerRuntime {
    pub(crate) async fn handle_subscription_create(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SubscriptionCreateParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid subscription/create params: {error}"),
                );
            }
        };
        let subscription_id = SubscriptionId::new();

        // CRITICAL SECTION (08 §4): the connections lock is held across the
        // barrier read, replay collection, and registration. Live delivery
        // takes the same lock, so no event reaches this connection between
        // the barrier read and the registration — events with seq ≤ barrier
        // are covered by snapshot+replay, later events arrive live.
        let mut connections = self.connections.lock().await;
        if !connections.contains_key(&connection_id) {
            return self.error_response(
                request_id,
                ProtocolErrorCode::NotInitialized,
                "connection is not registered",
            );
        }
        let mut result =
            match self.prepare_subscription(&request_id, &params.selectors, &params.after) {
                Ok(result) => result,
                Err(response) => return response,
            };
        if params.include_snapshot {
            for selector in &params.selectors {
                let barrier = result
                    .cursors
                    .iter()
                    .find(|cursor| cursor.stream_id == selector_stream_id(selector))
                    .map(|cursor| cursor.seq)
                    .unwrap_or(0);
                match self.build_snapshot(selector, barrier).await {
                    Ok(Some(snapshot)) => result.snapshots.push(snapshot),
                    Ok(None) => {}
                    Err(message) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::InternalError,
                            message,
                        );
                    }
                }
            }
        }
        result.pending_control_requests = self.pending_control_requests(&params.selectors).await;
        if !params.after.is_empty() {
            result.recovery_snapshots = self.recovery_snapshots(&params.selectors).await;
        }

        // Seed the ack floor with the create-time barriers: create already
        // handed the client everything up to `cursors`, so a later ack below
        // that floor would move the durable ack cursor backward (replay
        // duplication after reconnect). Monotonic by the struct contract.
        let acked = result
            .cursors
            .iter()
            .map(|cursor| (cursor.stream_id.clone(), cursor.seq))
            .collect();
        self.event_subscriptions.lock().await.insert(
            subscription_id.as_str().to_owned(),
            EventSubscription {
                connection_id,
                selectors: params.selectors.clone(),
                acked,
                last_ack_at: None,
            },
        );
        // The connection's cached selector view must stay the UNION of all
        // its live subscriptions. Replacing it with just this subscription's
        // selectors silently stranded approvals: the TUI's session
        // subscription was clobbered by the roster's SessionsByCwd
        // subscription, after which `controlling_connection_ids` no longer
        // routed permission requests to it ("no client connection is
        // available for permission request").
        self.refresh_connection_selectors(&mut connections, connection_id)
            .await;
        self.refresh_cwd_selector_count().await;
        drop(connections);
        result.pending_control_requests = self
            .reissue_pending_control_requests(connection_id, result.pending_control_requests)
            .await;

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: SubscriptionCreateResult {
                subscription_id,
                ..result
            },
        })
        .expect("serialize subscription/create response")
    }

    pub(crate) async fn handle_subscription_update(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SubscriptionUpdateParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid subscription/update params: {error}"),
                );
            }
        };
        // Same critical section as create: newly added streams get
        // barrier-consistent cursors atomically with the selector swap.
        let mut connections = self.connections.lock().await;
        let mut subscriptions = self.event_subscriptions.lock().await;
        let Some(subscription) = subscriptions.get_mut(params.subscription_id.as_str()) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "unknown subscription id",
            );
        };
        if subscription.connection_id != connection_id {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "unknown subscription id",
            );
        }
        let mut result = match self.prepare_subscription(&request_id, &params.selectors, &[]) {
            Ok(result) => result,
            Err(response) => return response,
        };
        result.subscription_id = params.subscription_id;
        subscription.selectors = params.selectors.clone();
        drop(subscriptions);
        // Same union rule as subscription/create: the cached view must
        // reflect every live subscription of this connection, not just the
        // one being updated.
        self.refresh_connection_selectors(&mut connections, connection_id)
            .await;
        self.refresh_cwd_selector_count().await;
        drop(connections);

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result,
        })
        .expect("serialize subscription/update response")
    }

    pub(crate) async fn handle_subscription_ack(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SubscriptionAckParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid subscription/ack params: {error}"),
                );
            }
        };
        let mut subscriptions = self.event_subscriptions.lock().await;
        let Some(subscription) = subscriptions.get_mut(params.subscription_id.as_str()) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "unknown subscription id",
            );
        };
        if subscription.connection_id != connection_id {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "unknown subscription id",
            );
        }
        let selector_streams: Vec<String> = subscription
            .selectors
            .iter()
            .map(selector_stream_id)
            .collect();
        for cursor in &params.cursors {
            if !selector_streams.contains(&cursor.stream_id) {
                return self.cursor_expired_response(
                    request_id,
                    format!(
                        "stream {} is not part of the subscription",
                        cursor.stream_id
                    ),
                );
            }
            let barrier = match self.deps.db.event_log_max_seq(&cursor.stream_id) {
                Ok(barrier) => barrier.unwrap_or(0),
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        format!("failed to read stream barrier: {error}"),
                    );
                }
            };
            let acked = subscription
                .acked
                .get(&cursor.stream_id)
                .cloned()
                .unwrap_or(0);
            if cursor.seq < acked || cursor.seq > barrier {
                return self.cursor_expired_response(
                    request_id,
                    format!(
                        "cursor {} for stream {} is outside (acked {acked}, barrier {barrier}]",
                        cursor.seq, cursor.stream_id
                    ),
                );
            }
            subscription
                .acked
                .insert(cursor.stream_id.clone(), cursor.seq);
        }
        subscription.last_ack_at = Some(Utc::now());
        drop(subscriptions);

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: RuntimePingResult {
                server_time_ms: Utc::now().timestamp_millis(),
            },
        })
        .expect("serialize subscription/ack response")
    }

    pub(crate) async fn handle_subscription_unsubscribe(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SubscriptionUnsubscribeParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid subscription/unsubscribe params: {error}"),
                );
            }
        };
        let mut connections = self.connections.lock().await;
        let mut subscriptions = self.event_subscriptions.lock().await;
        let removed = subscriptions.remove(params.subscription_id.as_str());
        match removed {
            Some(subscription) if subscription.connection_id == connection_id => {}
            _ => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    "unknown subscription id",
                );
            }
        }
        drop(subscriptions);
        self.refresh_connection_selectors(&mut connections, connection_id)
            .await;
        self.refresh_cwd_selector_count().await;
        drop(connections);

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: RuntimePingResult {
                server_time_ms: Utc::now().timestamp_millis(),
            },
        })
        .expect("serialize subscription/unsubscribe response")
    }

    /// Drops every new-style subscription of a closed connection.
    pub(crate) async fn drop_event_subscriptions_for_connection(&self, connection_id: u64) {
        self.event_subscriptions
            .lock()
            .await
            .retain(|_, subscription| subscription.connection_id != connection_id);
        self.refresh_cwd_selector_count().await;
    }

    /// Full in-flight item text for reconnect. `nextChunkIndex` is 0 because
    /// the snapshot replaces accumulated channel text rather than resuming
    /// a lost delta sequence.
    async fn recovery_snapshots(
        &self,
        selectors: &[StreamSelector],
    ) -> Vec<devo_protocol::native::event::LiveItemSnapshot> {
        use devo_protocol::native::event::DeltaChannel;

        let mut snapshots = Vec::new();
        for selector in selectors {
            let StreamSelector::Session { session_id } = selector else {
                continue;
            };
            let Some(handle) = self.session(*session_id).await else {
                continue;
            };
            let Some(deferred) = handle.take_shutdown_deferred_snapshot().await else {
                continue;
            };
            let Some(turn_id) = deferred.active_turn_id else {
                continue;
            };
            if let Some((item_id, seq, text)) = deferred.deferred_assistant {
                snapshots.push(super::native_surface::live_item_snapshot(
                    *session_id,
                    turn_id,
                    item_id,
                    seq,
                    Item::AssistantMessage { text: text.clone() },
                    DeltaChannel::AssistantMessage,
                    text,
                ));
            }
            if let Some((item_id, seq, text)) = deferred.deferred_reasoning {
                snapshots.push(super::native_surface::live_item_snapshot(
                    *session_id,
                    turn_id,
                    item_id,
                    seq,
                    Item::Reasoning {
                        text: text.clone(),
                        provider_payload_ref: None,
                    },
                    DeltaChannel::Reasoning,
                    text,
                ));
            }
        }
        snapshots
    }

    /// Computes barriers, validates `after` cursors, and collects replay for
    /// one selector set. Must be called inside the critical section (see
    /// `handle_subscription_create`). The `Err` variant is a ready-made
    /// JSON-RPC error response.
    fn prepare_subscription(
        &self,
        request_id: &serde_json::Value,
        selectors: &[StreamSelector],
        after: &[EventCursor],
    ) -> Result<SubscriptionCreateResult, serde_json::Value> {
        let mut result = SubscriptionCreateResult {
            subscription_id: SubscriptionId::new(),
            snapshots: Vec::new(),
            replay: Vec::new(),
            recovery_snapshots: Vec::new(),
            cursors: Vec::new(),
            pending_control_requests: Vec::new(),
        };
        for selector in selectors {
            let stream_id = selector_stream_id(selector);
            let barrier = self
                .deps
                .db
                .event_log_max_seq(&stream_id)
                .map_err(|error| {
                    self.error_response(
                        request_id.clone(),
                        ProtocolErrorCode::InternalError,
                        format!("failed to read stream barrier: {error}"),
                    )
                })?
                .unwrap_or(0);
            let after_seq = after
                .iter()
                .find(|cursor| cursor.stream_id == stream_id)
                .map(|cursor| cursor.seq)
                .unwrap_or(0);
            // A cursor from the future means the log was rebuilt (or never
            // contained the stream): the client must re-snapshot (08 §4).
            if after_seq > barrier {
                return Err(self.cursor_expired_response(
                    request_id.clone(),
                    format!(
                        "cursor {after_seq} for stream {stream_id} is past the log barrier {barrier}"
                    ),
                ));
            }
            let rows = self
                .deps
                .db
                .event_log_rows(&stream_id, after_seq)
                .map_err(|error| {
                    self.error_response(
                        request_id.clone(),
                        ProtocolErrorCode::InternalError,
                        format!("failed to read event log: {error}"),
                    )
                })?;
            for row in rows {
                let mut envelope: EventEnvelope =
                    serde_json::from_str(&row.payload).map_err(|error| {
                        self.error_response(
                            request_id.clone(),
                            ProtocolErrorCode::InternalError,
                            format!("failed to decode stored event: {error}"),
                        )
                    })?;
                // Stored payloads carry meta.seq = null (the outbox does not
                // know the log seq at write time); hydrate it from the row.
                envelope.meta.seq = Some(row.seq);
                result.replay.push(envelope);
            }
            result.cursors.push(EventCursor {
                stream_id,
                seq: barrier,
            });
        }
        Ok(result)
    }

    fn cursor_expired_response(
        &self,
        request_id: serde_json::Value,
        message: String,
    ) -> serde_json::Value {
        let mut agent_error = AgentError::new(codes::CURSOR_EXPIRED, message.clone());
        agent_error.requires_snapshot = true;
        serde_json::to_value(ErrorResponse {
            id: request_id,
            error: ProtocolError {
                code: ProtocolErrorCode::CursorExpired,
                message,
                data: serde_json::to_value(agent_error).expect("serialize agent error"),
            },
        })
        .expect("serialize cursor-expired response")
    }

    async fn build_snapshot(
        &self,
        selector: &StreamSelector,
        barrier_seq: u64,
    ) -> Result<Option<StreamSnapshot>, String> {
        let stream_id = selector_stream_id(selector);
        match selector {
            StreamSelector::Session { session_id } => {
                let Some(rollout_path) = self.snapshot_rollout_path(session_id).await else {
                    return Ok(None);
                };
                let history = devo_core::read_canonical_history(&rollout_path)
                    .map_err(|error| format!("failed to read session history: {error}"))?;
                let Some(mut session) = history.session else {
                    return Ok(None);
                };
                // Durable history may still say InProgress after a crash; live
                // ownership is ActiveTurnRegistry only (same rule as session/read).
                let active_turn = self
                    .active_turns
                    .active_turn(*session_id)
                    .await
                    .map(Box::new);
                match active_turn.as_ref() {
                    Some(turn) => {
                        session.status = SessionStatus::Active;
                        session.active_turn_id = Some(turn.id);
                    }
                    None => {
                        session.status = SessionStatus::Idle;
                        session.active_turn_id = None;
                    }
                }
                session.sync_activity();
                let queue = self
                    .snapshot_queue_entries(session_id)
                    .await
                    .map_err(|error| format!("failed to read session queue: {error}"))?;
                Ok(Some(StreamSnapshot {
                    stream_id,
                    barrier_seq,
                    data: SnapshotData::Session {
                        session,
                        active_turn,
                        queue,
                    },
                }))
            }
            StreamSelector::SessionsByCwd { cwd } => {
                let sessions = self
                    .native_sessions_for_cwd(cwd)
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(Some(StreamSnapshot {
                    stream_id,
                    barrier_seq,
                    data: SnapshotData::SessionsList { sessions },
                }))
            }
            // v1: no standalone task snapshot source; the task item is
            // visible through its owning session's history.
            StreamSelector::BackgroundTask { .. } => Ok(None),
        }
    }

    /// Native sessions under one cwd: the rollout history reader is the
    /// source of truth (the SQLite index is a cache that may lag or lack
    /// rows for never-indexed files).
    async fn native_sessions_for_cwd(
        &self,
        cwd: &std::path::Path,
    ) -> anyhow::Result<Vec<devo_protocol::native::session::Session>> {
        let mut sessions = Vec::new();
        for rollout_path in self.rollout_store.rollout_paths()? {
            let Ok(history) = devo_core::read_canonical_history(&rollout_path) else {
                // Damaged files contribute nothing to the list snapshot;
                // resume's fail-closed policy reports them separately.
                continue;
            };
            if let Some(session) = history.session
                && session.cwd == cwd
            {
                sessions.push(*session);
            }
        }
        Ok(sessions)
    }

    /// Mailbox-free rollout-path resolution for the subscription critical
    /// section. `handle_subscription_create` holds the `connections` lock
    /// across `build_snapshot`, and the session actor may be parked
    /// broadcasting into that same lock (`broadcast_notification` takes it on every
    /// turn event), so waiting on the actor mailbox here — as
    /// `resolve_rollout_path` does — is an ABBA deadlock. The in-flight
    /// turn's inline record is read under `try_lock` only: on contention we
    /// fall through to the persisted sources (SQLite index, then a rollout
    /// store scan) rather than risk a lock-ordering inversion with
    /// turn-event persistence.
    async fn snapshot_rollout_path(
        &self,
        session_id: &NativeSessionId,
    ) -> Option<std::path::PathBuf> {
        let session_id = *session_id;
        if let Some(stream) = self.active_stream_state(session_id).await
            && let Ok(stream) = stream.try_lock()
            && let Some(inline) = stream.turn_inline.as_ref()
            && let Some(path) = inline.rollout_path.clone()
        {
            return Some(path);
        }
        if let Ok(Some(index)) = self.deps.db.get_session_index(&session_id)
            && let Some(path) = index.rollout_path
        {
            return Some(path);
        }
        self.rollout_store
            .find_rollout_by_session_id(&session_id)
            .ok()
            .flatten()
    }

    /// Queue source for session snapshots: the session's in-memory turn
    /// queue while a turn is active (same source and 1-based positions as
    /// `session/queue/list` and `queue/updated`, and the only source for
    /// ephemeral sessions that skip DB writes). Falls back to the persisted
    /// SQLite queue otherwise (idle or never-resumed sessions; the DB mirror
    /// is written through on every mutation for durable sessions).
    ///
    /// Locking: `build_snapshot` runs inside the `connections` critical
    /// section, so this must never wait on the session-actor mailbox — the
    /// actor may be parked broadcasting into that same lock. The spawn
    /// snapshot comes from the runtime registry only (mailbox-free), and the
    /// queue std::Mutex is held only for the entry build.
    async fn snapshot_queue_entries(
        &self,
        session_id: &NativeSessionId,
    ) -> anyhow::Result<Vec<QueueEntry>> {
        let session_id = *session_id;
        if let Some(spawn) = self.active_spawn_snapshot_for_session(session_id).await {
            let queue = spawn
                .pending_turn_queue
                .lock()
                .expect("pending turn queue mutex should not be poisoned");
            return Ok(native_queue_entries(&queue));
        }
        self.queue_entries(&session_id)
    }

    fn queue_entries(&self, session_id: &NativeSessionId) -> anyhow::Result<Vec<QueueEntry>> {
        let pending = self.deps.db.list_pending(session_id, QueueType::Turn)?;
        Ok(pending
            .into_iter()
            .enumerate()
            .map(|(index, item)| {
                let (input, preview) = queue_entry_content(&item);
                QueueEntry {
                    queue_item_id: item.id,
                    // 1-based, matching `native_queue_entries` (in-memory
                    // path) and `session/queue/list`.
                    position: (index + 1) as u32,
                    input,
                    preview,
                    enqueued_at: item.created_at,
                }
            })
            .collect())
    }

    /// Pending approvals/structured questions of the subscribed sessions
    /// (08 §4: reconnecting clients must be able to answer them).
    pub(crate) async fn pending_control_requests(
        &self,
        selectors: &[StreamSelector],
    ) -> Vec<PendingControlRequest> {
        let mut out = Vec::new();
        for selector in selectors {
            let StreamSelector::Session { session_id } = selector else {
                continue;
            };
            let snapshot = self.session_interactive.pending_snapshot(*session_id).await;
            for approval in snapshot.approvals {
                let (native_session_id, native_turn_id) = self
                    .native_session_turn_ids(approval.owner_session_id, approval.turn_id)
                    .await;
                let command = approval.command.clone();
                let kind = if command.is_some() {
                    ControlRequestKind::ApprovalCommand
                } else if matches!(
                    approval.resource,
                    Some(devo_safety::ResourceKind::FileWrite)
                ) {
                    ControlRequestKind::ApprovalFileChange
                } else {
                    ControlRequestKind::ApprovalPermission
                };
                let target = if let Some(path) = &approval.path {
                    Some(ApprovalTarget::Path { path: path.clone() })
                } else if let Some(host) = &approval.host {
                    Some(ApprovalTarget::Host { host: host.clone() })
                } else {
                    command
                        .clone()
                        .map(|command| ApprovalTarget::Command { command })
                };
                out.push(PendingControlRequest {
                    request_id: approval.approval_id.clone(),
                    kind,
                    item: waiting_item_envelope(
                        &native_session_id,
                        native_turn_id,
                        approval.persisted.as_ref(),
                        Item::Approval {
                            approval_id: approval.approval_id.clone(),
                            target_item_id: None,
                            action_summary: command.unwrap_or_else(|| approval.tool_name.clone()),
                            justification: String::new(),
                            resource: approval.resource.map(|resource| format!("{resource:?}")),
                            available_scopes: approval.available_scopes.clone(),
                            command_pattern: approval.command_pattern.clone(),
                            command_prefix: approval.command_prefix.clone(),
                            target,
                            decision: None,
                        },
                    ),
                });
            }
            for user_input in snapshot.user_inputs {
                let (native_session_id, native_turn_id) = self
                    .native_session_turn_ids(user_input.owner_session_id, user_input.turn_id)
                    .await;
                out.push(PendingControlRequest {
                    request_id: user_input.request_id.clone(),
                    kind: ControlRequestKind::UserInput,
                    item: waiting_item_envelope(
                        &native_session_id,
                        native_turn_id,
                        user_input.persisted.as_ref(),
                        Item::UserInputRequest {
                            request_id: user_input.request_id.clone(),
                            target_item_id: None,
                            questions: user_input
                                .questions
                                .into_iter()
                                .map(|question| devo_protocol::native::item::UserQuestion {
                                    id: question.id,
                                    header: question.header,
                                    question: question.question,
                                    is_other: question.is_other,
                                    is_secret: question.is_secret,
                                    options: question.options.map(|options| {
                                        options
                                            .into_iter()
                                            .map(|option| {
                                                devo_protocol::native::item::UserQuestionOption {
                                                    label: option.label,
                                                    description: option.description,
                                                }
                                            })
                                            .collect()
                                    }),
                                })
                                .collect(),
                            answers: None,
                        },
                    ),
                });
            }

            // Waiting items are restored into live lanes on hydrate so a
            // reconnecting desktop client can still answer them after restart.
        }
        out
    }

    /// Rebuilds one connection's cached selector union from the registry
    /// (after unsubscribe or connection close of a sibling).
    async fn refresh_connection_selectors(
        &self,
        connections: &mut HashMap<u64, ConnectionRuntime>,
        connection_id: u64,
    ) {
        let selectors: Vec<StreamSelector> = self
            .event_subscriptions
            .lock()
            .await
            .values()
            .filter(|subscription| subscription.connection_id == connection_id)
            .flat_map(|subscription| subscription.selectors.clone())
            .collect();
        if let Some(connection) = connections.get_mut(&connection_id) {
            // A live session-kind subscription now declares this connection's
            // delivery scope explicitly. Prune legacy implicit filters for
            // sessions it no longer covers: the implicit filters pushed by
            // session/resume|new|fork are otherwise never removed, so a client
            // that resumed session A and later switched to B kept receiving
            // (and rendering) A's entire item stream forever. Connections
            // without any live session selector (pure legacy ACP) keep every
            // implicit filter.
            let covered: HashSet<SessionId> = selectors
                .iter()
                .filter_map(|selector| match selector {
                    StreamSelector::Session { session_id } => Some(*session_id),
                    _ => None,
                })
                .collect();
            if !covered.is_empty() {
                connection.subscriptions.retain(|filter| {
                    filter
                        .session_id
                        .as_ref()
                        .is_none_or(|id| covered.contains(id))
                });
            }
            connection.event_selectors = selectors;
        }
    }

    /// Recomputes the cheap SessionsByCwd gate used by the broadcast path.
    async fn refresh_cwd_selector_count(&self) {
        let count = self
            .event_subscriptions
            .lock()
            .await
            .values()
            .flat_map(|subscription| subscription.selectors.iter())
            .filter(|selector| matches!(selector, StreamSelector::SessionsByCwd { .. }))
            .count();
        self.sessions_by_cwd_subscriptions
            .store(count, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Builds the waiting-state envelope for a pending control request. The
fn waiting_item_envelope(
    session_id: &NativeSessionId,
    turn_id: NativeTurnId,
    persisted: Option<&crate::execution::PersistedLivingItem>,
    item: Item,
) -> ItemEnvelope {
    let now = Utc::now();
    ItemEnvelope {
        id: persisted
            .map(|persisted| persisted.item_id)
            .unwrap_or_default(),
        session_id: *session_id,
        turn_id,
        seq: persisted.map_or(0, |persisted| persisted.seq),
        revision: 1,
        created_at: persisted.map_or(now, |persisted| persisted.created_at),
        updated_at: now,
        state: ItemState::Waiting,
        item,
        parent_id: None,
    }
}

/// Extracts display content and a single-line preview from a pending queue
/// entry. Structured `UserInput` entries keep only their display text for
/// now (the full part list is stored but the canonical `UserInput` mapping
/// for skills/mentions/images lands with `session/queue/*` in P4).
fn queue_entry_content(item: &devo_protocol::PendingInputItem) -> (Vec<UserInput>, String) {
    let text = match &item.kind {
        devo_protocol::PendingInputKind::UserText { text } => text.clone(),
        devo_protocol::PendingInputKind::UserInput { display_text, .. } => display_text.clone(),
        // Non-input queue kinds (hook blocks, budget steering) are not
        // user-editable inputs; they surface as an empty entry.
        _ => String::new(),
    };
    let preview = text.lines().next().unwrap_or_default().to_owned();
    if text.is_empty() {
        (Vec::new(), preview)
    } else {
        (vec![UserInput::Text { text }], preview)
    }
}
