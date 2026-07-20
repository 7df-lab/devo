use serde::{Deserialize, Serialize};
use strum_macros::Display;

/// Windows sandbox enforcement level (stable wire values).
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Display)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum WindowsSandboxLevel {
    #[default]
    Disabled,
    RestrictedToken,
    Elevated,
}
