use devo_core::SessionState;
use devo_core::TurnConfig;

pub(crate) fn research_stage_system(stage_prompt: String) -> String {
    let mut system = devo_core::research::prompts::system();
    if !stage_prompt.trim().is_empty() {
        system.push_str("\n\n");
        system.push_str(stage_prompt.trim());
    }
    system
}

pub(crate) fn research_session_context(
    session: &SessionState,
    turn_config: &TurnConfig,
    system_prompt: String,
) -> devo_core::SessionContext {
    let model = &turn_config.model;
    let reasoning_effort_selection = turn_config.reasoning_effort_selection.as_deref();
    let normalized_reasoning_effort_selection =
        model.normalize_reasoning_effort_selection(reasoning_effort_selection);
    let resolved =
        model.resolve_reasoning_effort_selection(normalized_reasoning_effort_selection.as_deref());
    devo_core::SessionContext {
        base_instructions: system_prompt,
        available_skills: None,
        workspace_instructions: None,
        locked_agents_snapshot: None,
        environment: devo_core::EnvironmentContext::capture(&session.cwd),
        language: devo_core::LanguageContext::default(),
        persona: devo_core::Persona::Default,
        model: model.clone(),
        reasoning_effort_selection: normalized_reasoning_effort_selection,
        reasoning_effort: resolved.effective_reasoning_effort,
        system_prompt_mode: devo_core::SystemPromptMode::DeepResearch,
    }
}
