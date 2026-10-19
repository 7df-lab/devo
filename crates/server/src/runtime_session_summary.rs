use std::ops::{Deref, DerefMut};

use chrono::{DateTime, Utc};
use devo_protocol::CollaborationMode;
use devo_protocol::native::session::{Session, SessionParent, SessionStatus};

/// Actor-owned session summary.
///
/// Native `Session` is the source of truth. The remaining fields are runtime
/// details that have no Native session representation yet.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RuntimeSessionSummary {
    pub(crate) native: Session,
    pub(crate) updated_at: DateTime<Utc>,
    pub(crate) agent_path: Option<String>,
    pub(crate) agent_nickname: Option<String>,
    pub(crate) agent_role: Option<String>,
    pub(crate) prompt_token_estimate: usize,
    pub(crate) last_query_usage: Option<devo_protocol::native::usage::TurnUsage>,
    pub(crate) last_query_total_tokens: usize,
    pub(crate) last_context_occupancy: Option<devo_protocol::native::item::ContextOccupancy>,
    pub(crate) collaboration_mode: CollaborationMode,
}

impl RuntimeSessionSummary {
    pub(crate) fn new(
        native: Session,
        updated_at: DateTime<Utc>,
        collaboration_mode: CollaborationMode,
    ) -> Self {
        Self {
            native,
            updated_at,
            agent_path: None,
            agent_nickname: None,
            agent_role: None,
            prompt_token_estimate: 0,
            last_query_usage: None,
            last_query_total_tokens: 0,
            last_context_occupancy: None,
            collaboration_mode,
        }
    }

    pub(crate) fn session_id(&self) -> devo_protocol::native::ids::SessionId {
        self.native.id
    }

    pub(crate) fn parent_session_id(&self) -> Option<devo_protocol::native::ids::SessionId> {
        let SessionParent::Agent { session_id, .. } = self.native.parent.as_ref()?;
        Some(*session_id)
    }

    pub(crate) fn model_selection(&self) -> Option<&str> {
        (!self.native.model.provider.is_empty() && self.native.model.provider != "unknown")
            .then_some(self.native.model.provider.as_str())
            .or_else(|| {
                (!self.native.model.model.is_empty()).then_some(self.native.model.model.as_str())
            })
    }

    pub(crate) fn model_name(&self) -> Option<&str> {
        (!self.native.model.model.is_empty()).then_some(self.native.model.model.as_str())
    }

    pub(crate) fn model_binding_id(&self) -> Option<&str> {
        (!self.native.model.provider.is_empty() && self.native.model.provider != "unknown")
            .then_some(self.native.model.provider.as_str())
    }

    pub(crate) fn permission_preset(&self) -> Option<devo_protocol::PermissionPreset> {
        Some(match self.native.settings.permission_profile {
            devo_protocol::native::model::PermissionProfile::Default => {
                devo_protocol::PermissionPreset::Default
            }
            devo_protocol::native::model::PermissionProfile::AutoReview => {
                devo_protocol::PermissionPreset::AutoReview
            }
            devo_protocol::native::model::PermissionProfile::FullAccess => {
                devo_protocol::PermissionPreset::FullAccess
            }
        })
    }

    pub(crate) fn set_permission_preset(&mut self, preset: devo_protocol::PermissionPreset) {
        self.native.settings.permission_profile = match preset {
            devo_protocol::PermissionPreset::Default => {
                devo_protocol::native::model::PermissionProfile::Default
            }
            devo_protocol::PermissionPreset::AutoReview => {
                devo_protocol::native::model::PermissionProfile::AutoReview
            }
            devo_protocol::PermissionPreset::FullAccess => {
                devo_protocol::native::model::PermissionProfile::FullAccess
            }
        };
    }

    pub(crate) fn total_input_tokens(&self) -> usize {
        self.native.usage.total.input_tokens as usize
    }

    pub(crate) fn total_output_tokens(&self) -> usize {
        self.native.usage.total.output_tokens as usize
    }

    pub(crate) fn total_tokens(&self) -> usize {
        self.native.usage.total.total_tokens as usize
    }

    pub(crate) fn total_cache_creation_tokens(&self) -> usize {
        self.native.usage.total.cache_creation_input_tokens as usize
    }

    pub(crate) fn total_cache_read_tokens(&self) -> usize {
        self.native.usage.total.cache_read_input_tokens as usize
    }

    pub(crate) fn set_cumulative_usage(
        &mut self,
        input: usize,
        output: usize,
        total: usize,
        cache_creation: usize,
        cache_read: usize,
    ) {
        self.native.usage.total.input_tokens = input as u64;
        self.native.usage.total.output_tokens = output as u64;
        self.native.usage.total.total_tokens = total as u64;
        self.native.usage.total.cache_creation_input_tokens = cache_creation as u64;
        self.native.usage.total.cache_read_input_tokens = cache_read as u64;
        self.native.usage.updated_at = self.updated_at;
    }

    pub(crate) fn set_status(&mut self, status: SessionStatus) {
        self.native.status = status;
        if matches!(status, SessionStatus::Idle) {
            self.native.active_turn_id = None;
        }
        self.native.sync_activity();
    }

    pub(crate) fn status_changed_notification(
        &self,
    ) -> devo_protocol::native::event::ServerNotification {
        devo_protocol::native::event::ServerNotification::session_status_changed_from_session(
            &self.native,
        )
    }
}

impl Deref for RuntimeSessionSummary {
    type Target = Session;

    fn deref(&self) -> &Self::Target {
        &self.native
    }
}

impl DerefMut for RuntimeSessionSummary {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.native
    }
}
