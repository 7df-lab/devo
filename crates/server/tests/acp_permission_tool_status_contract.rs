#[path = "support/acp_permission_support.rs"]
mod acp_permission_support;
#[path = "support/acp_runtime_harness.rs"]
mod acp_runtime_harness;

use anyhow::Context;
use anyhow::Result;
use devo_protocol::AcpEmptyResult;
use devo_server::AcpSuccessResponse;
use pretty_assertions::assert_eq;

use acp_permission_support::PermissionCase;
use acp_permission_support::run_permission_case;
use acp_permission_support::start_permission_prompt;

fn permission_cases() -> [PermissionCase; 3] {
    [
        PermissionCase {
            outcome: serde_json::json!({ "outcome": "selected", "optionId": "allow_once" }),
            expect_tool_calls: 1,
            expect_success: true,
            assert_legacy_removed: false,
        },
        PermissionCase {
            outcome: serde_json::json!({ "outcome": "selected", "optionId": "reject_once" }),
            expect_tool_calls: 0,
            expect_success: false,
            assert_legacy_removed: true,
        },
        PermissionCase {
            outcome: serde_json::json!({ "outcome": "cancelled" }),
            expect_tool_calls: 0,
            expect_success: false,
            assert_legacy_removed: false,
        },
    ]
}

#[tokio::test]
async fn acp_permission_flow_uses_request_response_and_tool_status_lifecycle() -> Result<()> {
    run_permission_case(&permission_cases()[0]).await
}

#[tokio::test]
async fn acp_permission_rejection_fails_tool_without_legacy_method() -> Result<()> {
    run_permission_case(&permission_cases()[1]).await
}

#[tokio::test]
async fn acp_permission_cancellation_fails_tool_without_executing() -> Result<()> {
    run_permission_case(&permission_cases()[2]).await
}

#[tokio::test]
async fn acp_session_cancel_returns_empty_result_to_json_rpc_request() -> Result<()> {
    let prompt = start_permission_prompt(3).await?;
    let cancel_response: AcpSuccessResponse<AcpEmptyResult> = serde_json::from_value(
        prompt
            .runtime
            .handle_incoming(
                prompt.connection_id,
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 4,
                    "method": "session/cancel",
                    "params": { "sessionId": prompt.session_id }
                }),
            )
            .await
            .context("session/cancel response")?,
    )?;
    assert_eq!(
        cancel_response,
        AcpSuccessResponse {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(4),
            result: AcpEmptyResult::default(),
        }
    );
    Ok(())
}
