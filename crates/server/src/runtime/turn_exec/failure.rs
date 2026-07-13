use devo_protocol::{TurnErrorPayload, TurnFailureReason};
use devo_provider::error::ProviderError;

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

pub(super) fn turn_error_payload_from_error(error: &devo_core::AgentError) -> TurnErrorPayload {
    let code = match error {
        devo_core::AgentError::Provider(source) => source
            .chain()
            .find_map(|cause| cause.downcast_ref::<ProviderError>())
            .map_or("PROVIDER_ERROR", ProviderError::error_code),
        devo_core::AgentError::MaxTurnsExceeded(_) => "MAX_TURNS_EXCEEDED",
        devo_core::AgentError::ContextTooLong => "CONTEXT_TOO_LONG",
        devo_core::AgentError::Aborted => "ABORTED",
    };
    TurnErrorPayload {
        code: code.to_string(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn preserves_structured_provider_error_code() {
        let error = devo_core::AgentError::Provider(anyhow::Error::new(
            ProviderError::ProviderServerError {
                message: "Internal server error".to_string(),
                status_code: Some(500),
                provider_name: Some("openai".to_string()),
            },
        ));

        assert_eq!(
            turn_error_payload_from_error(&error),
            TurnErrorPayload {
                code: "PROVIDER_SERVER_ERROR".to_string(),
                message:
                    "model provider error: provider server error (Some(500)): Internal server error"
                        .to_string(),
            }
        );
    }
}
