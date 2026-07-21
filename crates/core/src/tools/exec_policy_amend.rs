//! Prefix-rule amendment helpers for exec policy.

/// Prefixes that are too broad or dangerous to turn into persistent allow rules.
static BANNED_PREFIX_SUGGESTIONS: &[&[&str]] = &[
    &["python3"],
    &["python3", "-"],
    &["python3", "-c"],
    &["python"],
    &["python", "-"],
    &["python", "-c"],
    &["py"],
    &["py", "-3"],
    &["pythonw"],
    &["pyw"],
    &["pypy"],
    &["pypy3"],
    &["git"],
    &["bash"],
    &["bash", "-lc"],
    &["sh"],
    &["sh", "-c"],
    &["sh", "-lc"],
    &["zsh"],
    &["zsh", "-lc"],
    &["/bin/zsh"],
    &["/bin/zsh", "-lc"],
    &["/bin/bash"],
    &["/bin/bash", "-lc"],
    &["pwsh"],
    &["pwsh", "-Command"],
    &["pwsh", "-c"],
    &["powershell"],
    &["powershell", "-Command"],
    &["powershell", "-c"],
    &["powershell.exe"],
    &["powershell.exe", "-Command"],
    &["powershell.exe", "-c"],
    &["env"],
    &["sudo"],
    &["node"],
    &["node", "-e"],
    &["perl"],
    &["perl", "-e"],
    &["ruby"],
    &["ruby", "-e"],
    &["php"],
    &["php", "-r"],
    &["lua"],
    &["lua", "-e"],
    &["osascript"],
    &["rm"],
    &["dd"],
    &["mkfs"],
    &["shred"],
];

/// Returns true when a model-suggested prefix rule is too broad to auto-approve.
pub fn is_banned_prefix_suggestion(prefix_rule: &[String]) -> bool {
    if prefix_rule.is_empty() {
        return true;
    }
    BANNED_PREFIX_SUGGESTIONS.iter().any(|banned| {
        prefix_rule.len() == banned.len()
            && prefix_rule
                .iter()
                .map(String::as_str)
                .eq(banned.iter().copied())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn rejects_banned_interpreter_prefixes() {
        assert!(is_banned_prefix_suggestion(&[
            "python3".to_string(),
            "-c".to_string()
        ]));
        assert!(is_banned_prefix_suggestion(&[
            "bash".to_string(),
            "-lc".to_string()
        ]));
        assert!(is_banned_prefix_suggestion(&["rm".to_string()]));
    }

    #[test]
    fn allows_specific_command_prefixes() {
        assert!(!is_banned_prefix_suggestion(&[
            "cargo".to_string(),
            "test".to_string()
        ]));
        assert!(!is_banned_prefix_suggestion(&[
            "git".to_string(),
            "status".to_string()
        ]));
    }

    #[test]
    fn empty_prefix_is_banned() {
        assert!(is_banned_prefix_suggestion(&[]));
    }

    #[test]
    fn exact_banned_match_only() {
        assert!(!is_banned_prefix_suggestion(&[
            "git".to_string(),
            "status".to_string()
        ]));
        assert_eq!(is_banned_prefix_suggestion(&["git".to_string()]), true);
    }
}
