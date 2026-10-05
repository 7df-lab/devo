//! Native-first resume hydrate structures.
//!
//! First-party v2 rollout lines land here as Native `Session` / `Turn` (plus
//! persistence extras). Legacy `SessionRecord` is **not** materialized for
//! actor ownership — `into_runtime_session` installs `rollout_path` + Native
//! `RuntimeSessionSummary` only. Completed turns stay as [`ReplayedTurn`] on
//! `RuntimeSession::turns_by_id` for fork/rollback cuts and Native history copy.
//!
//! Live append constructs [`devo_core::RolloutLine`] directly (typed wrappers
//! use `rollout_write` helpers); this module only consumes v2 on resume.

use chrono::{DateTime, Utc};
use devo_core::SessionPersistenceExtras;
use devo_core::TurnPersistenceExtras;
use devo_protocol::native::session::Session;
use devo_protocol::native::turn::{Turn, TurnStatus as NativeTurnStatus};

use crate::turn::{RuntimeTurn, RuntimeTurnExtras};

/// Session snapshot accumulated during Native-first resume.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ReplayedSession {
    pub(crate) native: Session,
    pub(crate) extras: SessionPersistenceExtras,
    /// Metadata update time (title / settings / activity side-channels).
    pub(crate) updated_at: DateTime<Utc>,
    /// Sub-agent display fields not modeled on Native `Session`.
    pub(crate) agent_path: Option<String>,
    pub(crate) agent_nickname: Option<String>,
}

impl ReplayedSession {
    pub(crate) fn from_native(
        native: Session,
        extras: Option<SessionPersistenceExtras>,
        updated_at: DateTime<Utc>,
    ) -> Self {
        let extras = extras.unwrap_or_else(|| SessionPersistenceExtras {
            session_context: None,
            cli_version: String::new(),
            source: String::new(),
            collaboration_mode: None,
            permission_preset: None,
            kernel_snapshot_path: None,
        });
        Self {
            native,
            extras,
            updated_at,
            agent_path: None,
            agent_nickname: None,
        }
    }

    pub(crate) fn legacy_session_id(&self) -> devo_core::SessionId {
        self.native.id
    }

    pub(crate) fn agent_role(&self) -> Option<String> {
        match &self.native.parent {
            Some(devo_protocol::native::session::SessionParent::Agent { role, .. }) => role.clone(),
            None => None,
        }
    }
}

/// Turn snapshot accumulated during Native-first resume (and kept live for
/// fork/rollback cuts).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ReplayedTurn {
    pub(crate) native: Turn,
    pub(crate) extras: TurnPersistenceExtras,
}

impl ReplayedTurn {
    pub(crate) fn from_native(native: Turn, extras: Option<TurnPersistenceExtras>) -> Self {
        Self {
            native,
            extras: extras.unwrap_or(TurnPersistenceExtras {
                session_context: None,
                turn_context: None,
                request_thinking: None,
                input_token_estimate: None,
                latest_query_usage: None,
                context_occupancy: None,
                stop_reason: None,
                failure_reason: None,
            }),
        }
    }

    pub(crate) fn legacy_turn_id(&self) -> devo_core::TurnId {
        self.native
            .id
            .as_str()
            .parse()
            .expect("replay Native turn ids originate as legacy UUIDs")
    }

    pub(crate) fn legacy_status(&self) -> devo_core::TurnStatus {
        match self.native.status {
            NativeTurnStatus::InProgress => devo_core::TurnStatus::Running,
            NativeTurnStatus::WaitingApproval => devo_core::TurnStatus::WaitingApproval,
            NativeTurnStatus::Completed => devo_core::TurnStatus::Completed,
            NativeTurnStatus::Interrupted => devo_core::TurnStatus::Interrupted,
            NativeTurnStatus::Failed => devo_core::TurnStatus::Failed,
        }
    }

    pub(crate) fn into_runtime_turn(self) -> RuntimeTurn {
        RuntimeTurn::from_native(self.native, &self.extras)
    }
}

impl RuntimeTurn {
    /// Builds a runtime turn from a Native v2 turn line (plus persistence extras).
    pub(crate) fn from_native(native: Turn, extras: &TurnPersistenceExtras) -> Self {
        Self {
            native,
            extras: RuntimeTurnExtras {
                request_thinking: extras.request_thinking.clone(),
                stop_reason: extras.stop_reason.clone(),
                failure_reason: extras.failure_reason,
            },
        }
    }
}
