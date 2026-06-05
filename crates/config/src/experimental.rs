use serde::Deserialize;
use serde::Serialize;

/// Experimental feature gates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ExperimentalConfig {
    /// Enables the experimental built-in `code_search` tool.
    #[serde(
        default,
        rename = "code-search",
        alias = "code_search",
        skip_serializing_if = "is_false"
    )]
    pub code_search: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}
