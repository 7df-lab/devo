//! Provider connection and query-stream timeout configuration.
//!
//! The query loop bounds stream creation and waits between normalized model
//! events. Continuous generation has no overall deadline. Non-streaming SDK
//! calls are not covered by that query-stream deadline.

use std::time::Duration;

/// TCP/TLS connection timeout for provider HTTP clients.
pub const CONNECT_TIMEOUT_SECS: u64 = 30;

/// TCP/TLS connection timeout for provider HTTP clients.
#[inline]
pub fn connect_timeout() -> Duration {
    Duration::from_secs(CONNECT_TIMEOUT_SECS)
}

/// Maximum query-stream creation or idle wait. Keepalives are not model events.
/// Partial model content is never automatically replayed after this deadline.
pub fn response_idle_timeout() -> Duration {
    Duration::from_secs(300)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn connect_timeout_is_thirty_seconds() {
        assert_eq!(connect_timeout(), Duration::from_secs(30));
    }
}
