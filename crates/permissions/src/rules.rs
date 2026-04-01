use crate::{
    PermissionDecision, PermissionMode, PermissionPolicy, PermissionRequest, ResourceKind,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A single path/command allow-rule persisted in configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRule {
    pub resource: ResourceKind,
    /// Glob or prefix that the target must match.
    pub pattern: String,
    pub allow: bool,
}

/// A rule-based permission policy.
///
/// 1. If an explicit rule matches, use it.
/// 2. Otherwise fall back to the configured [`PermissionMode`].
pub struct RuleBasedPolicy {
    pub mode: PermissionMode,
    pub rules: Vec<PermissionRule>,
}

impl RuleBasedPolicy {
    pub fn new(mode: PermissionMode) -> Self {
        Self {
            mode,
            rules: Vec::new(),
        }
    }

    pub fn with_rules(mode: PermissionMode, rules: Vec<PermissionRule>) -> Self {
        Self { mode, rules }
    }

    fn match_rule(&self, request: &PermissionRequest) -> Option<&PermissionRule> {
        let target = request.target.as_deref().unwrap_or("");
        self.rules.iter().find(|rule| {
            rule.resource == request.resource && Self::pattern_matches(&rule.pattern, target)
        })
    }

    fn pattern_matches(pattern: &str, target: &str) -> bool {
        if pattern == "*" {
            return true;
        }
        if pattern.ends_with('*') {
            return target.starts_with(pattern.trim_end_matches('*'));
        }
        target == pattern
    }
}

#[async_trait]
impl PermissionPolicy for RuleBasedPolicy {
    async fn check(&self, request: &PermissionRequest) -> PermissionDecision {
        if let Some(rule) = self.match_rule(request) {
            return if rule.allow {
                PermissionDecision::Allow
            } else {
                PermissionDecision::Deny {
                    reason: format!("blocked by rule: {}", rule.pattern),
                }
            };
        }

        match self.mode {
            PermissionMode::AutoApprove => PermissionDecision::Allow,
            PermissionMode::Deny => PermissionDecision::Deny {
                reason: "permission mode is Deny".into(),
            },
            PermissionMode::Interactive => PermissionDecision::Ask {
                message: format!(
                    "{} wants to access {:?}: {}",
                    request.tool_name, request.resource, request.description
                ),
            },
        }
    }
}
