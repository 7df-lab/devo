use std::sync::Arc;
use std::time::Duration;

use devo_core::tools::ClientTerminal;
use devo_core::tools::ClientTerminalCreate;
use devo_core::tools::ClientTerminalCreateRequest;
use devo_core::tools::ClientTerminalEnv;
use devo_core::tools::ClientTerminalExitStatus;
use devo_core::tools::ClientTerminalOutput;
use devo_core::tools::ClientTerminalRequest;
use tokio_util::sync::CancellationToken;

use super::*;

const ACP_TERMINAL_CLIENT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

impl ServerRuntime {
    async fn create_acp_client_terminal_with_cancel(
        &self,
        session_id: SessionId,
        request: ClientTerminalCreateRequest,
        cancel_token: CancellationToken,
    ) -> Result<ClientTerminalCreate, ToolCallError> {
        if let Some(cwd) = request.cwd.as_ref()
            && !cwd.is_absolute()
        {
            return Err(ToolCallError::InvalidInput(
                "terminal/create cwd must be absolute".to_string(),
            ));
        }
        let Some(connection_id) = self
            .active_acp_connection_with_terminal_capability(session_id)
            .await
        else {
            return Ok(ClientTerminalCreate::Unsupported);
        };
        let params = crate::AcpTerminalCreateParams {
            session_id,
            command: request.command,
            args: request.args,
            env: request.env.into_iter().map(acp_terminal_env).collect(),
            cwd: request.cwd,
            output_byte_limit: request.output_byte_limit,
            meta: None,
        };
        let response = self
            .send_request_to_connection_with_timeout(
                connection_id,
                crate::ACP_TERMINAL_CREATE_METHOD,
                serde_json::to_value(params).expect("serialize ACP terminal/create params"),
                ACP_TERMINAL_CLIENT_REQUEST_TIMEOUT,
                cancel_token,
            )
            .await
            .map_err(|error| acp_terminal_request_error("terminal/create", error))?;
        let response = serde_json::from_value::<crate::AcpTerminalCreateResult>(response).map_err(
            |error| {
                ToolCallError::ExecutionFailed(format!("invalid terminal/create response: {error}"))
            },
        )?;
        Ok(ClientTerminalCreate::Created {
            terminal_id: response.terminal_id,
        })
    }

    async fn acp_client_terminal_output_with_cancel(
        &self,
        session_id: SessionId,
        request: ClientTerminalRequest,
        cancel_token: CancellationToken,
    ) -> Result<ClientTerminalOutput, ToolCallError> {
        let Some(connection_id) = self
            .active_acp_connection_with_terminal_capability(session_id)
            .await
        else {
            return Err(ToolCallError::ExecutionFailed(
                "client terminal capability is unavailable".to_string(),
            ));
        };
        let params = crate::AcpTerminalParams {
            session_id,
            terminal_id: request.terminal_id,
            meta: None,
        };
        let response = self
            .send_request_to_connection_with_timeout(
                connection_id,
                crate::ACP_TERMINAL_OUTPUT_METHOD,
                serde_json::to_value(params).expect("serialize ACP terminal/output params"),
                ACP_TERMINAL_CLIENT_REQUEST_TIMEOUT,
                cancel_token,
            )
            .await
            .map_err(|error| acp_terminal_request_error("terminal/output", error))?;
        let response = serde_json::from_value::<crate::AcpTerminalOutputResult>(response).map_err(
            |error| {
                ToolCallError::ExecutionFailed(format!("invalid terminal/output response: {error}"))
            },
        )?;
        Ok(ClientTerminalOutput {
            output: response.output,
            truncated: response.truncated,
            exit_status: response.exit_status.map(client_terminal_exit_status),
        })
    }

    async fn wait_for_acp_client_terminal_exit_with_cancel(
        &self,
        session_id: SessionId,
        request: ClientTerminalRequest,
        timeout: Duration,
        cancel_token: CancellationToken,
    ) -> Result<ClientTerminalExitStatus, ToolCallError> {
        let Some(connection_id) = self
            .active_acp_connection_with_terminal_capability(session_id)
            .await
        else {
            return Err(ToolCallError::ExecutionFailed(
                "client terminal capability is unavailable".to_string(),
            ));
        };
        let params = crate::AcpTerminalParams {
            session_id,
            terminal_id: request.terminal_id,
            meta: None,
        };
        let response = self
            .send_request_to_connection_with_timeout(
                connection_id,
                crate::ACP_TERMINAL_WAIT_FOR_EXIT_METHOD,
                serde_json::to_value(params).expect("serialize ACP terminal/wait_for_exit params"),
                timeout,
                cancel_token,
            )
            .await
            .map_err(|error| acp_terminal_wait_error(timeout, error))?;
        let response = serde_json::from_value::<crate::AcpTerminalWaitForExitResult>(response)
            .map_err(|error| {
                ToolCallError::ExecutionFailed(format!(
                    "invalid terminal/wait_for_exit response: {error}"
                ))
            })?;
        Ok(ClientTerminalExitStatus {
            exit_code: response.exit_code,
            signal: response.signal,
        })
    }

    async fn kill_acp_client_terminal_with_cancel(
        &self,
        session_id: SessionId,
        request: ClientTerminalRequest,
        cancel_token: CancellationToken,
    ) -> Result<(), ToolCallError> {
        self.send_acp_client_terminal_empty_method(
            session_id,
            request,
            crate::ACP_TERMINAL_KILL_METHOD,
            cancel_token,
        )
        .await
    }

    async fn release_acp_client_terminal_with_cancel(
        &self,
        session_id: SessionId,
        request: ClientTerminalRequest,
        cancel_token: CancellationToken,
    ) -> Result<(), ToolCallError> {
        self.send_acp_client_terminal_empty_method(
            session_id,
            request,
            crate::ACP_TERMINAL_RELEASE_METHOD,
            cancel_token,
        )
        .await
    }

    async fn send_acp_client_terminal_empty_method(
        &self,
        session_id: SessionId,
        request: ClientTerminalRequest,
        method: &str,
        cancel_token: CancellationToken,
    ) -> Result<(), ToolCallError> {
        let Some(connection_id) = self
            .active_acp_connection_with_terminal_capability(session_id)
            .await
        else {
            return Err(ToolCallError::ExecutionFailed(
                "client terminal capability is unavailable".to_string(),
            ));
        };
        let params = crate::AcpTerminalParams {
            session_id,
            terminal_id: request.terminal_id,
            meta: None,
        };
        self.send_request_to_connection_with_timeout(
            connection_id,
            method,
            serde_json::to_value(params).expect("serialize ACP terminal params"),
            ACP_TERMINAL_CLIENT_REQUEST_TIMEOUT,
            cancel_token,
        )
        .await
        .map_err(|error| acp_terminal_request_error(method, error))?;
        Ok(())
    }

    async fn active_acp_connection_with_terminal_capability(
        &self,
        session_id: SessionId,
    ) -> Option<u64> {
        let connection_id = *self.active_turn_connections.lock().await.get(&session_id)?;
        let connections = self.connections.lock().await;
        let connection = connections.get(&connection_id)?;
        connection
            .acp_client_capabilities
            .terminal
            .then_some(connection_id)
    }
}

#[async_trait::async_trait]
impl ClientTerminal for ServerRuntime {
    async fn create(
        self: Arc<Self>,
        request: ClientTerminalCreateRequest,
        cancel_token: CancellationToken,
    ) -> Result<ClientTerminalCreate, ToolCallError> {
        let session_id = SessionId::try_from(request.session_id.as_str())
            .map_err(|error| ToolCallError::InvalidInput(error.to_string()))?;
        self.create_acp_client_terminal_with_cancel(session_id, request, cancel_token)
            .await
    }

    async fn output(
        self: Arc<Self>,
        request: ClientTerminalRequest,
        cancel_token: CancellationToken,
    ) -> Result<ClientTerminalOutput, ToolCallError> {
        let session_id = SessionId::try_from(request.session_id.as_str())
            .map_err(|error| ToolCallError::InvalidInput(error.to_string()))?;
        self.acp_client_terminal_output_with_cancel(session_id, request, cancel_token)
            .await
    }

    async fn wait_for_exit(
        self: Arc<Self>,
        request: ClientTerminalRequest,
        timeout: Duration,
        cancel_token: CancellationToken,
    ) -> Result<ClientTerminalExitStatus, ToolCallError> {
        let session_id = SessionId::try_from(request.session_id.as_str())
            .map_err(|error| ToolCallError::InvalidInput(error.to_string()))?;
        self.wait_for_acp_client_terminal_exit_with_cancel(
            session_id,
            request,
            timeout,
            cancel_token,
        )
        .await
    }

    async fn kill(
        self: Arc<Self>,
        request: ClientTerminalRequest,
        cancel_token: CancellationToken,
    ) -> Result<(), ToolCallError> {
        let session_id = SessionId::try_from(request.session_id.as_str())
            .map_err(|error| ToolCallError::InvalidInput(error.to_string()))?;
        self.kill_acp_client_terminal_with_cancel(session_id, request, cancel_token)
            .await
    }

    async fn release(
        self: Arc<Self>,
        request: ClientTerminalRequest,
        cancel_token: CancellationToken,
    ) -> Result<(), ToolCallError> {
        let session_id = SessionId::try_from(request.session_id.as_str())
            .map_err(|error| ToolCallError::InvalidInput(error.to_string()))?;
        self.release_acp_client_terminal_with_cancel(session_id, request, cancel_token)
            .await
    }
}

fn acp_terminal_env(env: ClientTerminalEnv) -> crate::AcpEnvVariable {
    crate::AcpEnvVariable {
        name: env.name,
        value: env.value,
        meta: None,
    }
}

fn client_terminal_exit_status(status: crate::AcpTerminalExitStatus) -> ClientTerminalExitStatus {
    ClientTerminalExitStatus {
        exit_code: status.exit_code,
        signal: status.signal,
    }
}

fn acp_terminal_wait_error(timeout: Duration, error: String) -> ToolCallError {
    if error.starts_with("client request timed out") {
        return ToolCallError::TimedOut(timeout.as_secs());
    }
    acp_terminal_request_error("terminal/wait_for_exit", error)
}

fn acp_terminal_request_error(method: &str, error: String) -> ToolCallError {
    if error == "client request cancelled" {
        return ToolCallError::Cancelled;
    }
    ToolCallError::ExecutionFailed(format!("client {method} failed: {error}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn client_capabilities_gate_terminal_methods() {
        let capabilities = crate::AcpClientCapabilities {
            terminal: true,
            ..crate::AcpClientCapabilities::default()
        };

        assert!(capabilities.terminal);
        assert!(!crate::AcpClientCapabilities::default().terminal);
    }
}
