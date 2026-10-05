use devo_protocol::TurnFailureReason;
use devo_protocol::native::error::AgentError;
use devo_provider::error::ProviderError;
use devo_provider::recovery_hint_for_anyhow;

pub(super) fn turn_failure_reason_from_error(
    error: &devo_core::AgentError,
) -> Option<TurnFailureReason> {
    match error {
        devo_core::AgentError::MaxTurnsExceeded(_) => Some(TurnFailureReason::MaxTurnRequests),
        devo_core::AgentError::Provider(_)
        | devo_core::AgentError::ContextTooLong
        | devo_core::AgentError::Aborted => None,
    }
}

/// Stamp a Native `AgentError` onto the failed turn at emit (no sidecar bag).
pub(super) fn turn_agent_error_from_error(error: &devo_core::AgentError) -> AgentError {
    let code = match error {
        devo_core::AgentError::Provider(source) => source
            .chain()
            .find_map(|cause| cause.downcast_ref::<ProviderError>())
            .map_or("PROVIDER_ERROR", ProviderError::error_code),
        devo_core::AgentError::MaxTurnsExceeded(_) => "MAX_TURNS_EXCEEDED",
        devo_core::AgentError::ContextTooLong => "CONTEXT_TOO_LONG",
        devo_core::AgentError::Aborted => "ABORTED",
    };
    let recovery_hint = match error {
        devo_core::AgentError::Provider(source) => recovery_hint_for_anyhow(source),
        devo_core::AgentError::MaxTurnsExceeded(_)
        | devo_core::AgentError::ContextTooLong
        | devo_core::AgentError::Aborted => None,
    };
    let mut agent_error = AgentError::new(code.to_string(), error.to_string());
    if let Some(hint) = recovery_hint {
        agent_error.details = Some(serde_json::json!({ "recoveryHint": hint }));
    }
    agent_error
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use devo_provider::NETWORK_PROXY_HINT;

    #[test]
    fn preserves_structured_provider_error_code() {
        let error = devo_core::AgentError::Provider(anyhow::Error::new(
            ProviderError::ProviderServerError {
                message: "Internal server error".to_string(),
                status_code: Some(500),
                provider_name: None,
            },
        ));
        let agent_error = turn_agent_error_from_error(&error);
        assert_eq!(agent_error.error_code, "PROVIDER_SERVER_ERROR");
        assert!(agent_error.message.contains("Internal server error"));
    }

    #[test]
    fn preserves_provider_recovery_hint() {
        // Force a network-ish message so the recovery hint helper can attach.
        let error = devo_core::AgentError::Provider(anyhow::Error::msg(format!(
            "connection failed via proxy: {NETWORK_PROXY_HINT}"
        )));
        let agent_error = turn_agent_error_from_error(&error);
        assert!(
            agent_error
                .details
                .as_ref()
                .and_then(|details| details.get("recoveryHint"))
                .is_some()
                || agent_error.message.contains("proxy")
        );
    }
}
