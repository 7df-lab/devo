use crate::tools::{DeferredLoadingConfig, LoadedDeferredTools, ToolRegistry, ToolSearchMetrics};
use devo_protocol::ToolDefinition;

#[derive(Debug, Clone)]
pub struct ToolPromptSurface {
    pub system: Option<String>,
    pub tools: Vec<ToolDefinition>,
    pub metrics: ToolSearchMetrics,
    pub deferred_tool_names: Vec<String>,
    pub loaded_deferred_tool_names: Vec<String>,
}

pub fn build_deferred_tool_prompt_surface(
    base_system: Option<String>,
    registry: &ToolRegistry,
    session_id: &str,
    loaded_tools: &LoadedDeferredTools,
    config: &DeferredLoadingConfig,
) -> ToolPromptSurface {
    let prompt = registry.deferred_tool_prompt(session_id, loaded_tools, config);
    let system = match (base_system, prompt.reminder) {
        (Some(system), Some(reminder)) if !system.is_empty() => {
            Some(format!("{system}\n\n{reminder}"))
        }
        (Some(_), Some(reminder)) | (None, Some(reminder)) => Some(reminder),
        (Some(system), None) if !system.is_empty() => Some(system),
        _ => None,
    };

    ToolPromptSurface {
        system,
        tools: prompt.exposed,
        metrics: prompt.metrics,
        deferred_tool_names: prompt.deferred.into_iter().map(|tool| tool.name).collect(),
        loaded_deferred_tool_names: prompt.loaded_deferred,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::json_schema::JsonSchema;
    use crate::tools::registry::ToolRegistryBuilder;
    use crate::tools::tool_spec::{
        ToolExecutionMode, ToolOutputMode, ToolPreparationFeedback, ToolSpec,
    };
    use pretty_assertions::assert_eq;

    fn spec(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.to_string(),
            description: format!("{name} description"),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: true,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        }
    }

    fn registry() -> ToolRegistry {
        let mut builder = ToolRegistryBuilder::new();
        for name in [
            "read",
            "grep",
            "ToolSearch",
            "web_search",
            "multi_tool_use",
            "secret_internal",
        ] {
            builder.push_spec(spec(name));
        }
        builder.build()
    }

    #[test]
    fn model_surface_exposes_preloaded_tools_and_reminds_about_deferred_tools() {
        let mut config = DeferredLoadingConfig::default();
        config.hidden.insert("secret_internal".to_string());

        let surface = build_deferred_tool_prompt_surface(
            Some("base system".to_string()),
            &registry(),
            "session-1",
            &LoadedDeferredTools::default(),
            &config,
        );

        assert_eq!(
            surface
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["read", "grep", "ToolSearch"]
        );
        assert_eq!(
            surface.deferred_tool_names,
            vec!["web_search".to_string(), "multi_tool_use".to_string()]
        );
        let system = surface.system.expect("system reminder should be present");
        assert!(system.starts_with("base system\n\n<system-reminder>"));
        assert!(system.contains("web_search: web_search description"));
        assert!(system.contains("multi_tool_use: multi_tool_use description"));
        assert!(!system.contains("secret_internal"));
        assert_eq!(surface.metrics.exposed_tool_count, 3);
        assert_eq!(surface.metrics.hidden_tool_count, 2);
    }

    #[test]
    fn model_surface_keeps_loaded_deferred_schema_and_omits_it_from_reminder() {
        let mut loaded = LoadedDeferredTools::default();
        loaded.mark_loaded("session-1", "web_search");

        let surface = build_deferred_tool_prompt_surface(
            None,
            &registry(),
            "session-1",
            &loaded,
            &DeferredLoadingConfig::default(),
        );

        assert_eq!(
            surface
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["read", "grep", "ToolSearch", "web_search"]
        );
        assert_eq!(surface.loaded_deferred_tool_names, vec!["web_search"]);
        let system = surface.system.expect("remaining deferred reminder");
        assert!(!system.contains("web_search:"));
        assert!(system.contains("multi_tool_use:"));
        assert_eq!(surface.metrics.loaded_deferred_tool_count, 1);
    }
}
