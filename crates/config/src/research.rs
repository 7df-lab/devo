use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResearchConfig {
    #[serde(default = "default_max_researcher_iterations")]
    pub max_researcher_iterations: usize,
    #[serde(default = "default_fetch_summary_threshold_chars")]
    pub fetch_summary_threshold_chars: usize,
    #[serde(default = "default_max_summary_chars")]
    pub max_summary_chars: usize,
}

impl Default for ResearchConfig {
    fn default() -> Self {
        Self {
            max_researcher_iterations: default_max_researcher_iterations(),
            fetch_summary_threshold_chars: default_fetch_summary_threshold_chars(),
            max_summary_chars: default_max_summary_chars(),
        }
    }
}

impl ResearchConfig {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

fn default_max_researcher_iterations() -> usize {
    5
}

fn default_fetch_summary_threshold_chars() -> usize {
    24_000
}

fn default_max_summary_chars() -> usize {
    8_000
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn research_config_defaults_are_stable() {
        // Trace: L2-DES-RESEARCH-001
        // Verifies: deep research runtime caps have stable default values.
        let config = ResearchConfig::default();

        assert_eq!(
            config,
            ResearchConfig {
                max_researcher_iterations: 5,
                fetch_summary_threshold_chars: 24_000,
                max_summary_chars: 8_000,
            }
        );
    }
}
