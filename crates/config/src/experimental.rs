use serde::Deserialize;
use serde::Serialize;

/// Experimental feature gates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ExperimentalConfig {}
