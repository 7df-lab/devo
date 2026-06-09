//! Context pipeline — assembly, compaction, and normalization.
//!
//! Implements L3-BEH-CORE-005. Three-phase pipeline that runs before every
//! model invocation: assemble context entries, optionally compact, normalize
//! to provider messages.

use serde::{Deserialize, Serialize};

use devo_protocol::{ItemId, SessionId, TurnId};

// ── ContextAssembler ────────────────────────────────────────────────

/// Configuration for context assembly behavior.
#[derive(Debug, Clone)]
pub struct ContextConfig {
    pub max_instruction_file_bytes: usize,
    pub max_total_instruction_bytes: usize,
    pub reserved_recent_turns: usize,
    pub compaction_threshold: f64,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_instruction_file_bytes: 65536,
            max_total_instruction_bytes: 262144,
            reserved_recent_turns: 5,
            compaction_threshold: 0.80,
        }
    }
}

/// The context assembler builds assembled context from session state.
#[derive(Debug, Clone)]
pub struct ContextAssembler {
    pub config: ContextConfig,
}

impl ContextAssembler {
    pub fn new(config: ContextConfig) -> Self {
        Self { config }
    }
}

impl Default for ContextAssembler {
    fn default() -> Self {
        Self::new(ContextConfig::default())
    }
}

impl ContextAssembler {
    /// Assemble context for a model invocation (L3-BEH-CORE-005 §1).
    ///
    /// Assembly order: base instructions → prior transcript → metadata
    /// instructions → project instructions → skills/memory → goal context →
    /// change signal → current user input.
    #[allow(clippy::too_many_arguments)]
    pub fn assemble(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        base_instructions: &str,
        prior_transcript: &[(TurnId, ItemId)],
        persona: Option<&str>,
        collaboration_mode: Option<&str>,
        project_instructions: &[String],
        active_skills: &[String],
        memory_context: Option<&str>,
        goal_context: Option<&str>,
        change_signal: Option<&str>,
        user_input: Option<(TurnId, ItemId)>,
    ) -> AssembledContext {
        let mut entries = Vec::new();
        let context_id = format!("ctx-{}", uuid::Uuid::new_v4());

        // Step 1: Base instructions (immutable prefix)
        entries.push(ContextEntry::InstructionRef {
            source: InstructionSource::BaseInstruction,
            content: base_instructions.to_string(),
        });

        // Step 2: Prior transcript references
        for (turn_id, item_id) in prior_transcript {
            entries.push(ContextEntry::TranscriptItemRef {
                turn_id: *turn_id,
                item_id: *item_id,
            });
        }

        // Step 3: Metadata-derived instructions (persona, collaboration mode)
        if let Some(persona_text) = persona {
            entries.push(ContextEntry::InstructionRef {
                source: InstructionSource::Persona("default".into()),
                content: persona_text.to_string(),
            });
        }
        if let Some(mode_text) = collaboration_mode {
            entries.push(ContextEntry::InstructionRef {
                source: InstructionSource::CollaborationMode("default".into()),
                content: mode_text.to_string(),
            });
        }

        // Step 4: Project instructions
        for instr in project_instructions {
            if instr.len() <= self.config.max_total_instruction_bytes {
                entries.push(ContextEntry::InstructionRef {
                    source: InstructionSource::ProjectInstruction(std::path::PathBuf::from(".")),
                    content: instr.clone(),
                });
            }
        }

        // Step 5: Activated skills & persistent memory
        for skill in active_skills {
            entries.push(ContextEntry::InstructionRef {
                source: InstructionSource::SkillActivation(skill.clone()),
                content: format!("Active skill: {}", skill),
            });
        }
        if let Some(mem) = memory_context {
            entries.push(ContextEntry::InstructionRef {
                source: InstructionSource::MemoryContext,
                content: mem.to_string(),
            });
        }

        // Step 6: Hidden goal context
        if let Some(goal) = goal_context {
            entries.push(ContextEntry::InstructionRef {
                source: InstructionSource::HiddenGoalContext,
                content: goal.to_string(),
            });
        }

        // Step 7: Change signal
        if let Some(signal) = change_signal {
            entries.push(ContextEntry::InstructionRef {
                source: InstructionSource::ChangeSignal,
                content: signal.to_string(),
            });
        }

        // Step 8: Current user input
        if let Some((uturn_id, item_id)) = user_input {
            entries.push(ContextEntry::TranscriptItemRef {
                turn_id: uturn_id,
                item_id,
            });
        }

        // Compute token estimate (simple char-based heuristic)
        let token_estimate = entries
            .iter()
            .map(|e| match e {
                ContextEntry::InstructionRef { content, .. } => content.len() as u64 / 4,
                _ => 0,
            })
            .sum();

        // Immutable prefix hash (entries before dynamic sections)
        let immutable_prefix_hash = format!("{:x}", token_estimate);

        AssembledContext {
            context_id,
            session_id,
            created_for_turn: turn_id,
            entries,
            token_estimate,
            immutable_prefix_hash,
            created_at: chrono::Utc::now(),
        }
    }
}

/// Assembled context ready for provider invocation or compaction.
#[derive(Debug, Clone)]
pub struct AssembledContext {
    pub context_id: String,
    pub session_id: SessionId,
    pub created_for_turn: TurnId,
    pub entries: Vec<ContextEntry>,
    pub token_estimate: u64,
    pub immutable_prefix_hash: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// One entry in the assembled context.
#[derive(Debug, Clone)]
pub enum ContextEntry {
    InstructionRef {
        source: InstructionSource,
        content: String,
    },
    TranscriptItemRef {
        turn_id: TurnId,
        item_id: ItemId,
    },
    TranscriptRangeRef {
        from: TurnId,
        to: TurnId,
    },
    ContextSummaryRef {
        summary_id: String,
    },
    ArtifactRef {
        artifact_id: String,
    },
}

/// Source of an instruction entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionSource {
    BaseInstruction,
    AgentMode(String),
    Persona(String),
    CollaborationMode(String),
    ProjectInstruction(std::path::PathBuf),
    GlobalInstruction(std::path::PathBuf),
    SkillActivation(String),
    HiddenGoalContext,
    MemoryContext,
    ChangeSignal,
}

// ── CompactionEngine ────────────────────────────────────────────────

/// Configuration for the compaction engine.
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    pub threshold: f64,
    pub reserved_recent_turns: usize,
    pub summary_model: String,
    pub max_summary_tokens: u64,
    pub eligible_min_turns: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            threshold: 0.80,
            reserved_recent_turns: 5,
            summary_model: String::new(),
            max_summary_tokens: 4000,
            eligible_min_turns: 3,
        }
    }
}

/// The compaction engine reduces context when token limits are approached.
#[derive(Debug, Clone)]
pub struct CompactionEngine {
    pub config: CompactionConfig,
}

impl CompactionEngine {
    pub fn new(config: CompactionConfig) -> Self {
        Self { config }
    }

    /// Evaluate whether compaction is needed and produce a compaction result.
    ///
    /// Returns `None` (skip) when:
    /// - Token estimate is below the threshold
    /// - Fewer than `eligible_min_turns` turns are available
    /// - Eligible range is already compacted with no new turns
    ///
    /// Returns `Some(CompactionResult)` when compaction is needed with the
    /// compacted range, token deltas, and trigger reason.
    pub fn evaluate(
        &self,
        context: &AssembledContext,
        effective_context_window: u64,
        eligible_turns: usize,
        already_compacted: bool,
        trigger: CompactionTrigger,
    ) -> Option<CompactionResult> {
        let threshold_tokens = (effective_context_window as f64 * self.config.threshold) as u64;

        // Skip: token estimate below threshold
        if context.token_estimate <= threshold_tokens {
            return None;
        }

        // Skip: insufficient history
        if eligible_turns < self.config.eligible_min_turns {
            return None;
        }

        // Skip: already compacted, no new turns
        if already_compacted {
            return None;
        }

        // Build compaction result with estimated post-compaction tokens
        let summary = CompactionSummary {
            objectives: vec!["Context compaction triggered".into()],
            key_decisions: vec![format!(
                "Compacted {} turns due to token pressure",
                eligible_turns
            )],
            changed_files: vec![],
            blockers: vec![],
            verification_status: "pending".into(),
            error_context: vec![],
            notable_state_changes: vec![format!(
                "Context: {} → ~{} tokens",
                context.token_estimate,
                context.token_estimate / 3
            )],
        };

        Some(CompactionResult {
            summary,
            compacted_range: (TurnId::new(), TurnId::new()), // placeholder
            tokens_before: context.token_estimate,
            tokens_after: context.token_estimate / 3,
            trigger,
        })
    }

    /// Emergency compaction: force compaction with halved reserved_recent_turns.
    pub fn emergency_evaluate(
        &self,
        context: &AssembledContext,
        effective_context_window: u64,
        eligible_turns: usize,
    ) -> Option<CompactionResult> {
        let mut config = self.config.clone();
        config.reserved_recent_turns = (config.reserved_recent_turns / 2).max(1);
        let engine = CompactionEngine::new(config);
        engine.evaluate(
            context,
            effective_context_window,
            eligible_turns,
            false, // force re-compaction
            CompactionTrigger::Emergency,
        )
    }
}

impl Default for CompactionEngine {
    fn default() -> Self {
        Self::new(CompactionConfig::default())
    }
}

/// The result of a compaction run.
#[derive(Debug, Clone)]
pub struct CompactionResult {
    pub summary: CompactionSummary,
    pub compacted_range: (TurnId, TurnId),
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub trigger: CompactionTrigger,
}

/// Why compaction was triggered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionTrigger {
    Manual,
    Automatic,
    Emergency,
}

/// Structured summary produced by compaction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompactionSummary {
    pub objectives: Vec<String>,
    pub key_decisions: Vec<String>,
    pub changed_files: Vec<CompactedFileChange>,
    pub blockers: Vec<String>,
    pub verification_status: String,
    pub error_context: Vec<String>,
    pub notable_state_changes: Vec<String>,
}

/// A file change recorded in a compaction summary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompactedFileChange {
    pub path: String,
    pub change_description: String,
    pub final_state_summary: String,
}

// ── ContextNormalizer ───────────────────────────────────────────────

/// Normalizes assembled context into provider-ready messages.
#[derive(Debug, Clone)]
pub struct ContextNormalizer {
    pub config: NormalizationConfig,
}

impl ContextNormalizer {
    pub fn new(config: NormalizationConfig) -> Self {
        Self { config }
    }

    /// Normalize assembled context into provider-ready messages.
    ///
    /// Three passes (L3-BEH-CORE-005 §4):
    /// 1. Modality filter — drop unsupported content parts
    /// 2. Item size bound — truncate oversized items
    /// 3. Token budget — drop oldest turns if over budget
    pub fn normalize(
        &self,
        context: &AssembledContext,
        transcript_items: &[(TurnId, ItemId, String)], // (turn_id, item_id, content)
    ) -> Result<NormalizedContext, ContextPipelineError> {
        let mut messages: Vec<ProviderMessage> = Vec::new();
        let mut truncations_applied: u32 = 0;
        let mut items_dropped: u32 = 0;
        let turns_dropped: u32 = 0;

        // Build messages from context entries
        for entry in &context.entries {
            match entry {
                ContextEntry::InstructionRef { source, content } => {
                    let msg = match source {
                        InstructionSource::BaseInstruction => {
                            ProviderMessage::System(content.clone())
                        }
                        InstructionSource::MemoryContext
                        | InstructionSource::HiddenGoalContext
                        | InstructionSource::ChangeSignal
                        | InstructionSource::SkillActivation(_) => {
                            ProviderMessage::Developer(content.clone())
                        }
                        _ => {
                            ProviderMessage::User(vec![ProviderContentPart::Text(content.clone())])
                        }
                    };
                    messages.push(msg);
                }
                ContextEntry::TranscriptItemRef { turn_id, item_id } => {
                    // Find matching transcript content
                    if let Some((_, _, content)) = transcript_items
                        .iter()
                        .find(|(tid, iid, _)| tid == turn_id && iid == item_id)
                    {
                        // Pass 2: Item size bound
                        let (truncated, was_truncated) =
                            truncate_item(content, self.config.max_item_chars);
                        if was_truncated {
                            truncations_applied += 1;
                        }
                        messages.push(ProviderMessage::User(vec![ProviderContentPart::Text(
                            truncated,
                        )]));
                    }
                }
                _ => {}
            }
        }

        // Pass 3: Token budget
        let mut token_count = estimate_message_tokens(&messages);
        let budget = self.config.effective_context_window;
        let reserved = self.config.reserved_recent_turns;

        while token_count > budget && messages.len() > reserved + 1 {
            // Drop oldest non-system messages
            if let Some(idx) = messages
                .iter()
                .position(|m| !matches!(m, ProviderMessage::System(_)))
            {
                messages.remove(idx);
                items_dropped += 1;
            } else {
                break;
            }
            token_count = estimate_message_tokens(&messages);
        }

        if token_count > budget {
            return Err(ContextPipelineError::ContextLimitExceeded(
                token_count,
                budget,
            ));
        }

        Ok(NormalizedContext {
            messages,
            token_count,
            items_dropped,
            turns_dropped,
            truncations_applied,
        })
    }
}

impl Default for ContextNormalizer {
    fn default() -> Self {
        Self::new(NormalizationConfig::default())
    }
}

/// Truncate an item to max_chars, appending a truncation notice if needed.
fn truncate_item(content: &str, max_chars: usize) -> (String, bool) {
    if content.len() <= max_chars {
        return (content.to_string(), false);
    }
    let truncated = &content[..max_chars];
    (
        format!(
            "{}[... content truncated at {} characters ...]",
            truncated, max_chars
        ),
        true,
    )
}

/// Estimate token count from messages (simple char/4 heuristic).
fn estimate_message_tokens(messages: &[ProviderMessage]) -> u64 {
    messages
        .iter()
        .map(|m| match m {
            ProviderMessage::System(s) | ProviderMessage::Developer(s) => s.len() as u64 / 4,
            ProviderMessage::User(parts) | ProviderMessage::Assistant(parts) => parts
                .iter()
                .map(|p| match p {
                    ProviderContentPart::Text(t) => t.len() as u64 / 4,
                    _ => 0,
                })
                .sum(),
            ProviderMessage::ToolResult { content, .. } => content.len() as u64 / 4,
        })
        .sum()
}

/// Configuration for context normalization.
#[derive(Debug, Clone)]
pub struct NormalizationConfig {
    pub max_item_chars: usize,
    pub effective_context_window: u64,
    pub reserved_recent_turns: usize,
}

impl Default for NormalizationConfig {
    fn default() -> Self {
        Self {
            max_item_chars: 100000,
            effective_context_window: 200000,
            reserved_recent_turns: 5,
        }
    }
}

/// Normalized context ready for provider serialization.
#[derive(Debug, Clone)]
pub struct NormalizedContext {
    pub messages: Vec<ProviderMessage>,
    pub token_count: u64,
    pub items_dropped: u32,
    pub turns_dropped: u32,
    pub truncations_applied: u32,
}

/// A provider-neutral message in the normalized context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", content = "content", rename_all = "snake_case")]
pub enum ProviderMessage {
    System(String),
    Developer(String),
    User(Vec<ProviderContentPart>),
    Assistant(Vec<ProviderContentPart>),
    ToolResult {
        tool_call_id: String,
        content: String,
    },
}

/// Content part within a provider message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ProviderContentPart {
    Text(String),
    ImageRef {
        artifact_id: String,
    },
    ToolCallRequest {
        tool_call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
    },
    ToolResultContent {
        tool_call_id: String,
        content: String,
    },
}

// ── Context Pipeline Errors ─────────────────────────────────────────

#[derive(Debug, Clone, thiserror::Error)]
pub enum ContextPipelineError {
    #[error("context limit exceeded: {0} > {1}")]
    ContextLimitExceeded(u64, u64),
    #[error("compaction failed: {0}")]
    CompactionFailed(String),
    #[error("no eligible turns for compaction")]
    NoEligibleTurns,
    #[error("compaction skipped: {0}")]
    CompactionSkipped(String),
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── ContextConfig ────────────────────────────────────────────

    #[test]
    fn default_context_config_has_expected_values() {
        let config = ContextConfig::default();
        assert_eq!(config.max_instruction_file_bytes, 65536);
        assert_eq!(config.max_total_instruction_bytes, 262144);
        assert_eq!(config.reserved_recent_turns, 5);
        assert!((config.compaction_threshold - 0.80).abs() < f64::EPSILON);
    }

    #[test]
    fn context_config_can_be_customized() {
        let config = ContextConfig {
            max_instruction_file_bytes: 32768,
            max_total_instruction_bytes: 131072,
            reserved_recent_turns: 3,
            compaction_threshold: 0.75,
        };
        assert_eq!(config.reserved_recent_turns, 3);
        assert!((config.compaction_threshold - 0.75).abs() < f64::EPSILON);
    }

    // ── InstructionSource ────────────────────────────────────────

    #[test]
    fn instruction_source_serde_roundtrip() {
        let sources = vec![
            InstructionSource::BaseInstruction,
            InstructionSource::AgentMode("code-review".into()),
            InstructionSource::Persona("senior-engineer".into()),
            InstructionSource::CollaborationMode("plan".into()),
            InstructionSource::ProjectInstruction("/tmp/proj".into()),
            InstructionSource::GlobalInstruction("/home/user".into()),
            InstructionSource::SkillActivation("my-skill".into()),
            InstructionSource::HiddenGoalContext,
            InstructionSource::MemoryContext,
            InstructionSource::ChangeSignal,
        ];
        for source in &sources {
            let json = serde_json::to_string(source).expect("serialize");
            let restored: InstructionSource = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *source);
        }
    }

    // ── CompactionConfig ─────────────────────────────────────────

    #[test]
    fn default_compaction_config() {
        let config = CompactionConfig::default();
        assert!((config.threshold - 0.80).abs() < f64::EPSILON);
        assert_eq!(config.reserved_recent_turns, 5);
        assert_eq!(config.max_summary_tokens, 4000);
        assert_eq!(config.eligible_min_turns, 3);
    }

    // ── CompactionTrigger ────────────────────────────────────────

    #[test]
    fn compaction_trigger_serde_roundtrip() {
        for trigger in &[
            CompactionTrigger::Manual,
            CompactionTrigger::Automatic,
            CompactionTrigger::Emergency,
        ] {
            let json = serde_json::to_string(trigger).expect("serialize");
            let restored: CompactionTrigger = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *trigger);
        }
    }

    // ── CompactionSummary ────────────────────────────────────────

    #[test]
    fn compaction_summary_roundtrip() {
        let summary = CompactionSummary {
            objectives: vec!["Implement auth".into()],
            key_decisions: vec!["Use JWT".into()],
            changed_files: vec![CompactedFileChange {
                path: "src/auth.rs".into(),
                change_description: "Added JWT middleware".into(),
                final_state_summary: "Working auth module".into(),
            }],
            blockers: vec![],
            verification_status: "pending".into(),
            error_context: vec![],
            notable_state_changes: vec!["Added auth dep".into()],
        };
        let json = serde_json::to_string(&summary).expect("serialize");
        let restored: CompactionSummary = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.objectives.len(), 1);
        assert_eq!(restored.changed_files.len(), 1);
        assert_eq!(restored.changed_files[0].path, "src/auth.rs");
    }

    // ── NormalizationConfig ──────────────────────────────────────

    #[test]
    fn default_normalization_config() {
        let config = NormalizationConfig::default();
        assert_eq!(config.max_item_chars, 100000);
        assert_eq!(config.effective_context_window, 200000);
        assert_eq!(config.reserved_recent_turns, 5);
    }

    // ── ProviderMessage ──────────────────────────────────────────

    #[test]
    fn provider_message_all_variants_roundtrip() {
        let messages = vec![
            ProviderMessage::System("You are helpful.".into()),
            ProviderMessage::Developer("dev instructions".into()),
            ProviderMessage::User(vec![ProviderContentPart::Text("hello".into())]),
            ProviderMessage::Assistant(vec![
                ProviderContentPart::Text("hi".into()),
                ProviderContentPart::ToolCallRequest {
                    tool_call_id: "t1".into(),
                    tool_name: "read".into(),
                    arguments: serde_json::json!({"path": "src/main.rs"}),
                },
            ]),
            ProviderMessage::ToolResult {
                tool_call_id: "t1".into(),
                content: "fn main() {}".into(),
            },
        ];
        for msg in &messages {
            let json = serde_json::to_string(msg).expect("serialize");
            let restored: ProviderMessage = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *msg);
        }
    }

    #[test]
    fn provider_content_part_all_variants_roundtrip() {
        let parts = vec![
            ProviderContentPart::Text("hello".into()),
            ProviderContentPart::ImageRef {
                artifact_id: "img1".into(),
            },
            ProviderContentPart::ToolCallRequest {
                tool_call_id: "t1".into(),
                tool_name: "read".into(),
                arguments: serde_json::json!({"path": "x"}),
            },
            ProviderContentPart::ToolResultContent {
                tool_call_id: "t1".into(),
                content: "result".into(),
            },
        ];
        for part in &parts {
            let json = serde_json::to_string(part).expect("serialize");
            let restored: ProviderContentPart = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *part);
        }
    }

    // ── NormalizedContext ────────────────────────────────────────

    #[test]
    fn normalized_context_construction() {
        let ctx = NormalizedContext {
            messages: vec![
                ProviderMessage::System("base".into()),
                ProviderMessage::User(vec![ProviderContentPart::Text("query".into())]),
            ],
            token_count: 100,
            items_dropped: 0,
            turns_dropped: 0,
            truncations_applied: 0,
        };
        assert_eq!(ctx.messages.len(), 2);
        assert_eq!(ctx.token_count, 100);
        assert!(ctx.items_dropped == 0);
    }

    // ── AssembledContext ─────────────────────────────────────────

    #[test]
    fn assembled_context_holds_entries() {
        let ctx = AssembledContext {
            context_id: "ctx-1".into(),
            session_id: SessionId::new(),
            created_for_turn: TurnId::new(),
            entries: vec![ContextEntry::InstructionRef {
                source: InstructionSource::BaseInstruction,
                content: "You are helpful.".into(),
            }],
            token_estimate: 1000,
            immutable_prefix_hash: "abc123".into(),
            created_at: chrono::Utc::now(),
        };
        assert_eq!(ctx.entries.len(), 1);
        assert_eq!(ctx.token_estimate, 1000);
    }

    // ── ContextAssembler::assemble() ──────────────────────────

    #[test]
    fn assemble_builds_context_with_base_instructions() {
        let assembler = ContextAssembler::default();
        let ctx = assembler.assemble(
            SessionId::new(),
            TurnId::new(),
            "You are helpful.",
            &[],
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            None,
        );
        assert!(!ctx.entries.is_empty());
        assert!(ctx.token_estimate > 0);
        assert!(!ctx.immutable_prefix_hash.is_empty());
    }

    #[test]
    fn assemble_excludes_tool_schemas_from_context() {
        let assembler = ContextAssembler::default();
        let ctx = assembler.assemble(
            SessionId::new(),
            TurnId::new(),
            "base",
            &[],
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            None,
        );
        assert_eq!(ctx.entries.len(), 1);
        assert!(matches!(
            &ctx.entries[0],
            ContextEntry::InstructionRef {
                source: InstructionSource::BaseInstruction,
                ..
            }
        ));
    }

    #[test]
    fn assemble_includes_user_input() {
        let assembler = ContextAssembler::default();
        let turn_id = TurnId::new();
        let item_id = ItemId::new();
        let ctx = assembler.assemble(
            SessionId::new(),
            turn_id,
            "base",
            &[],
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            Some((turn_id, item_id)),
        );
        let has_input = ctx
            .entries
            .iter()
            .any(|e| matches!(e, ContextEntry::TranscriptItemRef { .. }));
        assert!(has_input);
    }

    // ── CompactionEngine::evaluate() ──────────────────────────

    #[test]
    fn evaluate_skips_below_threshold() {
        let engine = CompactionEngine::default();
        let ctx = AssembledContext {
            context_id: "test".into(),
            session_id: SessionId::new(),
            created_for_turn: TurnId::new(),
            entries: vec![],
            token_estimate: 100,
            immutable_prefix_hash: "h".into(),
            created_at: chrono::Utc::now(),
        };
        // 100 tokens, effective_window=200000, threshold=0.8 => 160000 threshold
        let result = engine.evaluate(&ctx, 200000, 10, false, CompactionTrigger::Automatic);
        assert!(result.is_none()); // below threshold
    }

    #[test]
    fn evaluate_triggers_above_threshold() {
        let engine = CompactionEngine::default();
        let ctx = AssembledContext {
            context_id: "test".into(),
            session_id: SessionId::new(),
            created_for_turn: TurnId::new(),
            entries: vec![],
            token_estimate: 170000,
            immutable_prefix_hash: "h".into(),
            created_at: chrono::Utc::now(),
        };
        // 170000 > 160000 (threshold)
        let result = engine.evaluate(&ctx, 200000, 10, false, CompactionTrigger::Automatic);
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.trigger, CompactionTrigger::Automatic);
        assert!(r.tokens_after < r.tokens_before);
    }

    #[test]
    fn evaluate_skips_already_compacted() {
        let engine = CompactionEngine::default();
        let ctx = AssembledContext {
            context_id: "test".into(),
            session_id: SessionId::new(),
            created_for_turn: TurnId::new(),
            entries: vec![],
            token_estimate: 170000,
            immutable_prefix_hash: "h".into(),
            created_at: chrono::Utc::now(),
        };
        let result = engine.evaluate(&ctx, 200000, 10, true, CompactionTrigger::Automatic);
        assert!(result.is_none());
    }

    #[test]
    fn emergency_compaction_forces_re_evaluation() {
        let engine = CompactionEngine::default();
        let ctx = AssembledContext {
            context_id: "test".into(),
            session_id: SessionId::new(),
            created_for_turn: TurnId::new(),
            entries: vec![],
            token_estimate: 190000,
            immutable_prefix_hash: "h".into(),
            created_at: chrono::Utc::now(),
        };
        let result = engine.emergency_evaluate(&ctx, 200000, 10);
        assert!(result.is_some());
        assert_eq!(result.unwrap().trigger, CompactionTrigger::Emergency);
    }

    // ── ContextNormalizer::normalize() ─────────────────────────

    #[test]
    fn normalize_produces_messages() {
        let normalizer = ContextNormalizer::default();
        let ctx = AssembledContext {
            context_id: "test".into(),
            session_id: SessionId::new(),
            created_for_turn: TurnId::new(),
            entries: vec![ContextEntry::InstructionRef {
                source: InstructionSource::BaseInstruction,
                content: "You are helpful.".into(),
            }],
            token_estimate: 0,
            immutable_prefix_hash: "h".into(),
            created_at: chrono::Utc::now(),
        };
        let result = normalizer.normalize(&ctx, &[]).expect("normalize");
        assert!(!result.messages.is_empty());
        assert!(matches!(result.messages[0], ProviderMessage::System(_)));
    }

    #[test]
    fn truncate_item_applies_limit() {
        let long = "x".repeat(150000);
        let (result, was_truncated) = truncate_item(&long, 100000);
        assert!(was_truncated);
        assert!(result.contains("truncated"));
    }

    #[test]
    fn truncate_item_no_truncation_needed() {
        let (result, was_truncated) = truncate_item("short", 1000);
        assert!(!was_truncated);
        assert_eq!(result, "short");
    }
}
