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
        use devo_core::ResolvedWebSearchConfig;
        use devo_protocol::native::session::WebSearchChoice;

        let mut config = if summary.parent_session_id().is_none() {
            self.resolve_turn_config(requested_model, thinking)
        } else {
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
            )?
            .1
        };
        if let Some(choice) = summary.settings.web_search.as_ref() {
            config.web_search = match choice {
                WebSearchChoice::Inherit => config.web_search,
                WebSearchChoice::Provider => ResolvedWebSearchConfig::Provider,
                WebSearchChoice::Off => ResolvedWebSearchConfig::Disabled,
                WebSearchChoice::Mcp {
                    server_id,
                    tool_name,
                } => ResolvedWebSearchConfig::Mcp {
                    server_id: server_id.clone(),
                    tool_name: tool_name.clone(),
                },
            };
        }
        Ok(config)
    }
}
