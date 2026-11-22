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
            apply_web_search_choice(&mut config.web_search, &mut config.web_fetch, choice);
        }
        Ok(config)
    }
}

fn apply_web_search_choice(
    search: &mut devo_core::ResolvedWebSearchConfig,
    fetch: &mut devo_core::ResolvedWebFetchConfig,
    choice: &devo_protocol::native::session::WebSearchChoice,
) {
    use devo_core::{ResolvedWebFetchConfig, ResolvedWebSearchConfig};
    use devo_protocol::native::session::{WebBackendChoice, WebSearchChoice};

    match choice {
        WebSearchChoice::Inherit => {}
        WebSearchChoice::Provider => *search = ResolvedWebSearchConfig::Provider,
        WebSearchChoice::Off => *search = ResolvedWebSearchConfig::Disabled,
        WebSearchChoice::Mcp {
            server_id,
            tool_name,
        } => {
            *search = ResolvedWebSearchConfig::Mcp {
                server_id: server_id.clone(),
                tool_name: tool_name.clone(),
            };
        }
        WebSearchChoice::Unified { backend } => match backend {
            WebBackendChoice::Provider => {
                *search = ResolvedWebSearchConfig::Provider;
                *fetch = ResolvedWebFetchConfig::Provider;
            }
            WebBackendChoice::Off => {
                *search = ResolvedWebSearchConfig::Disabled;
                *fetch = ResolvedWebFetchConfig::Disabled;
            }
            WebBackendChoice::Mcp {
                server_id,
                tool_name,
            } => {
                *search = ResolvedWebSearchConfig::Mcp {
                    server_id: server_id.clone(),
                    tool_name: tool_name.clone(),
                };
                *fetch = match super::web_search_builtin::supplier_for_server_id(server_id) {
                    Some(provider) if tool_name == super::web_search_builtin::SEARCH_TOOL => {
                        ResolvedWebFetchConfig::Mcp { provider }
                    }
                    Some(_) => ResolvedWebFetchConfig::Disabled,
                    None => ResolvedWebFetchConfig::CustomMcp {
                        server_id: server_id.clone(),
                        tool_name: "web_fetch".into(),
                    },
                };
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::apply_web_search_choice;
    use devo_core::{ResolvedWebFetchConfig, ResolvedWebSearchConfig};
    use devo_protocol::native::session::{WebBackendChoice, WebSearchChoice};

    #[test]
    fn legacy_choices_keep_the_configured_fetch_backend() {
        let initial_fetch = ResolvedWebFetchConfig::Local;
        for (choice, expected_search) in [
            (WebSearchChoice::Provider, ResolvedWebSearchConfig::Provider),
            (WebSearchChoice::Off, ResolvedWebSearchConfig::Disabled),
            (
                WebSearchChoice::Mcp {
                    server_id: "docs".into(),
                    tool_name: "search".into(),
                },
                ResolvedWebSearchConfig::Mcp {
                    server_id: "docs".into(),
                    tool_name: "search".into(),
                },
            ),
        ] {
            let mut search = ResolvedWebSearchConfig::Disabled;
            let mut fetch = initial_fetch.clone();
            apply_web_search_choice(&mut search, &mut fetch, &choice);
            assert_eq!((search, fetch), (expected_search, initial_fetch.clone()));
        }
    }

    #[test]
    fn unified_choices_set_both_capabilities() {
        for (backend, expected_search, expected_fetch) in [
            (
                WebBackendChoice::Provider,
                ResolvedWebSearchConfig::Provider,
                ResolvedWebFetchConfig::Provider,
            ),
            (
                WebBackendChoice::Off,
                ResolvedWebSearchConfig::Disabled,
                ResolvedWebFetchConfig::Disabled,
            ),
            (
                WebBackendChoice::Mcp {
                    server_id: "docs".into(),
                    tool_name: "search".into(),
                },
                ResolvedWebSearchConfig::Mcp {
                    server_id: "docs".into(),
                    tool_name: "search".into(),
                },
                ResolvedWebFetchConfig::CustomMcp {
                    server_id: "docs".into(),
                    tool_name: "web_fetch".into(),
                },
            ),
        ] {
            let mut search = ResolvedWebSearchConfig::Disabled;
            let mut fetch = ResolvedWebFetchConfig::Local;
            apply_web_search_choice(
                &mut search,
                &mut fetch,
                &WebSearchChoice::Unified { backend },
            );
            assert_eq!((search, fetch), (expected_search, expected_fetch));
        }
    }
}
