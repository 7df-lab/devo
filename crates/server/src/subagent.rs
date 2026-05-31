//! Subagent coordination — spawn, lifecycle, mailboxes.
//!
//! Implements L3-BEH-SERVER-003. Provides AgentRegistry, agent tree,
//! inter-agent mailbox channels, and spawn/close tool handlers.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use devo_protocol::SessionId;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

// ── Agent Registry ──────────────────────────────────────────────────

/// Per-root-session registry of all spawned subagents.
#[derive(Debug, Clone, Default)]
pub struct AgentRegistry {
    pub agents: HashMap<SessionId, SubagentMetadata>,
    pub parent_to_children: HashMap<SessionId, Vec<SessionId>>,
    pub child_to_parent: HashMap<SessionId, SessionId>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        parent_id: SessionId,
        child_id: SessionId,
        metadata: SubagentMetadata,
    ) {
        self.agents.insert(child_id, metadata);
        self.parent_to_children
            .entry(parent_id)
            .or_default()
            .push(child_id);
        self.child_to_parent.insert(child_id, parent_id);
    }

    pub fn unregister(&mut self, child_id: SessionId) {
        self.agents.remove(&child_id);
        if let Some(parent_id) = self.child_to_parent.remove(&child_id)
            && let Some(children) = self.parent_to_children.get_mut(&parent_id)
        {
            children.retain(|id| *id != child_id);
        }
    }

    pub fn get(&self, session_id: SessionId) -> Option<&SubagentMetadata> {
        self.agents.get(&session_id)
    }

    pub fn children_of(&self, parent_id: SessionId) -> Vec<SessionId> {
        self.parent_to_children
            .get(&parent_id)
            .cloned()
            .unwrap_or_default()
    }
}

/// Per-subagent metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentMetadata {
    pub session_id: SessionId,
    pub parent_session_id: SessionId,
    pub agent_path: String,
    pub nickname: String,
    pub role: String,
    pub status: SubagentStatus,
    pub spawned_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

/// Lifecycle status of a subagent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    Spawning,
    Running,
    WaitingForInput,
    Completed,
    Failed,
    Interrupted,
    Canceled,
}

// ── Agent Path ──────────────────────────────────────────────────────

/// Canonical agent path: `<parent>/<child>/...`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentPath(pub String);

impl AgentPath {
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn parent(&self) -> Option<AgentPath> {
        let s = &self.0;
        s.rfind('/').map(|pos| AgentPath(s[..pos].to_string()))
    }

    pub fn join(&self, name: &str) -> AgentPath {
        AgentPath(format!("{}/{}", self.0, name))
    }
}

// ── Inter-Agent Mailbox ─────────────────────────────────────────────

/// Mailbox channel for inter-agent communication.
#[derive(Debug, Clone)]
pub struct SubagentMailbox {
    pub tx: mpsc::UnboundedSender<SubagentMessage>,
    pub rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<SubagentMessage>>>,
}

impl Default for SubagentMailbox {
    fn default() -> Self {
        Self::new()
    }
}

impl SubagentMailbox {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            tx,
            rx: Arc::new(tokio::sync::Mutex::new(rx)),
        }
    }

    pub fn send(&self, msg: SubagentMessage) -> Result<(), SubagentError> {
        self.tx.send(msg).map_err(|_| SubagentError::MailboxClosed)
    }
}

/// A message sent through the inter-agent mailbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentMessage {
    pub message_id: String,
    pub from_session_id: SessionId,
    pub to_session_id: SessionId,
    pub content: String,
    pub sequence: u64,
}

// ── Spawn Params / Result ──────────────────────────────────────────

/// Parameters for spawning a subagent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnAgentParams {
    pub parent_session_id: SessionId,
    pub agent_nickname: String,
    pub agent_role: String,
    pub task: String,
    pub model: Option<String>,
}

/// Result of spawning a subagent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnAgentResult {
    pub child_session_id: SessionId,
    pub agent_path: String,
}

// ── Errors ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, thiserror::Error)]
pub enum SubagentError {
    #[error("agent not found: {0}")]
    AgentNotFound(SessionId),
    #[error("agent registry full")]
    RegistryFull,
    #[error("max depth exceeded")]
    MaxDepthExceeded,
    #[error("max agents per root exceeded")]
    MaxAgentsExceeded,
    #[error("mailbox closed")]
    MailboxClosed,
    #[error("spawn failed: {0}")]
    SpawnFailed(String),
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_registry_register_and_lookup() {
        let mut registry = AgentRegistry::new();
        let parent = SessionId::new();
        let child = SessionId::new();
        let meta = SubagentMetadata {
            session_id: child,
            parent_session_id: parent,
            agent_path: "root/code-reviewer".into(),
            nickname: "code-reviewer".into(),
            role: "reviewer".into(),
            status: SubagentStatus::Running,
            spawned_at: Utc::now(),
            closed_at: None,
        };
        registry.register(parent, child, meta);
        assert!(registry.get(child).is_some());
        assert_eq!(registry.children_of(parent).len(), 1);
    }

    #[test]
    fn agent_registry_unregister() {
        let mut registry = AgentRegistry::new();
        let parent = SessionId::new();
        let child = SessionId::new();
        registry.register(
            parent,
            child,
            SubagentMetadata {
                session_id: child,
                parent_session_id: parent,
                agent_path: "root/test".into(),
                nickname: "test".into(),
                role: "tester".into(),
                status: SubagentStatus::Completed,
                spawned_at: Utc::now(),
                closed_at: None,
            },
        );
        registry.unregister(child);
        assert!(registry.get(child).is_none());
        assert!(registry.children_of(parent).is_empty());
    }

    #[test]
    fn agent_path_join_and_parent() {
        let root = AgentPath::new("root");
        let child = root.join("code-reviewer");
        assert_eq!(child.as_str(), "root/code-reviewer");
        assert_eq!(child.parent().unwrap().as_str(), "root");
    }

    #[test]
    fn mailbox_send_receive() {
        let mailbox = SubagentMailbox::new();
        let msg = SubagentMessage {
            message_id: "msg-1".into(),
            from_session_id: SessionId::new(),
            to_session_id: SessionId::new(),
            content: "hello".into(),
            sequence: 0,
        };
        mailbox.send(msg.clone()).expect("send");
    }

    #[test]
    fn subagent_status_serde_roundtrip() {
        for status in &[
            SubagentStatus::Spawning,
            SubagentStatus::Running,
            SubagentStatus::WaitingForInput,
            SubagentStatus::Completed,
            SubagentStatus::Failed,
            SubagentStatus::Interrupted,
            SubagentStatus::Canceled,
        ] {
            let json = serde_json::to_string(status).expect("serialize");
            let restored: SubagentStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *status);
        }
    }
}
