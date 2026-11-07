use std::collections::HashSet;

use devo_core::AgentPromptIdentity;

use super::ServerRuntime;
use crate::runtime_session_summary::RuntimeSessionSummary;

impl ServerRuntime {
    /// Resolve actual parent edges instead of splitting display paths: agent
    /// nicknames may themselves contain `/`. Do not load actors here: loading
    /// can schedule recovery work and must not become part of query admission.
    pub(crate) async fn agent_prompt_identity(
        &self,
        summary: &RuntimeSessionSummary,
    ) -> AgentPromptIdentity {
        let Some(mut parent_id) = summary.parent_session_id() else {
            return AgentPromptIdentity::Root;
        };
        let mut depth = 1u32;
        let mut parent = parent_id.to_string();
        let mut seen = HashSet::from([summary.session_id()]);
        while seen.insert(parent_id) {
            let live = match self.session(parent_id).await {
                Some(handle) => handle.summary().await,
                None => None,
            };
            let (next_parent, path) = if let Some(ancestor) = live {
                (ancestor.parent_session_id(), ancestor.agent_path)
            } else if let Ok(Some(index)) = self.deps.db.get_session_index(&parent_id) {
                // The normal index has an agent path. For older entries without
                // one, raw Native lineage distinguishes an agent from a fork.
                let next_parent = if index.session.agent_path.is_some() {
                    index.session.parent_session_id
                } else {
                    index.rollout_path.as_deref().and_then(|path| {
                        self.rollout_store
                            .raw_session_parent(path, &parent_id)
                            .ok()
                            .flatten()
                            .flatten()
                    })
                };
                (next_parent, index.session.agent_path)
            } else {
                // Preserve child doctrine even for damaged/legacy lineage.
                break;
            };
            if depth == 1 {
                parent = path.unwrap_or_else(|| {
                    if next_parent.is_some() {
                        parent_id.to_string()
                    } else {
                        "root".to_string()
                    }
                });
            }
            let Some(next_parent) = next_parent else {
                break;
            };
            depth = depth.saturating_add(1);
            parent_id = next_parent;
        }
        AgentPromptIdentity::Child { depth, parent }
    }
}
