//! Subagent coordination — spawn, lifecycle, mailboxes.
//!
//! Implements L3-BEH-SERVER-003. Provides AgentRegistry, agent tree,
//! inter-agent mailbox channels, and spawn/close tool handlers.

use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use chrono::{DateTime, Utc};
use devo_protocol::AgentInfo;
use devo_protocol::AgentMailboxMessage;
use devo_protocol::AgentOutputEvent;
use devo_protocol::AgentOutputEventKind;
use devo_protocol::SessionId;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

// ── Agent Registry ──────────────────────────────────────────────────

/// Per-root-session registry of all spawned subagents.
///
/// The registry models a parent/child tree: any registered agent may in principle
/// spawn further children, and [`parent_to_children`](Self::parent_to_children) /
/// [`child_to_parent`](Self::child_to_parent) track that hierarchy for arbitrary depth.
///
/// In current product usage, only the root session agent is expected to coordinate
/// subagents; child sessions do not receive agent coordination tools and must not
/// spawn nested subagents. That restriction is enforced at runtime (tool policy and
/// prompts), not by flattening this data structure. The tree-shaped registry is kept
/// intentionally so deeper nesting can be enabled later without redesigning storage
/// or lookup.
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

    pub fn update_status(&mut self, child_id: SessionId, status: SubagentStatus) {
        if let Some(agent) = self.agents.get_mut(&child_id) {
            agent.status = status;
            if matches!(
                status,
                SubagentStatus::Completed
                    | SubagentStatus::Failed
                    | SubagentStatus::Interrupted
                    | SubagentStatus::Canceled
                    | SubagentStatus::Closed
            ) {
                agent.closed_at.get_or_insert_with(Utc::now);
            }
        }
    }

    pub fn find_child(&self, parent_id: SessionId, target: &str) -> Option<SessionId> {
        let target = target.trim();
        if let Ok(session_id) = target.parse::<SessionId>()
            && self.agents.contains_key(&session_id)
        {
            return Some(session_id);
        }
        self.children_of(parent_id).into_iter().find(|child_id| {
            self.agents.get(child_id).is_some_and(|meta| {
                meta.agent_path == target
                    || meta.nickname == target
                    || meta.session_id.to_string() == target
            })
        })
    }

    pub fn list_children(&self, parent_id: SessionId, path_prefix: Option<&str>) -> Vec<AgentInfo> {
        self.children_of(parent_id)
            .into_iter()
            .filter_map(|child_id| self.agents.get(&child_id))
            .filter(|meta| path_prefix.is_none_or(|prefix| meta.agent_path.starts_with(prefix)))
            .map(SubagentMetadata::to_agent_info)
            .collect()
    }
}

/// Per-subagent metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentMetadata {
    pub session_id: SessionId,
    pub parent_session_id: SessionId,
    pub agent_path: String,
    pub nickname: String,
    pub role: String,
    pub status: SubagentStatus,
    pub spawned_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub last_task_message: Option<String>,
    pub close_requested: bool,
}

impl SubagentMetadata {
    pub fn to_agent_info(&self) -> AgentInfo {
        AgentInfo {
            session_id: self.session_id,
            parent_session_id: Some(self.parent_session_id),
            agent_path: self.agent_path.clone(),
            agent_nickname: self.nickname.clone(),
            agent_role: self.role.clone(),
            status: self.status.as_str().to_string(),
            last_task_message: self.last_task_message.clone(),
        }
    }
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
    Closed,
}

impl SubagentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spawning => "spawning",
            Self::Running => "running",
            Self::WaitingForInput => "waiting_for_input",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Canceled => "canceled",
            Self::Closed => "closed",
        }
    }
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

/// Mailbox queue for ordered inter-agent communication.
#[derive(Debug, Clone)]
pub struct SubagentMailbox {
    inner: Arc<Mutex<MailboxInner>>,
    notify: Arc<Notify>,
}

#[derive(Debug, Default)]
struct MailboxInner {
    next_sequence: u64,
    pending: VecDeque<AgentMailboxMessage>,
}

impl Default for SubagentMailbox {
    fn default() -> Self {
        Self::new()
    }
}

impl SubagentMailbox {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(MailboxInner {
                next_sequence: 1,
                pending: VecDeque::new(),
            })),
            notify: Arc::new(Notify::new()),
        }
    }

    pub async fn send(
        &self,
        mut msg: AgentMailboxMessage,
    ) -> Result<AgentMailboxMessage, SubagentError> {
        let mut inner = self.inner.lock().await;
        msg.sequence = inner.next_sequence;
        if msg.message_id.is_empty() {
            msg.message_id = format!("mail-{}", inner.next_sequence);
        }
        inner.next_sequence = inner.next_sequence.saturating_add(1);
        inner.pending.push_back(msg.clone());
        drop(inner);
        self.notify.notify_waiters();
        Ok(msg)
    }

    pub async fn drain(&self) -> Vec<AgentMailboxMessage> {
        let mut inner = self.inner.lock().await;
        inner.pending.drain(..).collect()
    }

    pub async fn wait(&self, timeout: Duration) -> (Vec<AgentMailboxMessage>, bool) {
        let messages = self.drain().await;
        if !messages.is_empty() {
            return (messages, false);
        }
        if tokio::time::timeout(timeout, self.notify.notified())
            .await
            .is_err()
        {
            return (Vec::new(), true);
        }
        (self.drain().await, false)
    }
}

// ── Parent Output Buffer ────────────────────────────────────────────

/// Per-parent ordered buffer of child assistant output and status changes.
#[derive(Debug, Clone)]
pub struct SubagentOutputBuffer {
    inner: Arc<Mutex<OutputBufferInner>>,
    notify: Arc<Notify>,
}

#[derive(Debug, Default)]
struct OutputBufferInner {
    next_sequence: u64,
    events: VecDeque<AgentOutputEvent>,
}

impl Default for SubagentOutputBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl SubagentOutputBuffer {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(OutputBufferInner {
                next_sequence: 1,
                events: VecDeque::new(),
            })),
            notify: Arc::new(Notify::new()),
        }
    }

    pub async fn push(&self, mut event: AgentOutputEvent) -> AgentOutputEvent {
        let mut inner = self.inner.lock().await;
        if event.kind.is_assistant_text() {
            event.kind = AgentOutputEventKind::AssistantMessage;
            if let Some(last) = inner.events.back_mut()
                && last.kind == AgentOutputEventKind::AssistantMessage
                && last.child_session_id == event.child_session_id
                && last.turn_id == event.turn_id
                && last.status.is_none()
            {
                if let Some(delta) = event.text.take() {
                    last.text.get_or_insert_with(String::new).push_str(&delta);
                }
                last.created_at = event.created_at;
                // Release the lock before any clone/notify work. Callers currently
                // discard the return value; avoid cloning the full accumulated
                // assistant text on every token while holding the buffer mutex
                // (multiple child event streams contend on this lock).
                drop(inner);
                self.notify.notify_waiters();
                return event;
            }
        }
        event.sequence = inner.next_sequence;
        inner.next_sequence = inner.next_sequence.saturating_add(1);
        inner.events.push_back(event.clone());
        drop(inner);
        self.notify.notify_waiters();
        event
    }

    /// Non-blocking variant for streaming text. Returns `false` when the buffer
    /// lock is busy so token broadcast is never stalled behind `wait_agent`.
    pub fn try_push_text_delta(&self, mut event: AgentOutputEvent) -> bool {
        let Ok(mut inner) = self.inner.try_lock() else {
            return false;
        };
        event.kind = AgentOutputEventKind::AssistantMessage;
        if let Some(last) = inner.events.back_mut()
            && last.kind == AgentOutputEventKind::AssistantMessage
            && last.child_session_id == event.child_session_id
            && last.turn_id == event.turn_id
            && last.status.is_none()
        {
            if let Some(delta) = event.text.take() {
                last.text.get_or_insert_with(String::new).push_str(&delta);
            }
            last.created_at = event.created_at;
            drop(inner);
            self.notify.notify_waiters();
            return true;
        }
        event.sequence = inner.next_sequence;
        inner.next_sequence = inner.next_sequence.saturating_add(1);
        inner.events.push_back(event);
        drop(inner);
        self.notify.notify_waiters();
        true
    }

    pub async fn wait_after(
        &self,
        after_sequence: u64,
        target_session_ids: &[SessionId],
        timeout: Duration,
        cancel: Option<CancellationToken>,
    ) -> (Vec<AgentOutputEvent>, u64, bool) {
        if target_session_ids.is_empty() {
            let inner = self.inner.lock().await;
            return (Vec::new(), inner.next_sequence, true);
        }
        let target_session_ids = target_session_ids.iter().copied().collect::<HashSet<_>>();
        let start = Instant::now();
        loop {
            if cancel.as_ref().is_some_and(CancellationToken::is_cancelled) {
                let (_, next_sequence) =
                    self.events_after(after_sequence, &target_session_ids).await;
                return (Vec::new(), next_sequence, true);
            }
            let notified = self.notify.notified();
            let (events, next_sequence) =
                self.events_after(after_sequence, &target_session_ids).await;
            if !events.is_empty() {
                return (events, next_sequence, false);
            }
            let elapsed = start.elapsed();
            if elapsed >= timeout {
                return (Vec::new(), next_sequence, true);
            }
            let remaining = timeout.saturating_sub(elapsed);
            let sleep = tokio::time::sleep(remaining);
            tokio::pin!(sleep);
            tokio::select! {
                _ = &mut sleep => {
                    let (_, next_sequence) =
                        self.events_after(after_sequence, &target_session_ids).await;
                    return (Vec::new(), next_sequence, true);
                }
                _ = notified => {}
                _ = async {
                    if let Some(token) = &cancel {
                        token.cancelled().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    let (_, next_sequence) =
                        self.events_after(after_sequence, &target_session_ids).await;
                    return (Vec::new(), next_sequence, true);
                }
            }
        }
    }

    async fn events_after(
        &self,
        after_sequence: u64,
        target_session_ids: &HashSet<SessionId>,
    ) -> (Vec<AgentOutputEvent>, u64) {
        let inner = self.inner.lock().await;
        let events = inner
            .events
            .iter()
            .filter(|event| event.sequence > after_sequence)
            .filter(|event| target_session_ids.contains(&event.child_session_id))
            .cloned()
            .collect();
        (events, inner.next_sequence)
    }
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
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn output_buffer_accumulates_assistant_deltas_per_turn() {
        let buffer = SubagentOutputBuffer::new();
        let child = SessionId::new();
        let turn_id = devo_protocol::TurnId::new();
        let base = || AgentOutputEvent {
            sequence: 0,
            child_session_id: child,
            agent_path: "root/worker".into(),
            turn_id: Some(turn_id),
            kind: AgentOutputEventKind::AssistantDelta,
            text: None,
            status: None,
            created_at: Utc::now(),
        };

        let first = buffer
            .push(AgentOutputEvent {
                text: Some("alpha ".into()),
                ..base()
            })
            .await;
        assert_eq!(first.sequence, 1);
        assert_eq!(first.text.as_deref(), Some("alpha "));
        // Coalesced deltas no longer return a full snapshot (callers discard it);
        // accumulated text is observed through wait_after / buffer reads.
        let _ = buffer
            .push(AgentOutputEvent {
                text: Some("beta".into()),
                ..base()
            })
            .await;

        let (events, next_sequence, timed_out) = buffer
            .wait_after(0, &[child], Duration::from_millis(1), None)
            .await;
        assert!(!timed_out);
        assert_eq!(next_sequence, 2);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].sequence, 1);
        assert_eq!(events[0].text.as_deref(), Some("alpha beta"));
        assert_eq!(events[0].kind, AgentOutputEventKind::AssistantMessage);
    }

    #[tokio::test]
    async fn wait_after_empty_targets_returns_immediately() {
        let buffer = SubagentOutputBuffer::new();
        let child = SessionId::new();
        buffer
            .push(AgentOutputEvent {
                sequence: 0,
                child_session_id: child,
                agent_path: "root/worker".into(),
                turn_id: None,
                kind: AgentOutputEventKind::Status,
                text: None,
                status: Some("running".into()),
                created_at: Utc::now(),
            })
            .await;

        let (events, next_sequence, timed_out) = buffer
            .wait_after(0, &[], Duration::from_secs(60), None)
            .await;
        assert!(timed_out);
        assert!(events.is_empty());
        assert_eq!(next_sequence, 2);
    }

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
            last_task_message: Some("review this".into()),
            close_requested: false,
        };
        registry.register(parent, child, meta.clone());
        assert_eq!(registry.get(child), Some(&meta));
        assert_eq!(registry.children_of(parent), vec![child]);
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
                last_task_message: None,
                close_requested: false,
            },
        );
        registry.unregister(child);
        assert!(registry.get(child).is_none());
        assert!(registry.children_of(parent).is_empty());
    }

    #[tokio::test]
    async fn mailbox_drains_waits_and_keeps_late_messages() {
        let mailbox = SubagentMailbox::new();
        let parent = SessionId::new();
        let child = SessionId::new();
        let message = AgentMailboxMessage {
            message_id: String::new(),
            from_session_id: child,
            to_session_id: parent,
            from_agent_path: "root/test".into(),
            to_agent_path: "root".into(),
            content: "done".into(),
            sequence: 0,
            created_at: Utc::now(),
        };
        let delivered = mailbox.send(message.clone()).await.expect("send message");
        let messages = mailbox.drain().await;
        assert_eq!(messages, vec![delivered]);

        let (messages, timed_out) = mailbox.wait(Duration::from_millis(1)).await;
        assert!(messages.is_empty());
        assert!(timed_out);

        let mailbox_for_task = mailbox.clone();
        let expected = message;
        let task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            mailbox_for_task
                .send(expected)
                .await
                .expect("send delayed message");
        });
        let (messages, timed_out) = mailbox.wait(Duration::from_millis(100)).await;
        task.await.expect("delayed send joins");
        assert!(!timed_out);
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn agent_path_join_and_parent() {
        let root = AgentPath::new("root");
        let child = root.join("code-reviewer");
        assert_eq!(child.as_str(), "root/code-reviewer");
        assert_eq!(child.parent().unwrap().as_str(), "root");
    }

    #[tokio::test]
    async fn mailbox_send_receive() {
        let mailbox = SubagentMailbox::new();
        let msg = AgentMailboxMessage {
            message_id: "msg-1".into(),
            from_session_id: SessionId::new(),
            to_session_id: SessionId::new(),
            from_agent_path: "root/child".into(),
            to_agent_path: "root".into(),
            content: "hello".into(),
            sequence: 0,
            created_at: Utc::now(),
        };
        let delivered = mailbox.send(msg).await.expect("send");
        assert_eq!(mailbox.drain().await, vec![delivered]);
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
            SubagentStatus::Closed,
        ] {
            let json = serde_json::to_string(status).expect("serialize");
            let restored: SubagentStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *status);
        }
    }
}
