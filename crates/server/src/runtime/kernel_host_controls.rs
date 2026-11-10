//! Non-steering orchestration. Waits run outside actors and never own admission locks.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use devo_protocol::native::ids::SessionId;
use devo_protocol::native::item::Item;
use serde_json::{Value, json};
use tokio::time::Instant;

use super::ServerRuntime;
use super::kernel_host_bridge::HostBridge;
use crate::subagent::SubagentStatus;

pub(super) async fn collect(bridge: &HostBridge, params: &Value) -> Value {
    let result = async {
        let timeout_ms = match params.get("timeout_ms") {
            None => 0,
            Some(value) => value.as_u64().filter(|ms| *ms <= 300_000)
                .ok_or("timeout_ms must be an integer between 0 and 300000")?,
        };
        let targets = match params.get("targets") {
            None => Vec::new(),
            Some(Value::Array(values)) => values.iter().map(|value| {
                value.as_str().filter(|target| !target.trim().is_empty())
                    .ok_or("targets must contain nonempty strings")
            }).collect::<Result<Vec<_>, _>>()?,
            Some(_) => return Err("targets must be an array".to_string()),
        };
        let runtime = bridge.runtime().ok_or("runtime not available for rlm.collect")?;
        // Rehydrate only durable direct children, not arbitrary user sessions. The
        // registry is process-local; the index and journal survive host restarts.
        runtime.restore_direct_children(bridge.session_id).await?;
        let ids = if targets.is_empty() {
            runtime.agent_registries.lock().await.get(&bridge.session_id)
                .map(|registry| registry.children_of(&bridge.session_id)).unwrap_or_default()
        } else {
            let mut seen = HashSet::new();
            let mut ids = Vec::new();
            for target in targets {
                let child = runtime.resolve_child_agent(bridge.session_id, target).await
                    .map_err(|error| error.to_string())?;
                if child.parent_session_id != bridge.session_id {
                    return Err("collect can only read direct children".to_string());
                }
                if seen.insert(child.session_id) { ids.push(child.session_id); }
            }
            ids
        };
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            // Register before observing state so a concurrent merge cannot be lost.
            let changed = runtime.rlm_child_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if bridge.cancel_token.is_cancelled() {
                return Err("rlm.collect interrupted".to_string());
            }
            let mut results = Vec::with_capacity(ids.len());
            for id in &ids { results.push(runtime.collect_child(bridge.session_id, *id).await?); }
            if results.iter().all(|result| result["settled"] == true || result["requires_resume"] == true) || Instant::now() >= deadline {
                return Ok(json!({"results": results}));
            }
            tokio::select! {
                _ = changed => {},
                _ = tokio::time::sleep_until(deadline) => {},
                _ = bridge.cancel_token.cancelled() => return Err("rlm.collect interrupted".to_string()),
            }
        }
    }.await;
    match result {
        Ok(result) => json!({"status": "ok", "result": result}),
        Err(error) => json!({"status": "error", "error": error}),
    }
}

impl ServerRuntime {
    pub(super) async fn restore_direct_children(
        self: &Arc<Self>,
        parent: SessionId,
    ) -> Result<(), String> {
        let rows = self
            .deps
            .db
            .list_sessions()
            .map_err(|error| error.to_string())?;
        for row in rows
            .into_iter()
            .filter(|row| row.parent_session_id == Some(parent) && row.agent_path.is_some())
        {
            if self.session(row.session_id).await.is_none() {
                self.get_or_load_parent_session(row.session_id)
                    .await
                    .map_err(|error| format!("restore child {}: {error}", row.session_id))?;
            }
        }
        Ok(())
    }

    async fn collect_child(&self, parent: SessionId, child: SessionId) -> Result<Value, String> {
        let meta = self
            .resolve_child_agent(parent, child.as_str())
            .await
            .map_err(|error| error.to_string())?;
        let handle = self
            .session(child)
            .await
            .ok_or("child session is unavailable")?;
        let (snapshot, queued, active) = {
            // Queue drain pops an input before registering its next turn. A
            // short admission-consistent snapshot must not mistake that gap
            // for completion. Release the gate before transcript/recovery I/O
            // or any wait for child progress.
            let _admission = handle.lock_state_change().await;
            let snapshot = handle
                .turn_reservation_snapshot()
                .await
                .ok_or("child snapshot is unavailable")?;
            let queued = !snapshot
                .pending_turn_queue
                .lock()
                .map_err(|_| "child queue is unavailable")?
                .is_empty()
                || self.has_pending_subagent_work(child);
            let active = self.active_turns.active_turn_id(child).await.is_some();
            (snapshot, queued, active)
        };
        // Terminal registry status is published before MergeTurn. Keep it running
        // until the working set is installed and the execution reservation clears.
        // Recovery is a journal-owned state, not just an unfinished status.
        // Provider failures and shutdown interruptions can also be resumable.
        // Queued follow-ups must not mask an abandoned current turn.
        let requires_resume = if !meta.close_requested && !active && snapshot.active_turn.is_none()
        {
            self.turn_recovery(child)
                .await
                .map_err(|error| format!("read child recovery: {error}"))?
                .is_some()
                || (queued && self.rlm_lazy_children.lock().await.contains(&child))
                || snapshot.latest_turn.as_ref().is_some_and(|turn| {
                    turn.native.status == devo_protocol::native::turn::TurnStatus::WaitingApproval
                })
        } else {
            false
        };
        let settled = !matches!(
            meta.status,
            SubagentStatus::Spawning | SubagentStatus::Running
        ) && snapshot.active_turn.is_none()
            && !queued
            && !active
            && !requires_resume;
        let answer = if settled {
            let latest_id = snapshot.latest_turn.as_ref().map(|turn| turn.turn_id());
            handle
                .transcript_items()
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|item| Some(item.turn_id) == latest_id)
                .filter_map(|item| match item.item {
                    Item::AssistantMessage { text, .. } => Some(text),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            String::new()
        };
        // Explicit host bound; callers can read the retained journal for larger output.
        let truncated = answer.chars().count() > 128 * 1024;
        let answer: String = answer.chars().take(128 * 1024).collect();
        let error = if (settled || requires_resume) && meta.status == SubagentStatus::Failed {
            if let Some(turn) = snapshot.latest_turn.as_ref() {
                // Native turn/read includes the canonical persisted terminal
                // error; the actor's latest-turn mirror can omit its details.
                let read = self
                    .handle_native_turn_read(
                        json!("rlm.collect"),
                        json!({
                            "sessionId": child, "turnId": turn.turn_id(),
                        }),
                    )
                    .await;
                Some(
                    read["result"]["turn"]["error"]["message"]
                        .as_str()
                        .unwrap_or("child turn failed; inspect its transcript")
                        .to_string(),
                )
            } else {
                Some("child failed before a turn was admitted".to_string())
            }
        } else {
            None
        };
        let note = self
            .rlm_progress_notes
            .lock()
            .await
            .get(&child)
            .map(|(_, note)| note.clone());
        Ok(json!({
            "rlm_child_id": child.to_string(), "session_name": meta.nickname,
            "session_dir": meta.session_dir, "status": if requires_resume { "waiting_for_resume" } else if settled { meta.status.as_str() } else { "running" },
            "settled": settled, "answer_preview": if answer.is_empty() { None } else { Some(answer.chars().take(512).collect::<String>()) },
            "answer_text": if answer.is_empty() { None } else { Some(answer) },
            "answer_truncated": truncated, "error": error, "progress_note": note,
            "requires_resume": requires_resume,
        }))
    }
}

pub(super) async fn progress_note(bridge: &HostBridge, params: &Value) -> Value {
    let result = async {
        let note = params
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|note| !note.is_empty() && note.encode_utf16().count() <= 512)
            .ok_or("message must contain 1 to 512 UTF-16 code units")?;
        let runtime = bridge
            .runtime()
            .ok_or("runtime not available for rlm.progress.note")?;
        if runtime
            .child_parent_and_path(bridge.session_id)
            .await
            .is_none()
        {
            return Err("progress_note requires a child session");
        }
        let mut notes = runtime.rlm_progress_notes.lock().await;
        if let Some((at, _)) = notes.get(&bridge.session_id) {
            let remaining = Duration::from_secs(/*secs*/ 10).saturating_sub(at.elapsed());
            if !remaining.is_zero() {
                return Ok(
                    json!({"accepted": false, "retry_after_ms": remaining.as_millis().max(1)}),
                );
            }
        }
        notes.insert(
            bridge.session_id,
            (std::time::Instant::now(), note.to_string()),
        );
        Ok(json!({"accepted": true, "retry_after_ms": null}))
    }
    .await;
    match result {
        Ok(result) => json!({"status": "ok", "result": result}),
        Err(error) => json!({"status": "error", "error": error}),
    }
}

/// The bridge may outlive a turn when a Python task is detached. Never let an
/// obsolete bridge end a later turn or claim success after its turn merged.
pub(super) async fn end_turn(bridge: &HostBridge) -> Value {
    if let (Some(runtime), Some(turn_id), Some(requested)) =
        (bridge.runtime(), bridge.turn_id, &bridge.end_turn_requested)
        && !bridge.cancel_token.is_cancelled()
        && runtime.active_turns.active_turn_id(bridge.session_id).await == Some(turn_id)
    {
        requested.store(true, std::sync::atomic::Ordering::Release);
        json!({"status": "ok", "result": {"ended": true}})
    } else {
        json!({"status": "error", "error": "rlm.end_turn requires an active model turn"})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn malformed_bounds_and_targets_are_rejected_before_runtime_access() {
        let bridge = HostBridge::for_tests(SessionId::new());
        for timeout in [
            json!(-1),
            json!(true),
            json!(1.5),
            json!(300001),
            json!(null),
        ] {
            assert_eq!(
                collect(&bridge, &json!({"timeout_ms":timeout})).await,
                json!({"status":"error","error":"timeout_ms must be an integer between 0 and 300000"})
            );
        }
        for target in [json!([null]), json!([false]), json!([""]), json!(["  "])] {
            assert_eq!(
                collect(&bridge, &json!({"targets":target})).await,
                json!({"status":"error","error":"targets must contain nonempty strings"})
            );
        }
        assert_eq!(
            end_turn(&bridge).await,
            json!({"status":"error","error":"rlm.end_turn requires an active model turn"})
        );
    }

    #[tokio::test]
    async fn progress_limit_counts_utf16_not_unicode_scalars() {
        let bridge = HostBridge::for_tests(SessionId::new());
        for message in [String::new(), " ".to_string(), "🎉".repeat(257)] {
            assert_eq!(
                progress_note(&bridge, &json!({"message":message})).await,
                json!({"status":"error","error":"message must contain 1 to 512 UTF-16 code units"})
            );
        }
        assert_eq!(
            progress_note(&bridge, &json!({"message":"🎉".repeat(256)})).await,
            json!({"status":"error","error":"runtime not available for rlm.progress.note"})
        );
    }
    #[tokio::test]
    async fn saved_unfinished_child_requires_explicit_recovery_without_waiting() {
        use devo_core::tools::AgentToolCoordinator;
        let home = tempfile::TempDir::new().unwrap();
        let runtime = crate::test_support::TestRuntime::noop().runtime(home.path());
        let started = runtime
            .start_session_with_registry(
                /*connection_id*/ 0,
                json!(1),
                devo_protocol::SessionStartParams {
                    cwd: home.path().to_path_buf(),
                    additional_directories: Vec::new(),
                    ephemeral: false,
                    title: None,
                    model: None,
                    model_binding_id: None,
                },
                /*tool_registry*/ None,
            )
            .await;
        let parent: SessionId =
            serde_json::from_value(started["result"]["session"]["id"].clone()).unwrap();
        let child = Arc::clone(&runtime)
            .spawn_agent(
                serde_json::from_value(json!({
                    "session_id":parent,"message":"task","nickname":"worker","fork_turns":"none"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let handle = runtime.session(child.child_session_id).await.unwrap();
        let mut saved = tokio::time::timeout(Duration::from_secs(/*secs*/ 5), async {
            loop {
                let snapshot = handle.turn_reservation_snapshot().await.unwrap();
                if runtime
                    .active_turns
                    .active_turn_id(child.child_session_id)
                    .await
                    .is_none()
                    && let Some(turn) = snapshot.latest_turn
                {
                    break turn;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let mut completed = saved.clone();
        completed.native.status = devo_protocol::native::turn::TurnStatus::Completed;
        handle
            .enqueue_pending_turn_input(devo_protocol::PendingInputItem::new(
                devo_protocol::PendingInputKind::UserText {
                    text: "queued follow-up".to_string(),
                },
                None,
                chrono::Utc::now(),
            ))
            .await;
        // Simulate the durable state exposed after a crash, without starting or
        // silently replaying the saved turn's tool intents. Pending follow-ups
        // must not mask the abandoned current turn.
        saved.native.status = devo_protocol::native::turn::TurnStatus::InProgress;
        handle.set_runtime_session_idle(Some(saved)).await;
        runtime
            .set_agent_status(parent, child.child_session_id, SubagentStatus::Running)
            .await;
        let mut bridge = HostBridge::for_tests(parent);
        bridge.runtime = Arc::downgrade(&runtime);
        let result = tokio::time::timeout(
            Duration::from_secs(/*secs*/ 1),
            collect(&bridge, &json!({"targets":["worker"],"timeout_ms":300000})),
        )
        .await
        .unwrap();
        let item = &result["result"]["results"][0];
        assert_eq!(
            json!({"status":item["status"],"settled":item["settled"],
            "requires_resume":item["requires_resume"],"answer_text":item["answer_text"]}),
            json!({"status":"waiting_for_resume","settled":false,"requires_resume":true,"answer_text":null})
        );
        // A normal live post-merge scheduling gap is not orphaned work.
        handle.set_runtime_session_idle(Some(completed)).await;
        runtime
            .set_agent_status(parent, child.child_session_id, SubagentStatus::Completed)
            .await;
        let live_gap = collect(&bridge, &json!({"targets":["worker"],"timeout_ms":0})).await;
        assert_eq!(
            live_gap["result"]["results"][0]["requires_resume"],
            json!(false)
        );
        // Cold hydration marks the same queued-only state as needing explicit
        // session resume. Starting any real turn clears this runtime marker.
        runtime
            .rlm_lazy_children
            .lock()
            .await
            .insert(child.child_session_id);
        let dormant = collect(&bridge, &json!({"targets":["worker"],"timeout_ms":300000})).await;
        assert_eq!(
            dormant["result"]["results"][0]["requires_resume"],
            json!(true)
        );
    }
}
