//! Native `session/schedule/*` handlers and background wake loop.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;

use crate::ProtocolErrorCode;
use crate::SuccessResponse;
use crate::runtime::ServerRuntime;

impl ServerRuntime {
    pub(crate) async fn handle_native_session_schedule_list(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_schedule::SessionScheduleListParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid session/schedule/list params: {error}"),
                    );
                }
            };
        match self
            .schedule_store
            .list(params.session_id.as_ref(), params.cwd.as_deref())
        {
            Ok(jobs) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::native::rpc_schedule::SessionScheduleListResult { jobs },
            })
            .expect("serialize session/schedule/list"),
            Err(error) => self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to list schedules: {error}"),
            ),
        }
    }

    pub(crate) async fn handle_native_session_schedule_upsert(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_schedule::SessionScheduleUpsertParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid session/schedule/upsert params: {error}"),
                    );
                }
            };
        let dispatch_guard = self.schedule_store.lock_dispatch().await;
        let job = match self.schedule_store.upsert(params) {
            Ok(job) => job,
            Err(error) => {
                let message = error.to_string();
                if message.starts_with("job not found:") || message.starts_with("invalid schedule:")
                {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        message,
                    );
                }
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to upsert schedule: {error}"),
                );
            }
        };
        drop(dispatch_guard);
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::SessionScheduleChanged {
                session_id: Some(job.session_id),
                job: Box::new(job.clone()),
                change: devo_protocol::native::event::SessionScheduleChange::Upserted,
            },
        )
        .await;
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_schedule::SessionScheduleUpsertResult { job },
        })
        .expect("serialize session/schedule/upsert")
    }

    pub(crate) async fn handle_native_session_schedule_update(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_schedule::SessionScheduleUpdateParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid session/schedule/update params: {error}"),
                    );
                }
            };
        let dispatch_guard = self.schedule_store.lock_dispatch().await;
        let job = match self.schedule_store.update(&params.job_id, params.action) {
            Ok(job) => job,
            Err(error) => {
                let message = error.to_string();
                if message.starts_with("job not found:")
                    || message.starts_with("stopped schedule is terminal")
                {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        message,
                    );
                }
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to update schedule: {error}"),
                );
            }
        };
        drop(dispatch_guard);
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::SessionScheduleChanged {
                session_id: Some(job.session_id),
                job: Box::new(job.clone()),
                change: devo_protocol::native::event::SessionScheduleChange::Updated,
            },
        )
        .await;
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_schedule::SessionScheduleUpdateResult { job },
        })
        .expect("serialize session/schedule/update")
    }

    pub(crate) async fn handle_native_session_schedule_delete(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_schedule::SessionScheduleDeleteParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid session/schedule/delete params: {error}"),
                    );
                }
            };
        let dispatch_guard = self.schedule_store.lock_dispatch().await;
        let job = match self.schedule_store.delete(&params.job_id) {
            Ok(job) => job,
            Err(error) => {
                let message = error.to_string();
                if message.starts_with("job not found:") {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        message,
                    );
                }
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to delete schedule: {error}"),
                );
            }
        };
        drop(dispatch_guard);
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::SessionScheduleChanged {
                session_id: Some(job.session_id),
                job: Box::new(job),
                change: devo_protocol::native::event::SessionScheduleChange::Deleted,
            },
        )
        .await;
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_schedule::SessionScheduleDeleteResult {},
        })
        .expect("serialize session/schedule/delete")
    }

    pub(crate) async fn handle_native_session_heartbeat_command(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_schedule::SessionHeartbeatCommandParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid session/heartbeat/command params: {error}"),
                    );
                }
            };
        // Serialize the complete read/modify/write command with due occurrence
        // admission. In particular, pause/clear must not race a wake that has
        // already inspected the job but has not yet queued its input.
        let dispatch_guard = self.schedule_store.lock_dispatch().await;
        let result =
            match crate::heartbeat_command::run_heartbeat_command(&self.schedule_store, params) {
                Ok(result) => result,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        error.to_string(),
                    );
                }
            };
        drop(dispatch_guard);
        if let Some(job) = result.job.as_ref() {
            let change = match result.action {
                devo_protocol::native::rpc_schedule::HeartbeatCommandAction::Clear => {
                    devo_protocol::native::event::SessionScheduleChange::Deleted
                }
                devo_protocol::native::rpc_schedule::HeartbeatCommandAction::Set => {
                    devo_protocol::native::event::SessionScheduleChange::Upserted
                }
                devo_protocol::native::rpc_schedule::HeartbeatCommandAction::Pause
                | devo_protocol::native::rpc_schedule::HeartbeatCommandAction::Resume => {
                    devo_protocol::native::event::SessionScheduleChange::Updated
                }
                devo_protocol::native::rpc_schedule::HeartbeatCommandAction::Status => {
                    // status is read-only; no broadcast
                    return serde_json::to_value(SuccessResponse {
                        id: request_id,
                        result,
                    })
                    .expect("serialize session/heartbeat/command");
                }
            };
            self.broadcast_notification(
                devo_protocol::native::event::ServerNotification::SessionScheduleChanged {
                    session_id: Some(job.session_id),
                    job: Box::new(job.clone()),
                    change,
                },
            )
            .await;
        }
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result,
        })
        .expect("serialize session/heartbeat/command")
    }

    pub(crate) fn start_schedule_wake_loop(self: &Arc<Self>) {
        let runtime = Arc::clone(self);
        let cancel = self.schedule_wake_cancel.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(15));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = ticker.tick() => {
                        if let Err(error) = runtime.dispatch_due_schedule_jobs().await {
                            tracing::warn!(error = %error, "schedule wake loop failed");
                        }
                    }
                }
            }
        });
    }

    pub(crate) async fn dispatch_due_schedule_jobs(self: &Arc<Self>) -> anyhow::Result<()> {
        if self.schedule_wake_cancel.is_cancelled() {
            return Ok(());
        }
        let _dispatch_guard = self.schedule_store.lock_dispatch().await;
        if self.schedule_wake_cancel.is_cancelled() {
            return Ok(());
        }
        let now = Utc::now();
        self.schedule_store.claim_due_with_outbox(now)?;
        for occurrence in self.schedule_store.pending_occurrences(now)? {
            let job = &occurrence.job;
            let occurrence_id = occurrence.occurrence_id.as_str();
            let session_id = job.session_id;
            if !self
                .schedule_store
                .get_job(&job.job_id)?
                .is_some_and(|current| {
                    current.status == devo_protocol::native::rpc_schedule::ScheduleJobStatus::Active
                })
            {
                tracing::debug!(
                    job_id = job.job_id.as_str(),
                    session_id = %session_id,
                    "discarding occurrence for inactive schedule"
                );
                self.schedule_store.acknowledge_occurrence(occurrence_id)?;
                continue;
            }
            let prompt = job
                .prompt
                .clone()
                .or_else(|| job.instruction.clone())
                .unwrap_or_else(|| "Scheduled heartbeat.".to_string());

            // The wake loop is detached from any client, so the session actor
            // may not be resident (LRU-evicted or never loaded in this process).
            if let Err(error) = self.get_or_load_parent_session(session_id).await {
                match error {
                    crate::runtime::session_cache::LoadSessionError::SessionNotFound
                    | crate::runtime::session_cache::LoadSessionError::RolloutMissing => {
                        tracing::warn!(
                            job_id = job.job_id.as_str(),
                            session_id = %session_id,
                            %error,
                            "retiring schedule job for missing session"
                        );
                        if let Err(delete_error) = self.schedule_store.delete(&job.job_id) {
                            tracing::warn!(
                                job_id = job.job_id.as_str(),
                                error = %delete_error,
                                "failed to retire schedule job for missing session"
                            );
                        }
                        if let Err(ack_error) =
                            self.schedule_store.acknowledge_occurrence(occurrence_id)
                        {
                            tracing::warn!(
                                job_id = job.job_id.as_str(),
                                error = %ack_error,
                                "failed to retire missing-session schedule occurrence"
                            );
                        }
                    }
                    _ => {
                        tracing::warn!(
                            job_id = job.job_id.as_str(),
                            session_id = %session_id,
                            %error,
                            "skipping schedule delivery; retrying soon"
                        );
                        self.retry_schedule_occurrence(occurrence_id);
                    }
                }
                continue;
            }

            match self
                .schedule_occurrence_materialized(session_id, occurrence_id)
                .await
            {
                Ok(true) => {
                    if let Err(error) = self.schedule_store.acknowledge_occurrence(occurrence_id) {
                        tracing::warn!(
                            job_id = job.job_id.as_str(),
                            %error,
                            "failed to acknowledge materialized schedule occurrence"
                        );
                    }
                    continue;
                }
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(
                        job_id = job.job_id.as_str(),
                        session_id = %session_id,
                        %error,
                        "failed to read schedule destination history"
                    );
                    if self
                        .schedule_store
                        .occurrence_inflight_since(occurrence_id)
                        .is_none()
                    {
                        self.retry_schedule_occurrence(occurrence_id);
                    }
                    continue;
                }
            }

            let reservation = self.session_turn_reservation_snapshot(session_id).await;
            let Some(reservation) = reservation else {
                self.retry_schedule_occurrence(occurrence_id);
                continue;
            };
            match self
                .schedule_occurrence_pending(session_id, occurrence_id, &reservation)
                .await
            {
                Ok(true) => {
                    let observed_at = Utc::now();
                    self.schedule_store
                        .mark_occurrence_inflight(occurrence_id, observed_at);
                    if let Err(error) = self
                        .schedule_store
                        .mark_occurrence_delivery_attempted(occurrence_id, observed_at)
                    {
                        tracing::warn!(
                            job_id = job.job_id.as_str(),
                            %error,
                            "failed to persist observed schedule delivery"
                        );
                    }
                    continue;
                }
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(
                        job_id = job.job_id.as_str(),
                        session_id = %session_id,
                        %error,
                        "failed to inspect schedule pending inputs"
                    );
                    if self
                        .schedule_store
                        .occurrence_inflight_since(occurrence_id)
                        .is_none()
                    {
                        self.retry_schedule_occurrence(occurrence_id);
                    }
                    continue;
                }
            }

            let active_turn_id = reservation.active_turn.as_ref().map(|turn| turn.turn_id());
            if let Some(admitted_at) = self.schedule_store.occurrence_inflight_since(occurrence_id)
            {
                let retry_after = chrono::Duration::seconds(30);
                if active_turn_id.is_some() || Utc::now() - admitted_at < retry_after {
                    continue;
                }
                // The accepted work is no longer queued or active, and no
                // transcript item was persisted. Re-deliver from the durable
                // occurrence snapshot rather than silently losing it.
                self.schedule_store.clear_occurrence_inflight(occurrence_id);
            }

            let active_user_texts = match active_turn_id {
                Some(turn_id) => self.active_turn_user_texts(session_id, turn_id).await,
                None => Vec::new(),
            };
            let already_active = active_user_texts.iter().any(|text| text == &prompt);
            let matches_prompt = |item: &devo_core::PendingInputItem| match &item.kind {
                devo_core::PendingInputKind::UserText { text } => text == &prompt,
                devo_core::PendingInputKind::UserInput { display_text, .. } => {
                    display_text == &prompt
                }
                _ => false,
            };
            let already_pending = {
                let queued = {
                    let queue = reservation
                        .pending_turn_queue
                        .lock()
                        .expect("pending turn queue mutex should not be poisoned");
                    queue.iter().any(&matches_prompt)
                };
                let steered = {
                    let queue = reservation
                        .steer_input_queue
                        .lock()
                        .expect("steer input queue mutex should not be poisoned");
                    queue.iter().any(&matches_prompt)
                };
                queued || steered
            };
            if already_active || already_pending {
                if occurrence.delivery_attempted_at.is_some() {
                    if self
                        .schedule_store
                        .occurrence_inflight_since(occurrence_id)
                        .is_none()
                    {
                        self.schedule_store
                            .mark_occurrence_inflight(occurrence_id, Utc::now());
                    }
                    continue;
                }
                tracing::debug!(
                    job_id = job.job_id.as_str(),
                    session_id = %session_id,
                    "schedule fire skipped; identical prompt already active or pending"
                );
                if let Err(error) = self.schedule_store.acknowledge_occurrence(occurrence_id) {
                    tracing::warn!(
                        job_id = job.job_id.as_str(),
                        %error,
                        "failed to acknowledge coalesced schedule occurrence"
                    );
                }
                continue;
            }

            let steer_turn_id = if job.delivery_mode
                == devo_protocol::native::rpc_schedule::ScheduleDeliveryMode::Steer
            {
                active_turn_id
            } else {
                None
            };
            let active_prompt = prompt.clone();
            let input = vec![devo_protocol::native::item::UserInput::Text { text: prompt }];
            let queue_params = || {
                serde_json::json!({
                    "sessionId": session_id,
                    "input": input.clone(),
                    "idempotencyKey": occurrence_id,
                    "clientUserMessageId": occurrence_id,
                })
            };
            if !self
                .schedule_store
                .try_reserve_occurrence(occurrence_id, Utc::now())
            {
                continue;
            }
            match self
                .schedule_store
                .mark_occurrence_delivery_attempted(occurrence_id, Utc::now())
            {
                Ok(true) => {}
                Ok(false) => {
                    self.schedule_store.clear_occurrence_inflight(occurrence_id);
                    continue;
                }
                Err(error) => {
                    tracing::warn!(
                        job_id = job.job_id.as_str(),
                        %error,
                        "failed to persist schedule delivery attempt"
                    );
                    self.retry_schedule_occurrence(occurrence_id);
                    continue;
                }
            }
            let response = match job.delivery_mode {
                devo_protocol::native::rpc_schedule::ScheduleDeliveryMode::FollowUp => {
                    self.handle_session_queue_push(
                        /*connection_id*/ 0,
                        serde_json::json!(null),
                        queue_params(),
                    )
                    .await
                }
                devo_protocol::native::rpc_schedule::ScheduleDeliveryMode::Steer => {
                    if let Some(expected_turn_id) = steer_turn_id {
                        let steer_response = self
                            .handle_turn_steer_with_client_user_message_id(
                                /*connection_id*/ 0,
                                serde_json::json!(null),
                                serde_json::json!({
                                    "sessionId": session_id,
                                    "expectedTurnId": expected_turn_id,
                                    "input": input.clone(),
                                    "clientUserMessageId": occurrence_id,
                                    "idempotencyKey": occurrence_id,
                                }),
                                Some(occurrence_id.to_string()),
                            )
                            .await;
                        let outcome = steer_response
                            .get("result")
                            .and_then(|result| result.get("outcome"))
                            .and_then(serde_json::Value::as_str);
                        if outcome == Some("injected") {
                            self.add_active_turn_user_text(
                                session_id,
                                expected_turn_id,
                                active_prompt.clone(),
                            )
                            .await;
                        }
                        let error_code = steer_response
                            .get("error")
                            .and_then(|error| error.get("code"))
                            .and_then(serde_json::Value::as_str);
                        if matches!(
                            error_code,
                            Some("ActiveTurnNotSteerable") | Some("ExpectedTurnMismatch")
                        ) {
                            tracing::debug!(
                                job_id = job.job_id.as_str(),
                                session_id = %session_id,
                                ?error_code,
                                "schedule steer fell back to queue"
                            );
                            self.handle_session_queue_push(
                                /*connection_id*/ 0,
                                serde_json::json!(null),
                                queue_params(),
                            )
                            .await
                        } else {
                            steer_response
                        }
                    } else {
                        self.handle_session_queue_push(
                            /*connection_id*/ 0,
                            serde_json::json!(null),
                            queue_params(),
                        )
                        .await
                    }
                }
            };
            if let Some(error) = response.get("error") {
                tracing::warn!(
                    job_id = job.job_id.as_str(),
                    session_id = %session_id,
                    %error,
                    "schedule delivery failed; retrying soon"
                );
                self.retry_schedule_occurrence(occurrence_id);
            } else {
                tracing::debug!(
                    job_id = job.job_id.as_str(),
                    session_id = %session_id,
                    "schedule occurrence accepted; awaiting transcript materialization"
                );
            }
        }
        Ok(())
    }

    async fn schedule_occurrence_materialized(
        &self,
        session_id: devo_protocol::native::ids::SessionId,
        client_user_message_id: &str,
    ) -> anyhow::Result<bool> {
        let rollout_path = self
            .resolve_rollout_path(&session_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("session has no rollout path"))?;
        let history = devo_core::read_canonical_history(&rollout_path)?;
        Ok(history.items.iter().any(|envelope| {
            matches!(
                &envelope.item,
                devo_protocol::native::item::Item::UserMessage {
                    client_user_message_id: Some(id),
                    ..
                } if id == client_user_message_id
            )
        }))
    }

    async fn schedule_occurrence_pending(
        &self,
        session_id: devo_protocol::native::ids::SessionId,
        client_user_message_id: &str,
        reservation: &crate::runtime::session_actor::snapshots::TurnReservationSnapshot,
    ) -> anyhow::Result<bool> {
        let matches_id = |item: &devo_core::PendingInputItem| {
            item.metadata.as_ref().is_some_and(|metadata| {
                metadata
                    .get("clientUserMessageId")
                    .or_else(|| metadata.get("client_user_message_id"))
                    .and_then(serde_json::Value::as_str)
                    == Some(client_user_message_id)
            })
        };
        let memory_pending = {
            let queue = reservation
                .pending_turn_queue
                .lock()
                .expect("pending turn queue mutex should not be poisoned");
            queue.iter().any(&matches_id)
        } || {
            let queue = reservation
                .steer_input_queue
                .lock()
                .expect("steer input queue mutex should not be poisoned");
            queue.iter().any(&matches_id)
        };
        if memory_pending {
            return Ok(true);
        }
        let durable_turn = self
            .deps
            .db
            .list_pending(&session_id, crate::db::QueueType::Turn)?;
        if durable_turn.iter().any(&matches_id) {
            return Ok(true);
        }
        let durable_steer = self
            .deps
            .db
            .list_pending(&session_id, crate::db::QueueType::Steer)?;
        Ok(durable_steer.iter().any(&matches_id))
    }

    fn retry_schedule_occurrence(&self, occurrence_id: &str) {
        let retry_at = Utc::now() + chrono::Duration::seconds(15);
        if let Err(error) = self
            .schedule_store
            .retry_occurrence(occurrence_id, retry_at)
        {
            self.schedule_store.clear_occurrence_inflight(occurrence_id);
            tracing::warn!(
                occurrence_id,
                error = %error,
                "failed to persist schedule occurrence retry"
            );
        }
    }
}

#[cfg(test)]
#[path = "schedule_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "schedule_heartbeat_tests.rs"]
mod heartbeat_tests;
