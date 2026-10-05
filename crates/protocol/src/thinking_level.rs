//! pi-ai-compatible thinking level map helpers (L2-DES-MODEL-003).
//!
//! Dual-writes alongside [`crate::ReasoningCapability`]: when a catalog entry
//! lacks an authored `thinkingLevelMap`, derive one so Native `model/list` and
//! InteractiveMode can use the same filter rules as
//! `@earendil-works/pi-ai` `getSupportedThinkingLevels` / `clampThinkingLevel`.

use std::collections::BTreeMap;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use strum_macros::Display;
use strum_macros::EnumIter;
use ts_rs::TS;

use crate::ReasoningCapability;
use crate::ReasoningEffort;
use crate::ReasoningLevelChoice;
use crate::normalize_reasoning_effort_literal;

/// Logical thinking / effort chip shown in UI (pi-ai `ModelThinkingLevel`).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
    TS,
    Display,
    EnumIter,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ModelThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl ModelThinkingLevel {
    /// Canonical lowercase wire / map key for this level.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }
}

impl FromStr for ModelThinkingLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" => Ok(Self::Off),
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::XHigh),
            "max" => Ok(Self::Max),
            _ => Err(format!("invalid model_thinking_level: {s}")),
        }
    }
}

/// Partial map of thinking level → provider wire string, or `None` for JSON
/// `null` (unsupported). Missing keys mean default-allowed for mid levels
/// (`off`/`minimal`/`low`/`medium`/`high`); `xhigh`/`max` require an explicit
/// non-null entry.
pub type ThinkingLevelMap = BTreeMap<String, Option<String>>;

const EXTENDED_THINKING_LEVELS: [ModelThinkingLevel; 7] = [
    ModelThinkingLevel::Off,
    ModelThinkingLevel::Minimal,
    ModelThinkingLevel::Low,
    ModelThinkingLevel::Medium,
    ModelThinkingLevel::High,
    ModelThinkingLevel::XHigh,
    ModelThinkingLevel::Max,
];

/// Returns the thinking levels the UI should offer, matching pi-ai
/// `getSupportedThinkingLevels`.
///
/// - `!reasoning` → `[Off]`
/// - map value `null` → excluded
/// - `xhigh` / `max` → included only when the key is present and non-null
/// - other levels → included when not explicitly null (missing key allowed)
pub fn get_supported_thinking_levels(
    reasoning: bool,
    map: Option<&ThinkingLevelMap>,
) -> Vec<ModelThinkingLevel> {
    if !reasoning {
        return vec![ModelThinkingLevel::Off];
    }

    EXTENDED_THINKING_LEVELS
        .into_iter()
        .filter(|level| {
            let key = level.as_str();
            let mapped = map.and_then(|m| m.get(key));
            match mapped {
                Some(None) => false,
                Some(Some(_)) => true,
                None => !matches!(level, ModelThinkingLevel::XHigh | ModelThinkingLevel::Max),
            }
        })
        .collect()
}

/// Clamps `level` to the nearest supported thinking level (pi-ai
/// `clampThinkingLevel` semantics: prefer higher neighbors, then lower).
pub fn clamp_thinking_level(
    level: ModelThinkingLevel,
    reasoning: bool,
    map: Option<&ThinkingLevelMap>,
) -> ModelThinkingLevel {
    let available = get_supported_thinking_levels(reasoning, map);
    if available.contains(&level) {
        return level;
    }

    let requested_index = EXTENDED_THINKING_LEVELS
        .iter()
        .position(|candidate| *candidate == level);
    let Some(requested_index) = requested_index else {
        return available
            .first()
            .copied()
            .unwrap_or(ModelThinkingLevel::Off);
    };

    for candidate in EXTENDED_THINKING_LEVELS.iter().skip(requested_index) {
        if available.contains(candidate) {
            return *candidate;
        }
    }
    for candidate in EXTENDED_THINKING_LEVELS[..requested_index].iter().rev() {
        if available.contains(candidate) {
            return *candidate;
        }
    }
    available
        .first()
        .copied()
        .unwrap_or(ModelThinkingLevel::Off)
}

/// Derives `(reasoning, thinkingLevelMap)` from legacy
/// [`ReasoningCapability`] when the catalog has no authored map.
///
/// - Unsupported → `(false, {})`
/// - Toggle → `(true, { off:"off", high:"on", others:null })` (UI `high` → wire `on`)
/// - Levels → `(true, map)` allowing listed choices; `Off` → `off:"off"`;
///   effort `None` → `off:"none"`; `xhigh`/`max` included only when listed
pub fn derive_thinking_fields_from_capability(
    cap: &ReasoningCapability,
) -> (bool, ThinkingLevelMap) {
    match cap {
        ReasoningCapability::Unsupported => (false, ThinkingLevelMap::new()),
        ReasoningCapability::Toggle => {
            let mut map = ThinkingLevelMap::new();
            map.insert("off".to_string(), Some("off".to_string()));
            map.insert("minimal".to_string(), None);
            map.insert("low".to_string(), None);
            map.insert("medium".to_string(), None);
            map.insert("high".to_string(), Some("on".to_string()));
            map.insert("xhigh".to_string(), None);
            map.insert("max".to_string(), None);
            (true, map)
        }
        ReasoningCapability::Levels(choices) => {
            let mut allowed = BTreeMap::<&str, String>::new();
            for choice in choices {
                match choice {
                    ReasoningLevelChoice::Off => {
                        allowed.entry("off").or_insert_with(|| "off".to_string());
                    }
                    ReasoningLevelChoice::Effort(ReasoningEffort::None) => {
                        allowed.entry("off").or_insert_with(|| "none".to_string());
                    }
                    ReasoningLevelChoice::Effort(effort) => {
                        let key = effort_thinking_level_key(*effort);
                        allowed.insert(key, key.to_string());
                    }
                }
            }

            let mut map = ThinkingLevelMap::new();
            for level in EXTENDED_THINKING_LEVELS {
                let key = level.as_str();
                if let Some(wire) = allowed.get(key) {
                    map.insert(key.to_string(), Some(wire.clone()));
                } else {
                    map.insert(key.to_string(), None);
                }
            }
            (true, map)
        }
    }
}

fn effort_thinking_level_key(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::None => "off",
        ReasoningEffort::Minimal => "minimal",
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
        ReasoningEffort::XHigh => "xhigh",
        ReasoningEffort::Max => "max",
    }
}

/// Normalizes a stored selection for UI chip keys: `disabled`→`off`,
/// `none`→`off`; `enabled`/`on` stay as `on` for callers to clamp.
pub fn normalize_reasoning_effort_selection_for_ui(raw: &str) -> String {
    match normalize_reasoning_effort_literal(raw).as_str() {
        "none" => String::from("off"),
        other => other.to_string(),
    }
}

/// Resolves `thinkingLevelMap` / `reasoning` for Native `ModelInfo`, preferring
/// authored catalog fields and falling back to capability derivation.
pub fn resolve_thinking_fields_for_model_info(
    capability: &ReasoningCapability,
    authored_reasoning: Option<bool>,
    authored_map: Option<&ThinkingLevelMap>,
) -> (bool, ThinkingLevelMap, Vec<String>) {
    let (reasoning, map) = match (authored_reasoning, authored_map) {
        (_, Some(map)) if !map.is_empty() => {
            let reasoning = authored_reasoning.unwrap_or(true);
            (reasoning, map.clone())
        }
        (Some(false), _) => (false, ThinkingLevelMap::new()),
        (Some(true), _) => {
            let (_, derived_map) = derive_thinking_fields_from_capability(capability);
            (true, derived_map)
        }
        (None, _) => derive_thinking_fields_from_capability(capability),
    };
    let available = get_supported_thinking_levels(reasoning, Some(&map))
        .into_iter()
        .map(|level| level.as_str().to_string())
        .collect();
    (reasoning, map, available)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::ModelThinkingLevel;
    use super::ThinkingLevelMap;
    use super::clamp_thinking_level;
    use super::derive_thinking_fields_from_capability;
    use super::get_supported_thinking_levels;
    use super::normalize_reasoning_effort_selection_for_ui;
    use crate::ReasoningCapability;
    use crate::ReasoningEffort;
    use crate::ReasoningLevelChoice;
    use crate::levels_with_leading_off;

    /// Trace: L2-DES-MODEL-003
    /// Verifies: unsupported reasoning yields only Off.
    #[test]
    fn get_supported_thinking_levels_unsupported() {
        let (reasoning, map) =
            derive_thinking_fields_from_capability(&ReasoningCapability::Unsupported);
        assert_eq!(reasoning, false);
        assert!(map.is_empty());
        assert_eq!(
            get_supported_thinking_levels(reasoning, Some(&map)),
            vec![ModelThinkingLevel::Off]
        );
    }

    /// Trace: L2-DES-MODEL-003
    /// Verifies: toggle-derived map exposes off + high (wire on).
    #[test]
    fn get_supported_thinking_levels_toggle_derived() {
        let (reasoning, map) = derive_thinking_fields_from_capability(&ReasoningCapability::Toggle);
        assert_eq!(reasoning, true);
        assert_eq!(map.get("high"), Some(&Some("on".to_string())));
        assert_eq!(map.get("minimal"), Some(&None));
        assert_eq!(
            get_supported_thinking_levels(reasoning, Some(&map)),
            vec![ModelThinkingLevel::Off, ModelThinkingLevel::High]
        );
    }

    /// Trace: L2-DES-MODEL-003
    /// Verifies: levels with max include max; none effort maps to UI off.
    #[test]
    fn get_supported_thinking_levels_with_max_and_none_to_off() {
        let cap = ReasoningCapability::Levels(vec![
            ReasoningLevelChoice::Effort(ReasoningEffort::None),
            ReasoningLevelChoice::Effort(ReasoningEffort::Low),
            ReasoningLevelChoice::Effort(ReasoningEffort::High),
            ReasoningLevelChoice::Effort(ReasoningEffort::Max),
        ]);
        let (reasoning, map) = derive_thinking_fields_from_capability(&cap);
        assert_eq!(map.get("off"), Some(&Some("none".to_string())));
        assert_eq!(map.get("max"), Some(&Some("max".to_string())));
        assert_eq!(map.get("xhigh"), Some(&None));
        assert_eq!(
            get_supported_thinking_levels(reasoning, Some(&map)),
            vec![
                ModelThinkingLevel::Off,
                ModelThinkingLevel::Low,
                ModelThinkingLevel::High,
                ModelThinkingLevel::Max,
            ]
        );
        assert_eq!(normalize_reasoning_effort_selection_for_ui("none"), "off");
        assert_eq!(
            normalize_reasoning_effort_selection_for_ui("disabled"),
            "off"
        );
    }

    /// Trace: L2-DES-MODEL-003
    /// Verifies: Off choice uses off wire value; xhigh requires explicit key.
    #[test]
    fn derive_levels_with_off_and_xhigh() {
        let cap = ReasoningCapability::Levels(levels_with_leading_off([
            ReasoningEffort::Medium,
            ReasoningEffort::XHigh,
        ]));
        let (reasoning, map) = derive_thinking_fields_from_capability(&cap);
        assert_eq!(map.get("off"), Some(&Some("off".to_string())));
        assert_eq!(map.get("xhigh"), Some(&Some("xhigh".to_string())));
        assert_eq!(
            get_supported_thinking_levels(reasoning, Some(&map)),
            vec![
                ModelThinkingLevel::Off,
                ModelThinkingLevel::Medium,
                ModelThinkingLevel::XHigh,
            ]
        );
    }

    /// Trace: L2-DES-MODEL-003
    /// Verifies: clamp walks toward nearest supported neighbor.
    #[test]
    fn clamp_thinking_level_snaps_to_supported() {
        let mut map = ThinkingLevelMap::new();
        map.insert("minimal".to_string(), None);
        map.insert("low".to_string(), None);
        map.insert("xhigh".to_string(), Some("xhigh".to_string()));
        assert_eq!(
            clamp_thinking_level(ModelThinkingLevel::Minimal, true, Some(&map)),
            ModelThinkingLevel::Medium
        );
        assert_eq!(
            clamp_thinking_level(ModelThinkingLevel::Max, true, Some(&map)),
            ModelThinkingLevel::XHigh
        );
    }
}
