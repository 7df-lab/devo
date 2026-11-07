use std::collections::HashSet;
use std::sync::Arc;

use devo_core::AgentPromptIdentity;

use super::ServerRuntime;
use crate::runtime_session_summary::RuntimeSessionSummary;

impl ServerRuntime {
    /// Resolve actual parent edges instead of splitting display paths: agent
    /// nicknames may themselves contain `/`. Old contexts do not store identity.
    pub(crate) async fn agent_prompt_identity(
        self: &Arc<Self>,
        summary: &RuntimeSessionSummary,
    ) -> AgentPromptIdentity {
        let Some(mut parent_id) = summary.parent_session_id() else {
            return AgentPromptIdentity::Root;
        };
        let mut depth = 1u32;
        let mut parent = parent_id.to_string();
        let mut seen = HashSet::from([summary.session_id()]);
        while seen.insert(parent_id) {
            let Ok(handle) = self.get_or_load_parent_session(parent_id).await else {
                // Preserve child doctrine even for damaged/legacy lineage.
                break;
            };
            let Some(ancestor) = handle.summary().await else {
                break;
            };
            if depth == 1 {
                parent = ancestor.agent_path.clone().unwrap_or_else(|| {
                    if ancestor.parent_session_id().is_some() {
                        parent_id.to_string()
                    } else {
                        "root".to_string()
                    }
                });
            }
            let Some(next_parent) = ancestor.parent_session_id() else {
                break;
            };
            depth = depth.saturating_add(1);
            parent_id = next_parent;
        }
        AgentPromptIdentity::Child { depth, parent }
    }
}
