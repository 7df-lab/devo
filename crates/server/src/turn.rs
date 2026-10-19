pub use devo_protocol::{
    ActiveTurnSteeringState, CollaborationMode, SteerInputRecord, TurnExecutionMode,
    TurnInputDisposition, TurnInterruptParams, TurnInterruptResult, TurnKind, TurnStartParams,
    TurnStartResult,
};

/// Native-first runtime turn held by the session actor.
///
/// `native` is the lifecycle source of truth. Extras retain only write-only
/// persistence / display fields that Native `Turn` does not model.
///
/// Turn kind on live paths is always `native.kind` (Native [`TurnKind`]).
/// Convert to legacy packed `devo_protocol::TurnKind` only at TurnRecord /
/// durable-record boundaries via [`legacy_turn_kind_from_native`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RuntimeTurn {
    pub(crate) native: devo_protocol::native::turn::Turn,
    pub(crate) extras: RuntimeTurnExtras,
}

/// Persistence / recovery scratch that is not a dual of Native `Turn`.
///
/// Model identity and reasoning live on `native.model` (`ModelBinding`).
/// Usage lives on `native.usage` (Native `TurnUsage`). These extras are
/// written into `TurnPersistenceExtras` (and recovery/title display) only.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RuntimeTurnExtras {
    pub(crate) request_thinking: Option<String>,
    pub(crate) stop_reason: Option<devo_core::StopReason>,
    pub(crate) failure_reason: Option<devo_protocol::TurnFailureReason>,
}

impl RuntimeTurn {
    pub(crate) fn new(
        native: devo_protocol::native::turn::Turn,
        extras: RuntimeTurnExtras,
    ) -> Self {
        Self { native, extras }
    }

    pub(crate) fn turn_id(&self) -> devo_protocol::native::ids::TurnId {
        self.native.id
    }

    #[allow(dead_code)]
    pub(crate) fn session_id(&self) -> devo_protocol::native::ids::SessionId {
        self.native.session_id
    }

    pub(crate) fn native_turn_id(&self) -> &devo_protocol::native::ids::TurnId {
        &self.native.id
    }

    /// Catalog / display model slug for history titles (Native snapshot).
    pub(crate) fn logical_model(&self) -> &str {
        self.native.model.model.as_str()
    }

    /// Provider binding id when present on the Native model snapshot.
    pub(crate) fn model_binding_id(&self) -> Option<&str> {
        (!self.native.model.provider.is_empty() && self.native.model.provider != "unknown")
            .then_some(self.native.model.provider.as_str())
    }

    /// Reasoning effort selection string derived from the Native model snapshot.
    pub(crate) fn reasoning_effort_selection(&self) -> Option<String> {
        self.native
            .model
            .reasoning_effort
            .map(|effort| effort.to_string())
    }
}

/// Packed `TurnRecord` / durable-record boundary only — not for live journal.
#[allow(dead_code)]
pub(crate) fn legacy_turn_kind_from_native(
    kind: devo_protocol::native::turn::TurnKind,
) -> devo_core::TurnKind {
    match kind {
        devo_protocol::native::turn::TurnKind::Regular => devo_core::TurnKind::Regular,
        devo_protocol::native::turn::TurnKind::Compaction => devo_core::TurnKind::ManualCompaction,
        devo_protocol::native::turn::TurnKind::GoalContinuation => {
            devo_core::TurnKind::Other("goal_continuation".to_string())
        }
    }
}
