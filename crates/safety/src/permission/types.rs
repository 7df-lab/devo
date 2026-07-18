use std::path::PathBuf;

/// A Devo tool access request evaluated by the compiled permission policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionAccess {
    Read {
        path: Option<PathBuf>,
        cwd: PathBuf,
    },
    Grep {
        path: Option<PathBuf>,
        glob: Option<String>,
        cwd: PathBuf,
        recursive: bool,
    },
    Edit {
        path: PathBuf,
        cwd: PathBuf,
    },
    Bash {
        command: String,
        cwd: PathBuf,
    },
    Mcp {
        name: String,
        input: serde_json::Value,
    },
    WebFetch(String),
    WebSearch(String),
}

/// Result of evaluating an access request against configured rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    Deny { reason: String },
    Ask,
    NoMatch,
}

impl PolicyDecision {
    pub(super) fn rank(&self) -> u8 {
        match self {
            Self::Deny { .. } => 3,
            Self::Ask => 2,
            Self::Allow => 1,
            Self::NoMatch => 0,
        }
    }

    pub(super) fn combine(self, other: Self) -> Self {
        match self.rank().cmp(&other.rank()) {
            std::cmp::Ordering::Greater => self,
            std::cmp::Ordering::Less => other,
            std::cmp::Ordering::Equal => match (&self, &other) {
                (Self::Deny { reason: left }, Self::Deny { reason: right }) if right < left => {
                    other
                }
                _ => self,
            },
        }
    }

    pub(super) fn escalation_only(self) -> Self {
        match self {
            Self::Deny { .. } | Self::Ask => self,
            Self::Allow | Self::NoMatch => Self::NoMatch,
        }
    }
}
