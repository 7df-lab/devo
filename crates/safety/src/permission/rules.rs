use std::path::Path;

use devo_config::PatternMode;
use devo_config::PermissionRule;
use devo_config::RuleAction;
use devo_config::ToolFilter;
use globset::GlobBuilder;
use globset::GlobMatcher;
use thiserror::Error;
use url::Url;

use super::PermissionAccess;
use super::PolicyDecision;

/// A configuration error found while compiling permission matchers.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PolicyCompileError {
    #[error("invalid glob in permission rule {rule_index} ({pattern:?}): {message}")]
    InvalidGlob {
        rule_index: usize,
        pattern: String,
        message: String,
    },
    #[error("invalid domain in permission rule {rule_index} ({pattern:?}): {message}")]
    InvalidDomain {
        rule_index: usize,
        pattern: String,
        message: String,
    },
}

#[derive(Debug, Clone)]
struct CompiledGlob {
    path: GlobMatcher,
    freeform: GlobMatcher,
}

#[derive(Debug, Clone)]
pub(super) struct CompiledRule {
    action: RuleAction,
    tool: ToolFilter,
    pattern: Option<String>,
    pattern_mode: PatternMode,
    glob: Option<CompiledGlob>,
    domain: Option<String>,
}

impl CompiledRule {
    pub(super) fn compile(
        rule_index: usize,
        rule: &PermissionRule,
    ) -> Result<Self, PolicyCompileError> {
        let mut glob = None;
        let mut domain = None;
        if let Some(pattern) = rule.pattern.as_deref() {
            if matches!(rule.pattern_mode, PatternMode::Domain) {
                domain = Some(normalize_domain_pattern(rule_index, pattern)?);
            }
            if pattern != "*" {
                glob = Some(compile_glob(rule_index, pattern)?);
            }
        }
        Ok(Self {
            action: rule.action,
            tool: rule.tool,
            pattern: rule.pattern.clone(),
            pattern_mode: rule.pattern_mode,
            glob,
            domain,
        })
    }

    pub(super) fn is_file_restriction(&self) -> bool {
        matches!(self.action, RuleAction::Deny | RuleAction::Ask)
            && matches!(
                self.tool,
                ToolFilter::Any | ToolFilter::Read | ToolFilter::Edit | ToolFilter::Grep
            )
    }

    pub(super) fn is_bash_restriction(&self) -> bool {
        matches!(self.action, RuleAction::Deny | RuleAction::Ask)
            && matches!(self.tool, ToolFilter::Any | ToolFilter::Bash)
    }

    pub(super) fn matches(&self, access: &PermissionAccess) -> bool {
        tool_filter_matches(access, self.tool) && self.pattern_matches(access)
    }

    pub(super) fn decision(&self) -> PolicyDecision {
        match self.action {
            RuleAction::Allow => PolicyDecision::Allow,
            RuleAction::Ask => PolicyDecision::Ask,
            RuleAction::Deny => PolicyDecision::Deny {
                reason: self.deny_reason(),
            },
        }
    }

    fn deny_reason(&self) -> String {
        let label = tool_label(self.tool);
        match self.pattern.as_deref() {
            Some(pattern) => {
                format!("Denied by permission policy: deny rule on {label} matching \"{pattern}\"")
            }
            None => format!("Denied by permission policy: deny rule on {label}"),
        }
    }

    fn pattern_matches(&self, access: &PermissionAccess) -> bool {
        let Some(pattern) = self.pattern.as_deref() else {
            return true;
        };
        if pattern == "*" {
            return true;
        }
        match access {
            PermissionAccess::Read {
                path: Some(path),
                cwd: _,
            }
            | PermissionAccess::Edit { path, cwd: _ } => self.path_matches(path),
            PermissionAccess::Read { path: None, cwd: _ } => false,
            PermissionAccess::Grep {
                path,
                glob: _,
                cwd: _,
                recursive: _,
            } => path.as_deref().is_some_and(|path| self.path_matches(path)),
            PermissionAccess::Bash { command, cwd: _ } => {
                let command = command.trim_start();
                command.starts_with(pattern) || self.freeform_matches(command)
            }
            PermissionAccess::Mcp { name, input: _ } => self.freeform_matches(name),
            PermissionAccess::WebFetch(url) => match self.pattern_mode {
                PatternMode::Domain => self.domain_matches(url),
                PatternMode::Glob => self.freeform_matches(url),
            },
            PermissionAccess::WebSearch(query) => {
                query.starts_with(pattern) || self.freeform_matches(query)
            }
        }
    }

    pub(super) fn recursive_scope_decision(&self, access: &PermissionAccess) -> PolicyDecision {
        if matches!(self.action, RuleAction::Allow) || !tool_filter_matches(access, self.tool) {
            return PolicyDecision::NoMatch;
        }
        self.decision()
    }

    fn path_matches(&self, path: &Path) -> bool {
        self.glob
            .as_ref()
            .is_some_and(|glob| glob.path.is_match(normalize_path(path)))
    }

    fn freeform_matches(&self, value: &str) -> bool {
        self.glob
            .as_ref()
            .is_some_and(|glob| glob.freeform.is_match(value))
    }

    fn domain_matches(&self, value: &str) -> bool {
        let Some(expected) = self.domain.as_deref() else {
            return false;
        };
        let Some(actual) = normalize_url_host(value) else {
            return false;
        };
        actual == expected
            || (!is_ip_host(expected)
                && actual
                    .strip_suffix(expected)
                    .is_some_and(|prefix| prefix.ends_with('.')))
    }
}

fn compile_glob(rule_index: usize, pattern: &str) -> Result<CompiledGlob, PolicyCompileError> {
    const PATH_SEPARATOR_IS_LITERAL: bool = true;
    const FREEFORM_SEPARATOR_IS_LITERAL: bool = false;
    let build = |literal_separator| {
        let mut builder = GlobBuilder::new(pattern);
        builder.literal_separator(literal_separator);
        builder.build().map(|glob| glob.compile_matcher())
    };
    let path = build(PATH_SEPARATOR_IS_LITERAL)
        .map_err(|error| invalid_glob(rule_index, pattern, error.to_string()))?;
    let freeform = build(FREEFORM_SEPARATOR_IS_LITERAL)
        .map_err(|error| invalid_glob(rule_index, pattern, error.to_string()))?;
    Ok(CompiledGlob { path, freeform })
}

fn invalid_glob(rule_index: usize, pattern: &str, message: String) -> PolicyCompileError {
    let prefix = format!("error parsing glob '{pattern}': ");
    PolicyCompileError::InvalidGlob {
        rule_index,
        pattern: pattern.to_string(),
        message: message
            .strip_prefix(&prefix)
            .unwrap_or(&message)
            .to_string(),
    }
}

fn normalize_domain_pattern(
    rule_index: usize,
    pattern: &str,
) -> Result<String, PolicyCompileError> {
    if pattern.contains(['*', '?', '[', ']']) {
        return Err(PolicyCompileError::InvalidDomain {
            rule_index,
            pattern: pattern.to_string(),
            message: "domain patterns do not support wildcards".to_string(),
        });
    }
    let candidate = if pattern.contains("://") {
        pattern.to_string()
    } else {
        format!("http://{pattern}")
    };
    let parsed = Url::parse(&candidate).map_err(|error| PolicyCompileError::InvalidDomain {
        rule_index,
        pattern: pattern.to_string(),
        message: error.to_string(),
    })?;
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || !matches!(parsed.path(), "" | "/")
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(PolicyCompileError::InvalidDomain {
            rule_index,
            pattern: pattern.to_string(),
            message: "domain pattern must not include credentials, a path, query, or fragment"
                .to_string(),
        });
    }
    normalize_host(parsed.host_str()).ok_or_else(|| PolicyCompileError::InvalidDomain {
        rule_index,
        pattern: pattern.to_string(),
        message: "domain pattern has no host".to_string(),
    })
}

fn normalize_url_host(value: &str) -> Option<String> {
    let parsed = Url::parse(value).ok()?;
    normalize_host(parsed.host_str())
}

fn normalize_host(host: Option<&str>) -> Option<String> {
    let normalized = host?.trim_end_matches('.').to_ascii_lowercase();
    (!normalized.is_empty()).then_some(normalized)
}

fn is_ip_host(host: &str) -> bool {
    host.trim_matches(['[', ']'])
        .parse::<std::net::IpAddr>()
        .is_ok()
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn tool_filter_matches(access: &PermissionAccess, filter: ToolFilter) -> bool {
    match filter {
        ToolFilter::Any => true,
        ToolFilter::Bash => matches!(access, PermissionAccess::Bash { .. }),
        ToolFilter::Edit => matches!(access, PermissionAccess::Edit { .. }),
        ToolFilter::Read => matches!(
            access,
            PermissionAccess::Read { .. } | PermissionAccess::Grep { .. }
        ),
        ToolFilter::Grep => matches!(access, PermissionAccess::Grep { .. }),
        ToolFilter::Mcp => matches!(access, PermissionAccess::Mcp { .. }),
        ToolFilter::WebFetch => matches!(access, PermissionAccess::WebFetch(_)),
        ToolFilter::WebSearch => matches!(access, PermissionAccess::WebSearch(_)),
    }
}

fn tool_label(tool: ToolFilter) -> &'static str {
    match tool {
        ToolFilter::Any => "any tool",
        ToolFilter::Bash => "bash",
        ToolFilter::Edit => "edit",
        ToolFilter::Read => "read",
        ToolFilter::Grep => "grep",
        ToolFilter::Mcp => "mcp",
        ToolFilter::WebFetch => "web_fetch",
        ToolFilter::WebSearch => "web_search",
    }
}
