//! Targeted MCP updates preserve the existing registry's host tools and settings.
use std::collections::HashSet;
use std::sync::Arc;

use super::handlers::{McpToolHandler, mcp_search_text, mcp_tool_spec};
use super::registry::{ToolExposure, ToolRegistry};
use crate::mcp::{McpManager, McpServerId, McpToolInfo, build_mcp_tool_exposure};

impl ToolRegistry {
    /// Replace only one server's tools, using already discovered metadata.
    /// No discovery, transport startup, or tool invocation occurs here.
    pub fn replace_mcp_server_tools(
        &self,
        server_id: &McpServerId,
        tools: Vec<McpToolInfo>,
        manager: Arc<dyn McpManager>,
    ) -> Self {
        // A session's runtime context can share the process manager while its
        // explicit ACP registry belongs to a different connection owner.
        if self
            .mcp_publication_owner
            .as_ref()
            .is_some_and(|owner| !Arc::ptr_eq(owner, &manager))
        {
            return self.clone();
        }
        let mut registry = self.clone();
        // Keep ownership even when withdrawing the last tool, so a foreign
        // manager cannot claim the empty registry on its next publication.
        registry.mcp_publication_owner = Some(Arc::clone(&manager));
        let sibling_names: HashSet<_> = self
            .mcp_tools
            .iter()
            .filter(|info| &info.server_id != server_id)
            .map(|info| info.flat_name.clone())
            .collect();
        let old_names: HashSet<_> = self
            .mcp_tools
            .iter()
            .filter(|info| &info.server_id == server_id)
            .map(|info| info.flat_name.clone())
            .filter(|name| !sibling_names.contains(name))
            .collect();
        registry
            .specs
            .retain(|spec| !old_names.contains(&spec.name));
        registry
            .handlers
            .retain(|name, _| !old_names.contains(name));
        registry
            .spec_exposure
            .retain(|name, _| !old_names.contains(name));
        registry
            .spec_search_text
            .retain(|name, _| !old_names.contains(name));
        registry
            .mcp_tools
            .retain(|info| &info.server_id != server_id);
        let occupied_names: HashSet<_> = registry
            .specs
            .iter()
            .map(|spec| spec.name.clone())
            .collect();
        // Ignore mismatched or colliding metadata rather than replace sibling/host tools.
        registry.mcp_tools.extend(tools.into_iter().filter(|info| {
            &info.server_id == server_id && !occupied_names.contains(&info.flat_name)
        }));
        let exposure = build_mcp_tool_exposure(&registry.mcp_tools);
        for (infos, policy) in [
            (exposure.direct_tools, ToolExposure::Direct),
            (exposure.deferred_tools, ToolExposure::Deferred),
        ] {
            for info in infos {
                let name = info.flat_name.clone();
                let policy = if registry
                    .model_tool_allowlist
                    .as_ref()
                    .is_some_and(|names| !names.contains(&name))
                {
                    ToolExposure::Hidden
                } else {
                    policy
                };
                // Threshold changes affect MCP exposure, but never sibling definitions/handlers.
                registry.spec_exposure.insert(name.clone(), policy);
                if &info.server_id != server_id {
                    continue;
                }
                if registry.handlers.contains_key(&name) {
                    continue;
                }
                registry
                    .spec_search_text
                    .insert(name.clone(), mcp_search_text(&info));
                registry.specs.push(mcp_tool_spec(&info));
                registry.handlers.insert(
                    name,
                    Arc::new(McpToolHandler::new(Arc::clone(&manager), info)),
                );
            }
        }
        registry.spec_index = registry
            .specs
            .iter()
            .enumerate()
            .map(|(index, spec)| (spec.name.clone(), index))
            .collect();
        if registry.handlers.contains_key("ToolSearch") {
            let entries = registry
                .tool_definitions()
                .into_iter()
                .map(|definition| {
                    let text = registry.spec_search_text.get(&definition.name).cloned();
                    (definition, text)
                })
                .collect();
            registry.handlers.insert(
                "ToolSearch".into(),
                Arc::new(super::handlers::ToolSearchHandler::new(
                    entries,
                    Arc::clone(&registry.loaded_deferred_tools),
                    registry.effective_deferred_loading_config(
                        &crate::deferred_loading::DeferredLoadingConfig::default(),
                    ),
                )),
            );
        }
        registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::{McpError, McpServerStatus};
    use async_trait::async_trait;
    use pretty_assertions::assert_eq;
    use serde_json::{Value, json};

    struct NoNetwork;
    #[async_trait]
    impl McpManager for NoNetwork {
        async fn statuses(&self) -> Result<Vec<McpServerStatus>, McpError> {
            panic!("no network")
        }
        async fn discover_tools(&self) -> Result<Vec<McpToolInfo>, McpError> {
            panic!("no network")
        }
        async fn refresh(&self, _: &McpServerId) -> Result<McpServerStatus, McpError> {
            panic!("no network")
        }
        async fn set_enabled(&self, _: &McpServerId, _: bool) -> Result<McpServerStatus, McpError> {
            panic!("no network")
        }
        async fn invoke_tool(&self, _: &McpServerId, _: &str, _: Value) -> Result<Value, McpError> {
            panic!("no calls")
        }
        async fn read_resource(&self, _: &McpServerId, _: &str) -> Result<Value, McpError> {
            panic!("no network")
        }
    }
    fn info(id: &str, name: &str) -> McpToolInfo {
        McpToolInfo::new(
            McpServerId(id.into()),
            id.into(),
            name.into(),
            Some(name.into()),
            json!({"type":"object", "properties":{name:{"type":"string"}}}),
            /*read_only_hint*/ true,
            /*supports_parallel_tool_calls*/ false,
        )
    }

    #[test]
    fn targeted_replacement_preserves_host_settings_siblings_and_purges_target() {
        let manager: Arc<dyn McpManager> = Arc::new(NoNetwork);
        let base =
            super::super::handlers::build_registry_from_plan(&super::super::ToolPlanConfig {
                web_fetch: false,
                web_search: false,
                ..Default::default()
            });
        let target = McpServerId("target".into());
        let sibling = McpServerId("sibling".into());
        let before = base
            .replace_mcp_server_tools(&target, vec![info("target", "old")], Arc::clone(&manager))
            .replace_mcp_server_tools(
                &sibling,
                vec![info("sibling", "stable")],
                Arc::clone(&manager),
            );
        let after = before.replace_mcp_server_tools(
            &target,
            vec![info("target", "new")],
            Arc::clone(&manager),
        );
        let sibling_name = "mcp__sibling__stable";
        assert_eq!(
            serde_json::to_value(after.spec(sibling_name)).unwrap(),
            serde_json::to_value(before.spec(sibling_name)).unwrap()
        );
        assert!(Arc::ptr_eq(
            after.get(sibling_name).unwrap(),
            before.get(sibling_name).unwrap()
        ));
        assert!(after.get("mcp__target__old").is_none());
        assert!(after.get("mcp__target__new").is_some());
        assert_eq!(after.model_tool_allowlist(), base.model_tool_allowlist());
        assert_eq!(
            after.spec_exposure["mcp__target__new"],
            ToolExposure::Hidden
        );
        assert!(Arc::ptr_eq(
            &after.loaded_deferred_tools,
            &base.loaded_deferred_tools
        ));
        assert!(Arc::ptr_eq(
            after.unified_exec_store.as_ref().unwrap(),
            base.unified_exec_store.as_ref().unwrap()
        ));
        let host_definitions = |registry: &ToolRegistry| {
            registry
                .tool_definitions()
                .into_iter()
                .filter(|definition| !definition.name.starts_with("mcp__"))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            serde_json::to_value(host_definitions(&after)).unwrap(),
            serde_json::to_value(host_definitions(&base)).unwrap()
        );
        let mut changed = info("target", "new");
        changed.input_schema =
            json!({"type":"object", "properties":{"changed":{"type":"boolean"}}});
        let changed_schema = changed.input_schema.clone();
        let after = after.replace_mcp_server_tools(&target, vec![changed], Arc::clone(&manager));
        assert_eq!(
            after
                .spec("mcp__target__new")
                .unwrap()
                .input_schema
                .to_json_value(),
            changed_schema
        );
        let purged = after.replace_mcp_server_tools(&target, Vec::new(), manager);
        assert!(purged.get("mcp__target__new").is_none());
        assert_eq!(
            serde_json::to_value(purged.spec(sibling_name)).unwrap(),
            serde_json::to_value(before.spec(sibling_name)).unwrap()
        );
    }

    #[test]
    fn replacement_recomputes_threshold_without_replacing_sibling_schemas() {
        let manager: Arc<dyn McpManager> = Arc::new(NoNetwork);
        let sibling = McpServerId("sibling".into());
        let target = McpServerId("target".into());
        let sibling_tools = (0..99)
            .map(|index| info("sibling", &format!("tool_{index}")))
            .collect();
        let base = ToolRegistry::new().replace_mcp_server_tools(
            &sibling,
            sibling_tools,
            Arc::clone(&manager),
        );
        let full = base.replace_mcp_server_tools(
            &target,
            vec![info("target", "new")],
            Arc::clone(&manager),
        );
        assert_eq!(
            full.spec_exposure["mcp__sibling__tool_0"],
            ToolExposure::Deferred
        );
        let reduced = full.replace_mcp_server_tools(&target, Vec::new(), manager);
        assert_eq!(
            reduced.spec_exposure["mcp__sibling__tool_0"],
            ToolExposure::Direct
        );
        assert_eq!(
            serde_json::to_value(base.tool_definitions()).unwrap(),
            serde_json::to_value(reduced.tool_definitions()).unwrap()
        );
    }

    #[tokio::test]
    async fn foreign_manager_cannot_claim_restricted_or_layered_session_registry() {
        let owner: Arc<dyn McpManager> = Arc::new(NoNetwork);
        let foreign: Arc<dyn McpManager> = Arc::new(NoNetwork);
        let id = McpServerId("target".into());
        let registry = ToolRegistry::new()
            .replace_mcp_server_tools(&id, vec![info("target", "session")], Arc::clone(&owner))
            .restricted_to_specs(&["mcp__target__session"]);
        let layered = super::super::handlers::build_registry_from_plan_with_session_mcp(
            &super::super::ToolPlanConfig {
                execution_surface: devo_kernel::ExecutionSurface::Rlm,
                ..Default::default()
            },
            Arc::clone(&foreign),
            &registry,
        )
        .await;
        for before in [registry, layered] {
            for tools in [Vec::new(), vec![info("target", "global")]] {
                let after = before.replace_mcp_server_tools(&id, tools, Arc::clone(&foreign));
                assert_eq!(
                    serde_json::to_value(after.tool_definitions()).unwrap(),
                    serde_json::to_value(before.tool_definitions()).unwrap()
                );
                assert!(Arc::ptr_eq(
                    after.get("mcp__target__session").unwrap(),
                    before.get("mcp__target__session").unwrap()
                ));
            }
            let empty = before.replace_mcp_server_tools(&id, Vec::new(), Arc::clone(&owner));
            let after = empty.replace_mcp_server_tools(
                &id,
                vec![info("target", "global")],
                Arc::clone(&foreign),
            );
            assert_eq!(
                serde_json::to_value(after.tool_definitions()).unwrap(),
                serde_json::to_value(empty.tool_definitions()).unwrap()
            );
        }
    }

    #[tokio::test]
    async fn default_reconnect_is_unsupported_without_refreshing() {
        assert!(
            NoNetwork
                .reconnect(&McpServerId("server".into()))
                .await
                .unwrap_err()
                .to_string()
                .contains("unsupported")
        );
    }
}
