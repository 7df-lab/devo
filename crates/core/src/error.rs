use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("model provider error: {0}")]
    Provider(#[from] anyhow::Error),

    #[error("max turns ({0}) exceeded")]
    MaxTurnsExceeded(usize),

    #[error("context too long after compaction")]
    ContextTooLong,

    #[error("session aborted by user")]
    Aborted,
}
