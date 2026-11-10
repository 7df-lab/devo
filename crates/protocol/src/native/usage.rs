//! Usage accounting: every model call produces a metering record, without
//! exception. Metering is a property of the call channel
//! (`InstrumentedProvider` + `CallContext`), not of each call site's
//! diligence.
//!
//! Truth source: `devo-api-design/09-usage.md`.

use chrono::DateTime;
use chrono::Utc;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use super::ids::SessionId;
use super::ids::TurnId;
use super::model::ModelBinding;

/// Metering context that every model call must carry; the instrumented
/// provider wrapper makes it compile-time impossible to call without one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct CallContext {
    pub session_id: SessionId,
    /// Attribution: AutoReview/Compaction belong to their triggering turn;
    /// title generation etc. belong to the session only (`None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub purpose: UsagePurpose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum UsagePurpose {
    /// Main loop, including model retries and post-tool follow-ups.
    TurnQuery,
    /// Model reviewer before approval.
    AutoReview,
    /// History compaction summary.
    Compaction,
    /// Session title generation.
    TitleGeneration,
    /// Continual Harness refine planning (`L2-DES-HARNESS-001`).
    Refine,
    /// Mid-tool Python cell wait-policy decision.
    PythonCellWatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum UsageCallOutcome {
    Succeeded,
    Failed,
    Cancelled,
}

/// A provider counter that was not reported. Counts remain available as known
/// subtotals, but consumers must not present these fields as complete totals.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "camelCase")]
pub enum UsageField {
    InputTokens,
    OutputTokens,
    ReasoningTokens,
    CacheReadInputTokens,
    CacheCreationInputTokens,
    TotalTokens,
}

impl UsageField {
    pub const ALL: [Self; 6] = [
        Self::InputTokens,
        Self::OutputTokens,
        Self::ReasoningTokens,
        Self::CacheReadInputTokens,
        Self::CacheCreationInputTokens,
        Self::TotalTokens,
    ];
}

/// Provider-reported token usage for one call. Distinct from
/// `ContextUsage` (context-window occupancy): usage answers "how much was
/// spent", context answers "how full is the window".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    /// `None` is legacy/unknown coverage; an empty list means all fields known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unreported_fields: Option<Vec<UsageField>>,
    /// Provider-reported total, which can include tokens outside the primary
    /// input/output counts. `None` preserves older ledger records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    /// Billed at a different rate by most providers, hence separate.
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
}

/// Monetary amount in micro-units (1/1_000_000) of `currency`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct Money {
    pub currency: String,
    pub micros: i64,
}

/// Captured parent ownership for a child model call. This persists the
/// triggering parent turn rather than attributing delayed completions to
/// whichever parent turn happens to be active when the call finishes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct UsageAttribution {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
}

/// One append-only metering record per model call attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct UsageRecord {
    pub call_id: String,
    /// Parent ownership captured at call start, absent on older records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<UsageAttribution>,
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub purpose: UsagePurpose,
    /// Snapshot: the model this call actually used.
    pub model: ModelBinding,
    pub outcome: UsageCallOutcome,
    /// `None` when the provider did not report usage; never fabricated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    /// `None` when no price list is available; never fabricated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_cost: Option<Money>,
    pub recorded_at: DateTime<Utc>,
}

/// Aggregated token/call totals. `call_count` includes failed and cancelled
/// attempts; `metered_call_count` counts attempts with reported usage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct UsageTotals {
    /// `None` is legacy/unknown coverage; an empty list means all fields known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unreported_fields: Option<Vec<UsageField>>,
    /// Aggregate tokens, preferring each call's provider-reported total and
    /// otherwise using `input_tokens + output_tokens`. For legacy lumps it is
    /// the only known number (the input/output split was never recorded).
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub call_count: u32,
    pub metered_call_count: u32,
    pub failed_call_count: u32,
    pub cancelled_call_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_cost: Option<Money>,
}

impl Default for UsageTotals {
    fn default() -> Self {
        Self {
            unreported_fields: Some(Vec::new()),
            total_tokens: 0,
            input_tokens: 0,
            output_tokens: 0,
            reasoning_tokens: 0,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            call_count: 0,
            metered_call_count: 0,
            failed_call_count: 0,
            cancelled_call_count: 0,
            estimated_cost: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PurposeUsage {
    /// `None` is legacy/unknown coverage; an empty list means all fields known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unreported_fields: Option<Vec<UsageField>>,
    pub purpose: UsagePurpose,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub call_count: u32,
    pub metered_call_count: u32,
    pub failed_call_count: u32,
    pub cancelled_call_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_cost: Option<Money>,
}

/// Split so that "how much did this turn cost" and "the bill" reconcile:
/// `query` covers only `TurnQuery`; `overhead` covers the turn's
/// `AutoReview + Compaction`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct TurnUsage {
    pub query: UsageTotals,
    pub overhead: UsageTotals,
}

impl TurnUsage {
    /// Builds a turn query meter from one provider-reported usage sample.
    ///
    /// First-party live paths keep this Native shape end-to-end; overhead stays
    /// zero until AutoReview/Compaction metering is folded in.
    pub fn from_provider_usage(usage: &crate::Usage) -> Self {
        if !usage.is_reported() {
            return Self::default();
        }
        Self {
            query: UsageTotals {
                unreported_fields: usage.unreported_fields.clone(),
                total_tokens: u64::try_from(usage.display_total_tokens()).unwrap_or(u64::MAX),
                input_tokens: u64::try_from(usage.input_tokens).unwrap_or(u64::MAX),
                output_tokens: u64::try_from(usage.output_tokens).unwrap_or(u64::MAX),
                reasoning_tokens: u64::try_from(usage.reasoning_output_tokens.unwrap_or(0))
                    .unwrap_or(u64::MAX),
                cache_read_input_tokens: u64::try_from(usage.cache_read_input_tokens.unwrap_or(0))
                    .unwrap_or(u64::MAX),
                cache_creation_input_tokens: u64::try_from(
                    usage.cache_creation_input_tokens.unwrap_or(0),
                )
                .unwrap_or(u64::MAX),
                call_count: 0,
                metered_call_count: 1,
                ..UsageTotals::default()
            },
            overhead: UsageTotals::default(),
        }
    }

    /// Display total for the query meter (provider total when present).
    pub fn display_total_tokens(&self) -> u64 {
        if self.query.total_tokens > 0 {
            self.query.total_tokens
        } else {
            self.query
                .input_tokens
                .saturating_add(self.query.output_tokens)
        }
    }
}

/// Derived cache aggregated from the usage ledger (truth = ledger sum).
/// Exists solely so list views need not page through turn details; on any
/// disagreement the ledger wins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionUsage {
    pub total: UsageTotals,
    pub by_purpose: Vec<PurposeUsage>,
    /// Pre-upgrade historical totals that cannot be decomposed into calls;
    /// not disguised as call records. `total = legacy + ledger`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy: Option<UsageTotals>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::{TokenUsage, TurnUsage, UsageAttribution};
    use crate::Usage;

    #[test]
    fn turn_usage_distinguishes_unknown_from_reported_zero() {
        assert_eq!(
            TurnUsage::from_provider_usage(&Usage::default()),
            TurnUsage::default()
        );
        let reported_zero = Usage {
            reported: true,
            ..Usage::default()
        };
        let mut expected = TurnUsage::default();
        expected.query.metered_call_count = 1;
        expected.query.unreported_fields = None;
        assert_eq!(TurnUsage::from_provider_usage(&reported_zero), expected);
    }

    #[test]
    fn old_token_usage_and_parent_attribution_remain_readable() {
        let value = json!({
            "inputTokens": 1, "outputTokens": 2, "reasoningTokens": 3,
            "cacheReadInputTokens": 4, "cacheCreationInputTokens": 5
        });
        let usage: TokenUsage = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            usage,
            TokenUsage {
                unreported_fields: None,
                total_tokens: None,
                input_tokens: 1,
                output_tokens: 2,
                reasoning_tokens: 3,
                cache_read_input_tokens: 4,
                cache_creation_input_tokens: 5,
            }
        );
        assert_eq!(serde_json::to_value(usage).unwrap(), value);
        let attribution = json!({"sessionId": "00000000-0000-0000-0000-000000000001"});
        let parent: UsageAttribution = serde_json::from_value(attribution.clone()).unwrap();
        assert_eq!(serde_json::to_value(parent).unwrap(), attribution);
    }
}
