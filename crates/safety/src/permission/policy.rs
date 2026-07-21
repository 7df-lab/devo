use devo_config::PermissionConfig;

use super::PermissionAccess;
use super::PolicyCompileError;
use super::PolicyDecision;
use super::bash_command_splitting::all_commands_from_script;
use super::bash_command_splitting::is_external_shell_script_invocation;
use super::bash_command_splitting::shell_dash_c_script;
use super::bash_command_splitting::unwrap_wrappers;
use super::rules::CompiledRule;
use super::shell_access::UnresolvedSymlinkPolicy;
use super::shell_access::evaluate_native_file_access;
use super::shell_access::evaluate_shell_file_access;

/// Immutable, validated permission policy ready for runtime evaluation.
#[derive(Debug, Clone)]
pub struct CompiledPolicy {
    rules: Vec<CompiledRule>,
    has_file_restrictions: bool,
    has_bash_restrictions: bool,
}

impl CompiledPolicy {
    /// Validate and compile every configured matcher.
    pub fn compile(config: &PermissionConfig) -> Result<Self, PolicyCompileError> {
        let rules = config
            .rules
            .iter()
            .enumerate()
            .map(|(index, rule)| CompiledRule::compile(index, rule))
            .collect::<Result<Vec<_>, _>>()?;
        let has_file_restrictions = rules.iter().any(CompiledRule::is_file_restriction);
        let has_bash_restrictions = rules.iter().any(CompiledRule::is_bash_restriction);
        Ok(Self {
            rules,
            has_file_restrictions,
            has_bash_restrictions,
        })
    }

    /// Evaluate one Devo-native access request. Matching actions use fixed
    /// security precedence, independent of declaration order.
    pub fn evaluate(&self, access: &PermissionAccess) -> PolicyDecision {
        let direct = self
            .evaluate_rules(access)
            .combine(evaluate_native_file_access(
                self,
                access,
                if self.has_file_restrictions {
                    UnresolvedSymlinkPolicy::Ask
                } else {
                    UnresolvedSymlinkPolicy::Ignore
                },
            ));
        let PermissionAccess::Bash { command, cwd } = access else {
            return direct;
        };
        let bash = if self.has_bash_restrictions {
            self.evaluate_bash_segments(command, cwd, 0)
        } else {
            PolicyDecision::NoMatch
        };
        let files = if self.has_file_restrictions {
            evaluate_shell_file_access(self, command, cwd)
        } else {
            PolicyDecision::NoMatch
        };
        direct.combine(bash).combine(files)
    }

    pub(super) fn evaluate_rules(&self, access: &PermissionAccess) -> PolicyDecision {
        self.rules
            .iter()
            .filter(|rule| rule.matches(access))
            .map(CompiledRule::decision)
            .fold(PolicyDecision::NoMatch, PolicyDecision::combine)
    }

    pub(super) fn evaluate_recursive_scope(&self, access: &PermissionAccess) -> PolicyDecision {
        self.rules
            .iter()
            .map(|rule| rule.recursive_scope_decision(access))
            .fold(PolicyDecision::NoMatch, PolicyDecision::combine)
    }

    fn evaluate_bash_segments(
        &self,
        command: &str,
        cwd: &std::path::Path,
        depth: usize,
    ) -> PolicyDecision {
        const MAX_NESTING: usize = 8;
        if depth >= MAX_NESTING {
            return PolicyDecision::Ask;
        }
        let Some(commands) = all_commands_from_script(command) else {
            return PolicyDecision::Ask;
        };
        let mut decision = PolicyDecision::NoMatch;
        for parsed in commands {
            let raw = parsed.words();
            let inner = unwrap_wrappers(raw);
            decision = decision.combine(self.evaluate_bash_words(raw, cwd));
            if inner.len() != raw.len() {
                decision = decision.combine(self.evaluate_bash_words(inner, cwd));
            }
            if let Some(script) = shell_dash_c_script(inner) {
                decision = decision.combine(self.evaluate_bash_segments(script, cwd, depth + 1));
            } else if is_external_shell_script_invocation(inner) {
                decision = decision.combine(PolicyDecision::Ask);
            }
        }
        decision
    }

    fn evaluate_bash_words(&self, words: &[String], cwd: &std::path::Path) -> PolicyDecision {
        let raw = self
            .evaluate_rules(&PermissionAccess::Bash {
                command: words.join(" "),
                cwd: cwd.to_path_buf(),
            })
            .escalation_only();
        let Some((program, arguments)) = words.split_first() else {
            return raw;
        };
        let normalized_program = super::bash_command_splitting::basename(program);
        if normalized_program == program {
            return raw;
        }
        raw.combine(
            self.evaluate_rules(&PermissionAccess::Bash {
                command: std::iter::once(normalized_program)
                    .chain(arguments.iter().map(String::as_str))
                    .collect::<Vec<_>>()
                    .join(" "),
                cwd: cwd.to_path_buf(),
            })
            .escalation_only(),
        )
    }
}
