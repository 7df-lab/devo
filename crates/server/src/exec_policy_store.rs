//! User exec-policy rules loaded from the Devo home directory.

use std::path::PathBuf;

use devo_core::tools::load_exec_policy_from_devo_home;
use devo_execpolicy::{Decision, Policy};
use devo_util_paths::find_devo_home;

/// Loads merged exec-policy rules from `$DEVO_HOME/rules/*.rules`.
///
/// Returns `None` when the rules directory is missing or unreadable; callers
/// should treat that as "no user exec policy configured".
pub fn load_user_exec_policy() -> Option<Policy> {
    load_exec_policy_from_devo_home().ok()
}

/// Returns the default user rules file path under `$DEVO_HOME/rules/default.rules`.
pub fn default_user_rules_path() -> std::io::Result<PathBuf> {
    Ok(find_devo_home()?.join("rules").join("default.rules"))
}

/// Evaluates a parsed shell argv against the user exec policy.
pub fn exec_policy_decision_for_argv(policy: &Policy, argv: &[String]) -> Option<Decision> {
    let evaluation = policy.check(argv, &|_| Decision::Prompt);
    evaluation.is_match().then_some(evaluation.decision)
}

#[cfg(test)]
mod tests {
    use super::*;
    use devo_execpolicy::PolicyParser;
    use pretty_assertions::assert_eq;

    fn policy_with_git_status_allow() -> Policy {
        let mut parser = PolicyParser::new();
        parser
            .parse(
                "test.rules",
                r#"prefix_rule(pattern=["git", "status"], decision="allow")"#,
            )
            .expect("parse rules");
        parser.build()
    }

    #[test]
    fn exec_policy_decision_for_argv_returns_none_without_match() {
        let policy = policy_with_git_status_allow();
        assert_eq!(
            exec_policy_decision_for_argv(&policy, &["cargo".into(), "test".into()]),
            None
        );
    }

    #[test]
    fn exec_policy_decision_for_argv_returns_allow_on_match() {
        let policy = policy_with_git_status_allow();
        assert_eq!(
            exec_policy_decision_for_argv(&policy, &["git".into(), "status".into()]),
            Some(Decision::Allow)
        );
    }
}
