use thiserror::Error;

use crate::contracts::ToolCallError;

#[derive(Debug, Clone, Error)]
pub enum ToolExecutionError {
    #[error("permission denied: {reason}")]
    PermissionDenied { reason: String },

    #[error("execution failed: {message}")]
    ExecutionFailed { message: String },

    #[error("timeout: {message}")]
    Timeout { message: String },

    #[error("interrupted")]
    Interrupted,

    #[error("internal: {message}")]
    Internal { message: String },
}

#[derive(Debug, Clone, Error)]
pub enum ToolDispatchError {
    #[error("unknown tool: {name}")]
    UnknownTool { name: String },

    #[error("{0}")]
    ExecutionError(#[from] ToolCallError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_error_display() {
        let err = ToolDispatchError::UnknownTool { name: "foo".into() };
        assert_eq!(err.to_string(), "unknown tool: foo");

        let err = ToolDispatchError::ExecutionError(ToolCallError::TimedOut(30));
        assert!(err.to_string().contains("timed out"));
    }

    #[test]
    fn dispatch_error_from_tool_call_error() {
        let exec = ToolCallError::ExecutionFailed("fail".into());
        let dispatch: ToolDispatchError = exec.into();
        assert!(matches!(dispatch, ToolDispatchError::ExecutionError(_)));
    }
}
