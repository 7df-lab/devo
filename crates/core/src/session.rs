use std::path::PathBuf;

use claw_compact::TokenBudget;
use claw_permissions::PermissionMode;

use crate::Message;

/// Configuration for a session.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub model: String,
    pub system_prompt: String,
    pub max_turns: usize,
    pub token_budget: TokenBudget,
    pub permission_mode: PermissionMode,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            model: "claude-sonnet-4-20250514".to_string(),
            system_prompt: String::new(),
            max_turns: 100,
            token_budget: TokenBudget::default(),
            permission_mode: PermissionMode::AutoApprove,
        }
    }
}

/// Mutable state for one conversation session.
///
/// This corresponds to the session-level state in Claude Code's
/// `AppStateStore` and `QueryEngine`, but stripped of UI concerns.
pub struct SessionState {
    pub id: String,
    pub config: SessionConfig,
    pub messages: Vec<Message>,
    pub cwd: PathBuf,
    pub turn_count: usize,
    pub total_input_tokens: usize,
    pub total_output_tokens: usize,
}

impl SessionState {
    pub fn new(config: SessionConfig, cwd: PathBuf) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            config,
            messages: Vec::new(),
            cwd,
            turn_count: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
        }
    }

    pub fn push_message(&mut self, msg: Message) {
        self.messages.push(msg);
    }

    pub fn to_request_messages(&self) -> Vec<claw_provider::RequestMessage> {
        self.messages
            .iter()
            .map(|m| m.to_request_message())
            .collect()
    }
}
