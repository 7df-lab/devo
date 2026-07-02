use devo_core::ResearchArtifactType;

pub(super) const RESEARCH_FILE_TOOL_NAMES: &[&str] = &["read", "write", "apply_patch"];
const RESEARCH_NO_TOOL_NAMES: &[&str] = &[];
const RESEARCH_CLARIFICATION_TOOL_NAMES: &[&str] = &["request_user_input"];
const RESEARCH_SUPERVISOR_TOOL_NAMES: &[&str] = &[
    "spawn_agent",
    "send_message",
    "wait_agent",
    "list_agents",
    "close_agent",
];
pub(crate) const RESEARCH_WORKER_TOOL_NAMES: &[&str] =
    &["read", "write", "apply_patch", "web_search", "webfetch"];

pub(crate) const RESEARCH_PIPELINE_STAGES: &[ResearchStageKind] = &[
    ResearchStageKind::Clarify,
    ResearchStageKind::Brief,
    ResearchStageKind::Supervisor,
    ResearchStageKind::Compress,
    ResearchStageKind::FinalReport,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResearchStageKind {
    Clarify,
    Brief,
    Supervisor,
    Compress,
    FinalReport,
}

impl ResearchStageKind {
    pub(super) fn prompt(self) -> String {
        match self {
            ResearchStageKind::Clarify => devo_core::research::prompts::clarify(),
            ResearchStageKind::Brief => devo_core::research::prompts::research_brief(),
            ResearchStageKind::Supervisor => devo_core::research::prompts::supervisor(),
            ResearchStageKind::Compress => devo_core::research::prompts::compress(),
            ResearchStageKind::FinalReport => devo_core::research::prompts::final_report(),
        }
    }

    pub(super) fn tool_names(self) -> &'static [&'static str] {
        match self {
            ResearchStageKind::Clarify => RESEARCH_CLARIFICATION_TOOL_NAMES,
            ResearchStageKind::Brief | ResearchStageKind::Compress => RESEARCH_NO_TOOL_NAMES,
            ResearchStageKind::Supervisor => RESEARCH_SUPERVISOR_TOOL_NAMES,
            ResearchStageKind::FinalReport => RESEARCH_FILE_TOOL_NAMES,
        }
    }

    pub(super) fn usage_prefix(self) -> &'static str {
        match self {
            ResearchStageKind::Clarify => "clarify_call",
            ResearchStageKind::Brief => "brief_call",
            ResearchStageKind::Supervisor => "supervisor_call",
            ResearchStageKind::Compress => "supervisor_compress_call",
            ResearchStageKind::FinalReport => "final_report_call",
        }
    }

    pub(super) fn artifact(self) -> Option<StreamedResearchArtifact> {
        match self {
            ResearchStageKind::Brief => Some(StreamedResearchArtifact {
                artifact_type: ResearchArtifactType::Brief,
                title: "Research Brief".to_string(),
            }),
            ResearchStageKind::Supervisor => Some(StreamedResearchArtifact {
                artifact_type: ResearchArtifactType::Plan,
                title: "Research Plan".to_string(),
            }),
            ResearchStageKind::Compress => Some(StreamedResearchArtifact {
                artifact_type: ResearchArtifactType::CompressedFinding,
                title: "Compressed Finding: Research Evidence".to_string(),
            }),
            ResearchStageKind::Clarify => Some(StreamedResearchArtifact {
                artifact_type: ResearchArtifactType::Clarification,
                title: "Research Clarification".to_string(),
            }),
            ResearchStageKind::FinalReport => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StreamedResearchArtifact {
    pub(super) artifact_type: ResearchArtifactType,
    pub(super) title: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn research_pipeline_stage_order_is_fixed() {
        assert_eq!(
            RESEARCH_PIPELINE_STAGES,
            &[
                ResearchStageKind::Clarify,
                ResearchStageKind::Brief,
                ResearchStageKind::Supervisor,
                ResearchStageKind::Compress,
                ResearchStageKind::FinalReport,
            ],
        );
    }

    #[test]
    fn research_stage_tool_policies_are_fixed() {
        const NO_TOOLS: &[&str] = &[];
        assert_eq!(
            ResearchStageKind::Clarify.tool_names(),
            ["request_user_input"].as_slice(),
        );
        assert_eq!(ResearchStageKind::Brief.tool_names(), NO_TOOLS);
        assert_eq!(
            ResearchStageKind::Supervisor.tool_names(),
            [
                "spawn_agent",
                "send_message",
                "wait_agent",
                "list_agents",
                "close_agent",
            ]
            .as_slice(),
        );
        assert_eq!(ResearchStageKind::Compress.tool_names(), NO_TOOLS);
        assert_eq!(
            ResearchStageKind::FinalReport.tool_names(),
            ["read", "write", "apply_patch"].as_slice(),
        );
    }

    #[test]
    fn research_stage_artifacts_are_fixed() {
        assert_eq!(
            ResearchStageKind::Clarify.artifact(),
            Some(StreamedResearchArtifact {
                artifact_type: ResearchArtifactType::Clarification,
                title: "Research Clarification".to_string(),
            }),
        );
        assert_eq!(
            ResearchStageKind::Brief.artifact(),
            Some(StreamedResearchArtifact {
                artifact_type: ResearchArtifactType::Brief,
                title: "Research Brief".to_string(),
            }),
        );
        assert_eq!(
            ResearchStageKind::Supervisor.artifact(),
            Some(StreamedResearchArtifact {
                artifact_type: ResearchArtifactType::Plan,
                title: "Research Plan".to_string(),
            }),
        );
        assert_eq!(
            ResearchStageKind::Compress.artifact(),
            Some(StreamedResearchArtifact {
                artifact_type: ResearchArtifactType::CompressedFinding,
                title: "Compressed Finding: Research Evidence".to_string(),
            }),
        );
        assert_eq!(ResearchStageKind::FinalReport.artifact(), None);
    }

    #[test]
    fn research_stage_usage_prefixes_are_fixed() {
        assert_eq!(ResearchStageKind::Clarify.usage_prefix(), "clarify_call");
        assert_eq!(ResearchStageKind::Brief.usage_prefix(), "brief_call");
        assert_eq!(
            ResearchStageKind::Supervisor.usage_prefix(),
            "supervisor_call"
        );
        assert_eq!(
            ResearchStageKind::Compress.usage_prefix(),
            "supervisor_compress_call",
        );
        assert_eq!(
            ResearchStageKind::FinalReport.usage_prefix(),
            "final_report_call",
        );
    }
}
