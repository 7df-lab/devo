use super::super::*;

use devo_core::ServerAuthConfig;

use crate::AcpAuthCapabilities;
use crate::AcpAuthMethod;
use crate::AcpAuthenticateParams;
use crate::AcpAuthenticateResult;
use crate::AcpEmptyAuthResult;
use crate::AcpErrorCode;
use crate::AcpErrorResponse;
use crate::AcpLogoutResult;
use crate::AcpSuccessResponse;

impl ServerRuntime {
    pub(crate) fn acp_auth_config(&self) -> ServerAuthConfig {
        self.deps
            .config_store
            .lock()
            .expect("app config store mutex should not be poisoned")
            .effective_config()
            .server
            .auth
            .clone()
    }

    pub(crate) fn acp_auth_enabled(&self) -> bool {
        self.acp_auth_config().enabled
    }

    pub(crate) fn acp_auth_methods(config: &ServerAuthConfig) -> Vec<AcpAuthMethod> {
        if !config.enabled {
            return Vec::new();
        }

        vec![AcpAuthMethod::agent(
            config.method_id.clone(),
            config.name.clone(),
            config.description.clone(),
        )]
    }

    pub(crate) fn acp_auth_capabilities(config: &ServerAuthConfig) -> AcpAuthCapabilities {
        if config.enabled && config.logout {
            AcpAuthCapabilities::with_logout()
        } else {
            AcpAuthCapabilities::default()
        }
    }

    pub(crate) async fn set_connection_authenticated(
        &self,
        connection_id: u64,
        authenticated: bool,
    ) {
        if let Some(connection) = self.connections.lock().await.get_mut(&connection_id) {
            connection.acp_authenticated = authenticated;
        }
    }

    pub(crate) async fn connection_authenticated(&self, connection_id: u64) -> bool {
        if !self.acp_auth_enabled() {
            return true;
        }

        self.connections
            .lock()
            .await
            .get(&connection_id)
            .is_some_and(|connection| connection.acp_authenticated)
    }

    pub(crate) async fn handle_acp_authenticate(
        &self,
        connection_id: u64,
        id: Option<serde_json::Value>,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let request_id = id.unwrap_or(serde_json::Value::Null);
        let params: AcpAuthenticateParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return acp_auth_error_response(
                    request_id,
                    AcpErrorCode::InvalidParams,
                    format!("invalid authenticate params: {error}"),
                );
            }
        };
        let config = self.acp_auth_config();
        if config.enabled && params.method_id != config.method_id {
            return acp_auth_error_response(
                request_id,
                AcpErrorCode::InvalidParams,
                "authenticate methodId does not match an advertised authentication method",
            );
        }

        self.set_connection_authenticated(connection_id, true).await;
        acp_auth_success_response(request_id, AcpAuthenticateResult::default())
    }

    pub(crate) async fn handle_acp_logout(
        &self,
        connection_id: u64,
        id: Option<serde_json::Value>,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let request_id = id.unwrap_or(serde_json::Value::Null);
        if let Err(error) = serde_json::from_value::<AcpEmptyAuthResult>(params) {
            return acp_auth_error_response(
                request_id,
                AcpErrorCode::InvalidParams,
                format!("invalid logout params: {error}"),
            );
        }
        let config = self.acp_auth_config();
        if !config.enabled || !config.logout {
            return acp_auth_error_response(
                request_id,
                AcpErrorCode::MethodNotFound,
                "logout is not supported",
            );
        }

        self.set_connection_authenticated(connection_id, false)
            .await;
        acp_auth_success_response(request_id, AcpLogoutResult::default())
    }
}

fn acp_auth_success_response<T: serde::Serialize>(
    request_id: serde_json::Value,
    result: T,
) -> serde_json::Value {
    serde_json::to_value(AcpSuccessResponse::new(request_id, result))
        .expect("serialize server auth success response")
}

fn acp_auth_error_response(
    request_id: serde_json::Value,
    code: AcpErrorCode,
    message: impl Into<String>,
) -> serde_json::Value {
    serde_json::to_value(AcpErrorResponse::new(
        request_id,
        code,
        message,
        serde_json::Value::Null,
    ))
    .expect("serialize server auth error response")
}
