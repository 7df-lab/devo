//! Kernel `host_request` dispatch table (P2).
//!
//! Maps action names from the CPython REPL to host behavior. Mutating actions
//! (`bash` / `write` / `edit` / `mcp.*`) go through [`HostBridge`] (Ask +
//! execute). Schedule actions reuse existing refine/compact façades.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, LazyLock, Mutex};

use serde_json::{Value, json};

use crate::runtime::compact_host::{
    compact_status_from_occupancy, compact_status_json, peek_pending_compact, schedule_compact_run,
};
use crate::runtime::kernel_host_bridge::HostBridge;
use crate::runtime::refine::peek_pending_refine;
use devo_core::tools::ToolAgentScope;
use devo_protocol::native::ids::SessionId;
use devo_protocol::native::item::ContextOccupancy;
use devo_protocol::native::rpc_session::SessionRefineRunParams;

pub use crate::runtime::kernel_host_bridge::host_handler_for_bridge;

/// In-memory bash completion / consumed notices keyed by session id.
static BASH_NOTICES: LazyLock<Mutex<HashMap<String, VecDeque<Value>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Dispatch one host_request with a live [`HostBridge`].
pub async fn dispatch_host_request_with_bridge(bridge: &HostBridge, data: Value) -> Value {
    let action = data
        .get("type")
        .or_else(|| data.get("action"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let params = data.get("params").cloned().unwrap_or_else(|| {
        let mut obj = data.as_object().cloned().unwrap_or_default();
        obj.remove("action");
        obj.remove("type");
        Value::Object(obj)
    });
    let session_id = bridge.session_id.as_str();

    match action.as_str() {
        "" => error_reply("missing host_request type/action"),
        "rlm.create_session" => {
            error_reply("denied: rlm.create_session (daemon out of scope this release)")
        }
        "rlm_heartbeat.list" => {
            let Some(runtime) = bridge.runtime() else {
                return error_reply("runtime not available for rlm_heartbeat.list");
            };
            runtime.host_rlm_heartbeat_list(session_id, &params)
        }
        "rlm_heartbeat.create" => {
            let Some(runtime) = bridge.runtime() else {
                return error_reply("runtime not available for rlm_heartbeat.create");
            };
            runtime.host_rlm_heartbeat_create(session_id, &params).await
        }
        "rlm_heartbeat.update" => {
            let Some(runtime) = bridge.runtime() else {
                return error_reply("runtime not available for rlm_heartbeat.update");
            };
            runtime.host_rlm_heartbeat_update(session_id, &params).await
        }
        "rlm_heartbeat.delete" => {
            let Some(runtime) = bridge.runtime() else {
                return error_reply("runtime not available for rlm_heartbeat.delete");
            };
            runtime.host_rlm_heartbeat_delete(session_id, &params).await
        }
        a if a.starts_with("rlm_heartbeat") => {
            error_reply(format!("unknown rlm_heartbeat action: {a}"))
        }
        "rlm.run" => {
            // Wire action behind kernel `await rlm.spawn(...)`: spawn a real
            // child session through the agent coordinator. Fresh namespace per
            // the child doctrine (no parent history fork), caller-named.
            let prompt = params
                .get("prompt")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");
            if prompt.is_empty() {
                return error_reply("rlm.run requires prompt");
            }
            let kwargs = params.get("kwargs").cloned().unwrap_or_else(|| json!({}));
            let name = kwargs
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string);
            if let Some(model) = kwargs
                .get("model")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|model| !model.is_empty())
            {
                return error_reply(format!(
                    "model override '{model}' is not supported by this host; children inherit \
                     the parent model (omit 'model')"
                ));
            }
            if let Some(thinking) = kwargs
                .get("thinking")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|thinking| !thinking.is_empty())
            {
                return error_reply(format!(
                    "thinking override '{thinking}' is not supported by this host; children \
                     inherit the parent level (omit 'thinking')"
                ));
            }
            let Some(runtime) = bridge.runtime() else {
                return error_reply("runtime not available for rlm.run");
            };
            let Some(sid) = parse_session_id(session_id) else {
                return error_reply("invalid session id");
            };
            use devo_core::tools::AgentToolCoordinator;
            use devo_protocol::AgentToolPolicy;
            use devo_protocol::SpawnAgentParams;
            match runtime
                .spawn_agent(SpawnAgentParams {
                    session_id: sid,
                    message: prompt.to_string(),
                    // RLM children start a fresh namespace ("[task from parent]"
                    // labeling); forking parent history would leak the parent
                    // transcript into the child instead.
                    fork_turns: Some("none".to_string()),
                    max_turns: None,
                    tool_policy: AgentToolPolicy::Inherit,
                    ephemeral: false,
                    nickname: name,
                })
                .await
            {
                Ok(result) => {
                    let session_dir = result.session_dir.clone().unwrap_or_default();
                    if session_dir.is_empty() {
                        return error_reply(
                            "spawned child has no persisted session dir (ephemeral?)",
                        );
                    }
                    ok_result(json!({
                        "rlm_child_id": result.child_session_id.to_string(),
                        "name": result.agent_nickname,
                        "session_dir": session_dir,
                        "model": result.model.unwrap_or_default(),
                    }))
                }
                Err(error) => error_reply(error.to_string()),
            }
        }
        "rlm.find_models" => {
            // Valid payload shape for the kernel parser (`models` list). This
            // host runs children on the inherited parent model, so there is no
            // alternate-model catalog to offer here.
            ok_result(json!({ "models": [] }))
        }
        "rlm.list_subagents" => {
            let Some(runtime) = bridge.runtime() else {
                return error_reply("runtime not available for rlm.list_subagents");
            };
            runtime.host_rlm_list_subagents(session_id).await
        }
        "rlm.delete_subagent" => {
            let Some(runtime) = bridge.runtime() else {
                return error_reply("runtime not available for rlm.delete_subagent");
            };
            runtime.host_rlm_delete_subagent(session_id, &params).await
        }
        "compact.status" => {
            let sid = parse_session_id(session_id);
            let scheduled = sid.as_ref().map(peek_pending_compact).unwrap_or(false);
            // Real occupancy when the runtime + session are reachable; before
            // the first usage (or right after a compaction) `usage_known` is
            // false and the status still reports the model's context window.
            let snapshot = match (bridge.runtime(), sid) {
                (Some(runtime), Some(sid)) => runtime.context_usage_snapshot(sid).await,
                _ => None,
            };
            let (occupancy, usage_known) = snapshot.unwrap_or((ContextOccupancy::empty(0), false));
            let status = compact_status_from_occupancy(Some(&occupancy), scheduled, usage_known);
            ok_result(compact_status_json(&status))
        }
        "compact.run" => {
            let Some(sid) = parse_session_id(session_id) else {
                return error_reply("invalid session id");
            };
            let instructions = params
                .get("instructions")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            // The kernel runtime returns scheduled:false as ok result (not error) so Python
            // skills can read `reason` without raising RuntimeError.
            let result = schedule_compact_run(&sid, instructions, /*turn_active*/ true);
            ok_result(json!({
                "scheduled": result.scheduled,
                "note": result.note,
                "reason": result.reason,
            }))
        }
        "refine.run" => {
            let Some(sid) = parse_session_id(session_id) else {
                return error_reply("invalid session id");
            };
            let is_root = !matches!(bridge.agent_scope, ToolAgentScope::Subagent);
            let rpc_params = SessionRefineRunParams {
                session_id: sid,
                instructions: params
                    .get("instructions")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                global: params
                    .get("global")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                rollback_id: params
                    .get("rollback_id")
                    .or_else(|| params.get("rollbackId"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            };
            let result = crate::runtime::refine::schedule_refine_run(
                &sid,
                &rpc_params,
                is_root,
                bridge.session_dir.as_deref(),
            );
            ok_result(json!({
                "scheduled": result.scheduled,
                "note": result.note,
                "reason": result.reason,
            }))
        }
        "refine.status" => {
            let Some(sid) = parse_session_id(session_id) else {
                return error_reply("invalid session id");
            };
            // Wire snake_case: pending + in_flight
            ok_result(json!({
                "pending": peek_pending_refine(&sid),
                "in_flight": false,
            }))
        }
        "goal.get" => {
            let Some(runtime) = bridge.runtime() else {
                return error_reply("runtime not available for goal.get");
            };
            runtime.host_goal_get(session_id).await
        }
        "goal.create" => {
            let Some(runtime) = bridge.runtime() else {
                return error_reply("runtime not available for goal.create");
            };
            runtime.host_goal_create(session_id, &params).await
        }
        "goal.complete" => {
            let Some(runtime) = bridge.runtime() else {
                return error_reply("runtime not available for goal.complete");
            };
            runtime.host_goal_complete(session_id).await
        }
        "bash" | "bash.start" => {
            crate::runtime::kernel_host_bridge::handle_bash(bridge, &params).await
        }
        "bash.completed" => {
            push_bash_notice(
                session_id,
                json!({
                    "kind": "async_bash_completion",
                    "params": params,
                }),
            );
            ok_result(json!({ "accepted": true }))
        }
        "python.completed" => {
            push_bash_notice(
                session_id,
                json!({
                    "kind": "async_python_completion",
                    "params": params,
                }),
            );
            ok_result(json!({ "accepted": true }))
        }
        "bash.consumed" => {
            let Some(pid) = params.get("pid").and_then(Value::as_i64) else {
                return error_reply("bash.consumed requires a pid");
            };
            consume_bash_notice(session_id, pid);
            ok_result(json!({ "accepted": true }))
        }
        "write" => crate::runtime::kernel_host_bridge::handle_write(bridge, &params).await,
        "edit" => crate::runtime::kernel_host_bridge::handle_edit(bridge, &params).await,
        "fs.read" => crate::runtime::kernel_host_bridge::handle_fs_read(bridge, &params).await,
        // fs.write shares the mediated write path (approval + WriteHandler).
        "fs.write" => crate::runtime::kernel_host_bridge::handle_write(bridge, &params).await,
        "web.search" => {
            crate::runtime::kernel_host_bridge::handle_web_search(bridge, &params).await
        }
        "web.fetch" => crate::runtime::kernel_host_bridge::handle_web_fetch(bridge, &params).await,
        "mcp.call" => crate::runtime::kernel_host_bridge::handle_mcp_call(bridge, &params).await,
        "mcp.list_tools" => {
            crate::runtime::kernel_host_bridge::handle_mcp_list_tools(bridge, &params).await
        }
        "mcp.config" | "mcp.refresh" => ok_result(json!({ "servers": [] })),
        "model.info" => ok_result(bridge.model_info_payload()),
        "plan.update" => {
            crate::runtime::kernel_host_bridge::handle_plan_update(bridge, &params).await
        }
        "question" => crate::runtime::kernel_host_bridge::handle_question(bridge, &params).await,
        "agent_message.send" => {
            let Some(runtime) = bridge.runtime() else {
                // Soft path: still queue a wake notice when runtime is absent (tests).
                push_bash_notice(
                    session_id,
                    json!({
                        "kind": "agent_message",
                        "params": params,
                    }),
                );
                let now = chrono::Utc::now().to_rfc3339();
                return ok_result(json!({
                    "accepted": true,
                    "queued": true,
                    "deliveryStatus": "queued",
                    "queuedAt": now,
                    "note": "queued local agent_message notice (no runtime)"
                }));
            };
            runtime.host_agent_message_send(session_id, &params).await
        }
        "agent_observe.list" => {
            let Some(runtime) = bridge.runtime() else {
                return ok_result(json!({ "agents": [], "current": null }));
            };
            runtime.host_agent_observe_list(session_id).await
        }
        "agent_observe.get" => {
            let Some(runtime) = bridge.runtime() else {
                return error_reply("runtime not available for agent_observe.get");
            };
            runtime.host_agent_observe_get(session_id, &params).await
        }
        "agent_observe.recent" => {
            let Some(runtime) = bridge.runtime() else {
                return error_reply("runtime not available for agent_observe.recent");
            };
            runtime.host_agent_observe_recent(session_id, &params).await
        }
        other => error_reply(format!("unknown host_request action: {other}")),
    }
}

/// Dispatch with a test/always-allow bridge bound to `session_id`.
#[allow(dead_code)] // exercised by unit tests; live path uses `dispatch_host_request_with_bridge`
pub async fn dispatch_host_request(session_id: &str, data: Value) -> Value {
    let bridge = HostBridge::for_tests(session_id);
    dispatch_host_request_with_bridge(&bridge, data).await
}

/// Thin wrapper: always-allow bridge for tests that only need a session id.
#[allow(dead_code)] // kept for tests / lightweight session-id-only call sites
pub fn host_handler_for_session(session_id: String) -> devo_kernel::HostRequestHandler {
    host_handler_for_bridge(Arc::new(HostBridge::for_tests(session_id)))
}

#[allow(dead_code)] // shared with python.completed / post-turn wake consumer
pub(crate) fn push_bash_notice(session_id: &str, notice: Value) {
    BASH_NOTICES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(session_id.to_string())
        .or_default()
        .push_back(notice);
}

pub(crate) fn take_bash_notices(session_id: &str) -> Vec<Value> {
    BASH_NOTICES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(session_id)
        .map(|q| q.into_iter().collect())
        .unwrap_or_default()
}

pub(crate) fn consume_bash_notice(session_id: &str, pid: i64) -> bool {
    let mut notices = BASH_NOTICES.lock().unwrap_or_else(|e| e.into_inner());
    let Some(queue) = notices.get_mut(session_id) else {
        return false;
    };
    let original_len = queue.len();
    queue.retain(|notice| {
        notice.get("kind").and_then(Value::as_str) != Some("async_bash_completion")
            || notice
                .get("params")
                .and_then(|params| params.get("pid"))
                .and_then(Value::as_i64)
                != Some(pid)
    });
    let removed = queue.len() != original_len;
    if queue.is_empty() {
        notices.remove(session_id);
    }
    removed
}

fn parse_session_id(session_id: &str) -> Option<SessionId> {
    session_id.parse().ok()
}

fn ok_result(result: Value) -> Value {
    json!({ "status": "ok", "result": result })
}

/// Normalize handler replies to the wire shape `{status:"ok", result}` / `{status:"error", error}`
/// envelope that `rlm.host_request` (`_parse_host_reply`) requires.
pub(crate) fn ensure_host_reply_envelope(reply: Value) -> Value {
    match reply.get("status").and_then(|v| v.as_str()) {
        Some("error") => reply,
        Some("ok") => {
            let only_status_and_result = reply.as_object().is_some_and(|obj| {
                obj.contains_key("result") && obj.keys().all(|k| k == "status" || k == "result")
            });
            if only_status_and_result {
                return reply;
            }
            let mut inner = reply;
            if let Some(obj) = inner.as_object_mut() {
                obj.remove("status");
            }
            ok_result(inner)
        }
        _ => ok_result(reply),
    }
}

fn error_reply(msg: impl Into<String>) -> Value {
    json!({ "status": "error", "error": msg.into() })
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use uuid::Uuid;

    use super::*;
    use crate::runtime::kernel_host_bridge::HostBridge;
    use devo_core::tools::PermissionChecker;
    use devo_protocol::CollaborationMode;

    /// Trace: L2-DES-RLM-001
    /// Verifies: rlm.create_session is denied.
    #[tokio::test]
    async fn denies_create_session() {
        let reply = dispatch_host_request(
            "ses_00000000-0000-0000-0000-000000000001",
            json!({ "type": "rlm.create_session" }),
        )
        .await;
        assert_eq!(reply["status"], "error");
        assert!(
            reply["error"].as_str().unwrap_or("").contains("denied"),
            "{reply}"
        );
    }

    #[tokio::test]
    async fn plan_update_requires_an_active_turn() {
        let bridge = HostBridge::for_tests("ses_00000000-0000-0000-0000-000000000001");
        let reply = dispatch_host_request_with_bridge(
            &bridge,
            json!({ "type": "plan.update", "plan": [] }),
        )
        .await;
        assert_eq!(reply["status"], "error");
        assert!(
            reply["error"]
                .as_str()
                .unwrap_or("")
                .contains("active turn")
        );
    }

    /// Trace: L2-DES-CONTEXT-002, L2-DES-RLM-001
    /// Verifies: compact.status returns shape with percent field (null ok).
    #[tokio::test]
    async fn compact_status_shape_includes_percent() {
        let reply = dispatch_host_request(
            "ses_00000000-0000-0000-0000-000000000001",
            json!({ "type": "compact.status" }),
        )
        .await;
        assert_eq!(reply["status"], "ok");
        let result = &reply["result"];
        assert!(result.get("scheduled").is_some(), "{reply}");
        assert!(
            result.get("percent").is_some(),
            "percent key required (null ok): {reply}"
        );
        assert!(result["percent"].is_null(), "{reply}");
    }

    /// Trace: L2-DES-CONTEXT-002, L1-REQ-RLM-001
    /// Verifies: compact.status reads real occupancy through the runtime — a
    /// fresh session (no measured usage yet) still reports the model's
    /// context window. The pre-fix handler hardcoded `None` occupancy +
    /// `usage_known: false` and returned all-nulls forever.
    #[tokio::test]
    async fn compact_status_reports_context_window_through_runtime() {
        use crate::test_support::NoopProvider;
        use crate::test_support::TestRuntime;
        use devo_core::PresetModelCatalog;
        use devo_core::tools::FileReadLedger;
        use devo_protocol::InputModality;

        let data_root = tempfile::TempDir::new().expect("temp dir");
        let cwd = data_root.path().join("workspace");
        std::fs::create_dir_all(&cwd).expect("workspace dir");
        let runtime = TestRuntime::new(Arc::new(
            NoopProvider::text().named("noop-compact-status-provider"),
        ))
        .catalog(Arc::new(
            PresetModelCatalog::load().expect("preset model catalog"),
        ))
        .db_file("compact_status_wiring.db")
        .runtime(data_root.path());

        let (notifications_tx, _notifications_rx) = crate::test_outbound_channel(128);
        let connection_id = runtime
            .register_connection(crate::ClientTransportKind::Stdio, notifications_tx)
            .await;
        let initialize = runtime
            .handle_incoming(
                connection_id,
                json!({
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": 1,
                        "clientCapabilities": {},
                        "_meta": { "devo": { "protocol": "native" } },
                        "clientInfo": {
                            "name": "compact-status-wiring",
                            "title": "Compact Status Wiring",
                            "version": "1.0.0"
                        }
                    }
                }),
            )
            .await
            .expect("initialize response");
        assert_eq!(initialize["result"]["agentInfo"]["name"], "devo-server");
        let response = runtime
            .handle_incoming(
                connection_id,
                json!({
                    "id": 2,
                    "method": "session/new",
                    "params": { "cwd": cwd, "idempotencyKey": "compact-status-wiring" }
                }),
            )
            .await
            .expect("session/new response");
        let session_id: SessionId = response["result"]["session"]["id"]
            .as_str()
            .expect("session id")
            .parse()
            .expect("session id parses");

        let bridge = HostBridge {
            session_id,
            turn_id: None,
            cwd,
            session_dir: None,
            collaboration_mode: CollaborationMode::Build,
            permission: PermissionChecker::always_allow(),
            client_filesystem: None,
            file_read_ledger: Arc::new(FileReadLedger::new()),
            sandbox_profile: None,
            cancel_token: tokio_util::sync::CancellationToken::new(),
            mcp_manager: None,
            agent_scope: ToolAgentScope::Parent,
            local_web_search: None,
            network_proxy: None,
            network_no_proxy: None,
            runtime: Arc::downgrade(&runtime),
            model_id: None,
            input_modalities: vec![InputModality::Text],
            context_window: None,
        };
        let reply =
            dispatch_host_request_with_bridge(&bridge, json!({ "type": "compact.status" })).await;
        assert_eq!(reply["status"], "ok");
        let result = &reply["result"];
        let window = result["context_window"]
            .as_u64()
            .expect("context_window must be populated from the model catalog");
        assert!(window > 0, "{reply}");
        assert!(
            result["tokens"].is_null(),
            "no measured usage yet, tokens stay null: {reply}"
        );
        assert!(result["percent"].is_null(), "{reply}");
        assert_eq!(result["scheduled"], false);
    }

    /// Trace: L2-DES-RLM-001
    /// Verifies: refine.status / refine.run use `{status, result}` envelope.
    #[tokio::test]
    async fn refine_host_reply_uses_result_envelope() {
        let sid = "ses_00000000-0000-0000-0000-000000000099";
        let status = dispatch_host_request(sid, json!({ "type": "refine.status" })).await;
        assert_eq!(status["status"], "ok");
        assert_eq!(status["result"]["pending"], false);
        assert_eq!(status["result"]["in_flight"], false);

        let run = dispatch_host_request(
            sid,
            json!({
                "type": "refine.run",
                "params": { "instructions": "remember FLAG_XYZ" }
            }),
        )
        .await;
        assert_eq!(run["status"], "ok");
        assert_eq!(run["result"]["scheduled"], true);
        assert!(run["result"].get("note").is_some(), "{run}");
    }

    /// Trace: L2-DES-RLM-001
    /// Verifies: rlm.run/list_subagents/delete_subagents reach their real
    /// handlers — no spawn stub. Without a runtime the reply must be an
    /// explicit error (never the old stub's fake pending handle), and prompt
    /// validation rejects empty prompts.
    #[tokio::test]
    async fn rlm_run_reaches_real_handler_or_errors_explicitly() {
        let sid = format!("ses_{}", Uuid::new_v4());
        let reply = dispatch_host_request(
            &sid,
            json!({
                "type": "rlm.run",
                "params": { "prompt": "do work", "kwargs": { "name": "child-a" } }
            }),
        )
        .await;
        assert_eq!(reply["status"], "error");
        assert!(
            reply["error"]
                .as_str()
                .is_some_and(|error| error.contains("runtime not available")),
            "expected explicit runtime error, got {reply}"
        );

        let empty_prompt = dispatch_host_request(
            &sid,
            json!({
                "type": "rlm.run",
                "params": { "prompt": "   ", "kwargs": { "name": "child-a" } }
            }),
        )
        .await;
        assert_eq!(empty_prompt["status"], "error");
        assert!(
            empty_prompt["error"]
                .as_str()
                .is_some_and(|error| error.contains("requires prompt"))
        );

        let model_override = dispatch_host_request(
            &sid,
            json!({
                "type": "rlm.run",
                "params": { "prompt": "do work", "kwargs": { "name": "child-a", "model": "x/y" } }
            }),
        )
        .await;
        assert_eq!(model_override["status"], "error");
        assert!(
            model_override["error"]
                .as_str()
                .is_some_and(|error| error.contains("not supported"))
        );

        let listing = dispatch_host_request(&sid, json!({ "type": "rlm.list_subagents" })).await;
        assert_eq!(listing["status"], "error");

        // find_models must return the `models` list shape the kernel parses.
        let models = dispatch_host_request(&sid, json!({ "type": "rlm.find_models" })).await;
        assert_eq!(models["status"], "ok");
        assert!(models["result"]["models"].as_array().is_some());
    }

    /// Trace: L2-DES-RLM-001
    /// Verifies: bash.completed enqueues an in-memory notice.
    #[tokio::test]
    async fn bash_completed_enqueues_notice() {
        let sid = format!("ses_{}", Uuid::new_v4());
        let reply = dispatch_host_request(
            &sid,
            json!({
                "type": "bash.completed",
                "params": { "pid": 123, "exitCode": 0 }
            }),
        )
        .await;
        assert_eq!(reply["status"], "ok");
        assert_eq!(reply["result"]["accepted"], true);
        let notices = take_bash_notices(&sid);
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0]["kind"], "async_bash_completion");
    }

    /// Verifies: consuming one Bash PID leaves other Bash/Python completion notices intact.
    #[tokio::test]
    async fn bash_consumed_removes_only_the_matching_notice() {
        let sid = format!("ses_{}", Uuid::new_v4());
        for pid in [123, 456] {
            let reply = dispatch_host_request(
                &sid,
                json!({
                    "type": "bash.completed",
                    "params": { "pid": pid, "exitCode": 0 }
                }),
            )
            .await;
            assert_eq!(reply["status"], "ok");
        }
        let python_reply = dispatch_host_request(
            &sid,
            json!({
                "type": "python.completed",
                "params": { "pid": 789, "exitCode": 0 }
            }),
        )
        .await;
        assert_eq!(python_reply["status"], "ok");

        let missing_pid = dispatch_host_request(&sid, json!({ "type": "bash.consumed" })).await;
        assert_eq!(missing_pid["status"], "error");

        let consumed = dispatch_host_request(
            &sid,
            json!({ "type": "bash.consumed", "params": { "pid": 123 } }),
        )
        .await;
        assert_eq!(consumed["status"], "ok");
        assert_eq!(
            take_bash_notices(&sid),
            vec![
                json!({
                    "kind": "async_bash_completion",
                    "params": { "pid": 456, "exitCode": 0 }
                }),
                json!({
                    "kind": "async_python_completion",
                    "params": { "pid": 789, "exitCode": 0 }
                }),
            ]
        );
    }

    /// Trace: L2-DES-RLM-001
    /// Verifies: agent_message.send queues a wake notice for family projection.
    #[tokio::test]
    async fn agent_message_send_enqueues_notice() {
        let sid = format!("ses_{}", Uuid::new_v4());
        let reply = dispatch_host_request(
            &sid,
            json!({
                "type": "agent_message.send",
                "params": { "text": "hi from child" }
            }),
        )
        .await;
        assert_eq!(reply["status"], "ok");
        assert_eq!(reply["result"]["queued"], true);
        let notices = take_bash_notices(&sid);
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0]["kind"], "agent_message");
    }

    /// Trace: L2-DES-RLM-001, L2-DES-SAFETY-001
    /// Verifies: web.search fails closed when local provider is not configured.
    #[tokio::test]
    async fn web_search_requires_local_config() {
        let bridge = HostBridge::for_tests(format!("ses_{}", Uuid::new_v4()));
        let reply = dispatch_host_request_with_bridge(
            &bridge,
            json!({
                "type": "web.search",
                "params": { "query": "devo rlm" }
            }),
        )
        .await;
        assert_eq!(reply["status"], "error");
        assert!(
            reply["error"]
                .as_str()
                .unwrap_or("")
                .contains("not configured"),
            "{reply}"
        );
    }

    /// Trace: L2-DES-RLM-001, L2-DES-SAFETY-001
    /// Verifies: Plan mode denies bash even with always_allow permission.
    #[tokio::test]
    async fn bash_denied_in_plan_mode_with_always_allow() {
        let mut bridge = HostBridge::for_tests(format!("ses_{}", Uuid::new_v4()));
        bridge.collaboration_mode = CollaborationMode::Plan;
        bridge.permission = PermissionChecker::always_allow();
        let reply = dispatch_host_request_with_bridge(
            &bridge,
            json!({
                "type": "bash",
                "params": { "command": "echo should-not-run" }
            }),
        )
        .await;
        assert_eq!(reply["status"], "error");
        assert!(
            reply["error"].as_str().unwrap_or("").contains("Plan"),
            "{reply}"
        );
    }
}
