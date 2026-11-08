//! Child turns revalidate their persisted binding instead of falling back to
//! process defaults after a configuration change or session restore.

use crate::runtime_session_summary::RuntimeSessionSummary;
use crate::session_context::SessionRuntimeContext;

impl SessionRuntimeContext {
    pub(crate) fn resolve_session_turn_config(
        &self,
        summary: &RuntimeSessionSummary,
        requested_model: Option<&str>,
        thinking: Option<String>,
    ) -> anyhow::Result<devo_core::TurnConfig> {
        if summary.parent_session_id().is_none() {
            return Ok(self.resolve_turn_config(requested_model, thinking));
        }
        let bound = summary
            .model_selection()
            .ok_or_else(|| anyhow::anyhow!("child session has no persisted model binding"))?;
        let selected = requested_model.unwrap_or(bound);
        let changed_thinking = thinking
            .as_deref()
            .filter(|thinking| Some(*thinking) != summary.settings.reasoning_effort.as_deref());
        self.resolve_child_model(
            None,
            selected,
            changed_thinking,
            summary.settings.reasoning_effort.as_deref(),
        )
        .map(|(_, config)| config)
    }
}
