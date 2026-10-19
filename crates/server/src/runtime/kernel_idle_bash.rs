//! Restricted idle host callback for detached Bash completion notices and withdrawals.

use std::sync::{Arc, Weak};

use futures::future::BoxFuture;
use serde_json::{Value, json};

use devo_protocol::native::ids::SessionId;

use super::ServerRuntime;

/// Handles only `bash.completed` and PID-scoped `bash.consumed`; all other idle requests are denied.
pub(crate) fn for_session(
    runtime: Weak<ServerRuntime>,
    session_id: SessionId,
) -> devo_kernel::HostRequestHandler {
    let session_key = session_id.to_string();
    Arc::new(move |_request_id, data| {
        let runtime = runtime.clone();
        let session_id = session_id;
        let session_key = session_key.clone();
        Box::pin(async move {
            let (action, params) = devo_kernel::parse_host_request_data(&data);
            match action.as_str() {
                "bash.completed" => {
                    super::kernel_host::push_bash_notice(
                        &session_key,
                        json!({
                            "kind": "async_bash_completion",
                            "params": params,
                        }),
                    );
                    if let Some(runtime) = runtime.upgrade() {
                        runtime
                            .drain_async_tool_completion_notices(session_id)
                            .await;
                    }
                }
                "bash.consumed" => {
                    let Some(pid) = params.get("pid").and_then(Value::as_i64) else {
                        return json!({
                            "status": "error",
                            "error": "bash.consumed requires a pid",
                        });
                    };
                    super::kernel_host::consume_bash_notice(&session_key, pid);
                }
                _ => {
                    return json!({
                        "status": "error",
                        "error": format!("idle host request denied: {action}"),
                    });
                }
            }
            json!({"status": "ok", "result": {"accepted": true}})
        }) as BoxFuture<'static, Value>
    })
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;
    use crate::runtime::kernel_host::take_bash_notices;

    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use anyhow::{Context, Result};
    use async_trait::async_trait;
    use devo_protocol::{ModelRequest, ModelResponse, StreamEvent};
    use devo_provider::ModelProviderSDK;
    use tempfile::TempDir;
    use uuid::Uuid;

    use crate::ClientTransportKind;
    use crate::SuccessResponse;
    use crate::runtime::outbound::test_outbound_channel;
    use crate::test_support::TestRuntime;

    struct GatedProvider {
        open: Arc<AtomicBool>,
        started: Arc<AtomicBool>,
    }

    #[async_trait]
    impl ModelProviderSDK for GatedProvider {
        fn name(&self) -> &str {
            "gated"
        }

        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            anyhow::bail!("gated provider does not support completion")
        }

        async fn completion_stream(
            &self,
            _request: ModelRequest,
        ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>>
        {
            self.started.store(true, Ordering::SeqCst);
            let open = Arc::clone(&self.open);
            Ok(Box::pin(futures::stream::unfold(false, move |done| {
                let open = Arc::clone(&open);
                async move {
                    if done {
                        return None;
                    }
                    while !open.load(Ordering::SeqCst) {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Some((
                        Ok(StreamEvent::MessageDone {
                            response: ModelResponse {
                                id: "idle-bash-followup".into(),
                                content: vec![devo_protocol::ResponseContent::Text("done".into())],
                                stop_reason: Some(devo_protocol::StopReason::EndTurn),
                                usage: devo_protocol::Usage::default(),
                                metadata: devo_protocol::ResponseMetadata::default(),
                            },
                        }),
                        true,
                    ))
                }
            })))
        }
    }

    #[tokio::test]
    async fn idle_host_handler_only_enqueues_bash_completion() {
        let session_id = SessionId::new();
        let session_key = session_id.to_string();
        let handler = for_session(Weak::new(), session_id);
        let reply = handler(
            "completion-1".into(),
            json!({
                "type": "bash.completed",
                "params": { "pid": 123, "exitCode": 0 },
            }),
        )
        .await;
        assert_eq!(reply["status"], "ok");
        assert_eq!(reply["result"]["accepted"], true);

        let notice = json!({
            "kind": "async_bash_completion",
            "params": { "pid": 123, "exitCode": 0 },
        });
        assert_eq!(take_bash_notices(&session_key), vec![notice.clone()]);
        super::super::kernel_host::push_bash_notice(&session_key, notice);
        let consumed = handler(
            "consumed-1".into(),
            json!({ "type": "bash.consumed", "params": { "pid": 123 } }),
        )
        .await;
        assert_eq!(consumed["status"], "ok");
        assert!(take_bash_notices(&session_key).is_empty());

        let denied = handler(
            "unsafe-1".into(),
            json!({ "type": "fs.write", "params": {} }),
        )
        .await;
        assert_eq!(denied["status"], "error");
        assert!(take_bash_notices(&session_key).is_empty());
    }

    #[tokio::test]
    async fn completion_notice_is_restored_when_follow_up_turn_cannot_start() {
        let data_root = TempDir::new().expect("tempdir");
        let runtime = TestRuntime::new(Arc::new(GatedProvider {
            open: Arc::new(AtomicBool::new(false)),
            started: Arc::new(AtomicBool::new(false)),
        }))
        .db_file("failed-idle-bash.db")
        .runtime(data_root.path());
        let session_id = SessionId::new();
        let session_key = session_id.to_string();
        let notice = json!({
            "kind": "async_bash_completion",
            "params": { "pid": 902, "exitCode": 1 },
        });
        crate::runtime::kernel_host::push_bash_notice(&session_key, notice.clone());

        runtime
            .drain_async_tool_completion_notices(session_id)
            .await;

        assert_eq!(take_bash_notices(&session_key), vec![notice]);
    }

    #[tokio::test]
    async fn idle_bash_completion_starts_a_follow_up_turn() -> Result<()> {
        let open = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicBool::new(false));
        let data_root = TempDir::new()?;
        let runtime = TestRuntime::new(Arc::new(GatedProvider {
            open: Arc::clone(&open),
            started: Arc::clone(&started),
        }))
        .db_file("idle-bash.db")
        .runtime(data_root.path());
        let (outbound, _receiver) = test_outbound_channel(8);
        let connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, outbound)
            .await;
        runtime
            .handle_acp_initialize(
                connection_id,
                Some(json!(1)),
                json!({
                    "protocolVersion": 1,
                    "clientCapabilities": { "terminal": false },
                    "_meta": { "devo": { "protocol": "native" } },
                }),
            )
            .await;
        let response = runtime
            .handle_incoming(
                connection_id,
                json!({
                    "id": 2,
                    "method": "session/new",
                    "params": {
                        "cwd": data_root.path(),
                        "idempotencyKey": format!("idle-bash-session-{}", Uuid::new_v4()),
                    },
                }),
            )
            .await
            .context("session/new response")?;
        let session: SuccessResponse<devo_protocol::native::rpc_session::SessionNewResult> =
            serde_json::from_value(response)?;
        let session_id = session.result.session.id;
        let session_key = session_id.to_string();

        let handler = for_session(Arc::downgrade(&runtime), session_id);
        let reply = handler(
            "bash-completion-1".into(),
            json!({
                "type": "bash.completed",
                "params": { "pid": 901, "command": "echo test", "exitCode": 0 },
            }),
        )
        .await;
        assert_eq!(reply["status"], "ok");
        tokio::time::timeout(Duration::from_secs(5), async {
            while !started.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .context("completion notice did not wake the idle session")?;
        let reservation = runtime
            .session_turn_reservation_snapshot(session_id)
            .await
            .context("follow-up turn reservation")?;
        let turn_id = reservation
            .active_turn
            .as_ref()
            .expect("idle completion starts a turn")
            .turn_id();
        assert!(
            runtime
                .active_turn_user_texts(session_id, turn_id)
                .await
                .iter()
                .any(|text| text.contains("Inspect the saved handle and continue."))
        );
        assert!(take_bash_notices(&session_key).is_empty());
        open.store(true, Ordering::SeqCst);
        Ok(())
    }
}
