use std::env;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

use devo_protocol::CollaborationMode;
use devo_protocol::Message;
use devo_protocol::Model;
use devo_protocol::ReasoningEffort;
use devo_protocol::UserInput;

use crate::SessionState;
use crate::TurnConfig;
use crate::context::AgentsMdDiff;
use crate::context::AgentsMdManager;
use crate::context::AgentsMdSnapshot;
use crate::context::ContextChangesFragment;
use crate::context::ContextualUserFragment;
use crate::context::MetadataContextChange;
use crate::context::user_instructions::UserInstructions;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Persona {
    #[default]
    Default,
}

impl Persona {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Default => "default",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentContext {
    pub cwd: PathBuf,
    pub shell: String,
    pub current_date: String,
    pub timezone: String,
}

impl EnvironmentContext {
    pub fn capture(cwd: &Path) -> Self {
        Self {
            cwd: cwd.to_path_buf(),
            shell: shell_basename(),
            current_date: chrono::Local::now().format("%Y-%m-%d").to_string(),
            timezone: iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string()),
        }
    }

    pub fn render(&self) -> String {
        format!(
            "<environment_context>\n  <cwd>{}</cwd>\n  <shell>{}</shell>\n  <current_date>{}</current_date>\n  <timezone>{}</timezone>\n</environment_context>",
            self.cwd.display(),
            self.shell,
            self.current_date,
            self.timezone,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageContext {
    pub language_preference: String,
}

impl Default for LanguageContext {
    fn default() -> Self {
        Self {
            language_preference: "Reply in the same natural language as the user's latest message. If the latest user message mixes languages, use the primary language of that message. Preserve technical terms, code identifiers, file paths, commands, API names, and quoted text in their original form unless the user explicitly asks to translate them. This language rule also applies to Proposed Plan: any content inside <proposed_plan></proposed_plan> must follow the same natural language as the user's latest message.".to_string(),
        }
    }
}

impl LanguageContext {
    pub fn render(&self) -> String {
        format!(
            "<language_preference>{}</language_preference>",
            self.language_preference
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionContext {
    pub base_instructions: String,
    #[serde(default)]
    pub available_skills: Option<String>,
    pub workspace_instructions: Option<String>,
    pub locked_agents_snapshot: Option<AgentsMdSnapshot>,
    pub environment: EnvironmentContext,
    #[serde(default)]
    pub language: LanguageContext,
    pub persona: Persona,
    pub model: Model,
    pub thinking_selection: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
}

impl SessionContext {
    pub fn capture(
        model: &Model,
        thinking_selection: Option<&str>,
        cwd: &Path,
        locked_agents_snapshot: Option<AgentsMdSnapshot>,
        available_skills: Option<String>,
    ) -> Self {
        let normalized_thinking_selection = model.normalize_thinking_selection(thinking_selection);
        let resolved = model.resolve_thinking_selection(normalized_thinking_selection.as_deref());
        let workspace_instructions = locked_agents_snapshot
            .as_ref()
            .map(|snapshot| snapshot.rendered_instructions.clone());
        Self {
            base_instructions: model.base_instructions.clone(),
            available_skills,
            workspace_instructions,
            locked_agents_snapshot,
            environment: EnvironmentContext::capture(cwd),
            language: LanguageContext::default(),
            persona: Persona::Default,
            model: model.clone(),
            thinking_selection: normalized_thinking_selection,
            reasoning_effort: resolved.effective_reasoning_effort,
        }
    }

    pub fn build_system_prompt(&self) -> String {
        let base = self.base_instructions.trim();
        let mode_prompt = crate::collaboration_mode_prompts::mode_introductions_prompt();
        if base.is_empty() {
            mode_prompt
        } else {
            format!("{base}\n\n{mode_prompt}")
        }
    }

    pub fn prefix_user_inputs(&self) -> Vec<UserInput> {
        let mut inputs = Vec::new();
        if let Some(text) = self
            .available_skills
            .as_ref()
            .filter(|text| !text.trim().is_empty())
        {
            inputs.push(UserInput::Text {
                text: text.trim().to_string(),
                text_elements: Vec::new(),
            });
        }
        if let Some(text) = self
            .workspace_instructions
            .as_ref()
            .filter(|text| !text.trim().is_empty())
        {
            inputs.push(UserInput::Text {
                text: UserInstructions {
                    directory: self.environment.cwd.display().to_string(),
                    text: text.clone(),
                }
                .render(),
                text_elements: Vec::new(),
            });
        }
        inputs.push(UserInput::Text {
            text: self.environment.render(),
            text_elements: Vec::new(),
        });
        inputs.push(UserInput::Text {
            text: self.language.render(),
            text_elements: Vec::new(),
        });
        inputs
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnContext {
    pub environment: EnvironmentContext,
    pub persona: Persona,
    pub model: Model,
    pub thinking_selection: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub observed_agents_snapshot: Option<AgentsMdSnapshot>,
    #[serde(default)]
    pub collaboration_mode: CollaborationMode,
}

impl TurnContext {
    pub fn capture(
        session: &SessionState,
        turn_config: &TurnConfig,
        observed_agents_snapshot: Option<AgentsMdSnapshot>,
    ) -> Self {
        let model = &turn_config.model;
        let thinking_selection = turn_config.thinking_selection.as_deref();
        let cwd = &session.cwd;
        let collaboration_mode = session.collaboration_mode;
        let normalized_thinking_selection = model.normalize_thinking_selection(thinking_selection);
        let resolved = model.resolve_thinking_selection(normalized_thinking_selection.as_deref());
        Self {
            environment: EnvironmentContext::capture(cwd),
            persona: Persona::Default,
            model: model.clone(),
            thinking_selection: normalized_thinking_selection,
            reasoning_effort: resolved.effective_reasoning_effort,
            observed_agents_snapshot,
            collaboration_mode,
        }
    }

    pub fn context_changes_since(&self, previous: Option<&TurnContext>) -> ContextChangesFragment {
        let mut metadata = Vec::new();
        let mut previous_collaboration_mode = None;
        let mut collaboration_mode_note = None;

        let Some(previous) = previous else {
            return ContextChangesFragment::new(
                self.collaboration_mode,
                previous_collaboration_mode,
                collaboration_mode_note,
                metadata,
            );
        };

        if self.environment.cwd != previous.environment.cwd {
            metadata.push(MetadataContextChange::new(
                "cwd",
                previous.environment.cwd.display().to_string(),
                self.environment.cwd.display().to_string(),
            ));
        }
        if self.environment.shell != previous.environment.shell {
            metadata.push(MetadataContextChange::new(
                "shell",
                previous.environment.shell.clone(),
                self.environment.shell.clone(),
            ));
        }
        if self.environment.current_date != previous.environment.current_date {
            metadata.push(MetadataContextChange::new(
                "current_date",
                previous.environment.current_date.clone(),
                self.environment.current_date.clone(),
            ));
        }
        if self.environment.timezone != previous.environment.timezone {
            metadata.push(MetadataContextChange::new(
                "timezone",
                previous.environment.timezone.clone(),
                self.environment.timezone.clone(),
            ));
        }
        if self.persona != previous.persona {
            metadata.push(MetadataContextChange::new(
                "persona",
                previous.persona.as_str().to_string(),
                self.persona.as_str().to_string(),
            ));
        }
        if self.model.slug != previous.model.slug {
            metadata.push(MetadataContextChange::new(
                "model",
                previous.model.slug.clone(),
                self.model.slug.clone(),
            ));
        }
        if self.thinking_selection != previous.thinking_selection {
            metadata.push(MetadataContextChange::new(
                "thinking_selection",
                format!("{:?}", previous.thinking_selection),
                format!("{:?}", self.thinking_selection),
            ));
        }
        if self.reasoning_effort != previous.reasoning_effort {
            metadata.push(MetadataContextChange::new(
                "reasoning_effort",
                format!("{:?}", previous.reasoning_effort),
                format!("{:?}", self.reasoning_effort),
            ));
        }
        if self.collaboration_mode != previous.collaboration_mode {
            previous_collaboration_mode = Some(previous.collaboration_mode);
            collaboration_mode_note = Some(
                "any previous instructions for other modes (e.g. Plan mode) are no longer active."
                    .to_string(),
            );
        }
        ContextChangesFragment::new(
            self.collaboration_mode,
            previous_collaboration_mode,
            collaboration_mode_note,
            metadata,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentsMdDiffFragment {
    diff: AgentsMdDiff,
}

impl AgentsMdDiffFragment {
    pub fn new(diff: AgentsMdDiff) -> Self {
        Self { diff }
    }

    pub fn to_message(&self) -> Message {
        Message::user(self.render())
    }
}

impl ContextualUserFragment for AgentsMdDiffFragment {
    const ROLE: &'static str = "user";
    const START_MARKER: &'static str = "<user_instructions_updates>";
    const END_MARKER: &'static str = "</user_instructions_updates>";

    fn body(&self) -> String {
        let mut lines = Vec::new();
        for path in &self.diff.added {
            lines.push(format!("added: {}", path.display()));
        }
        for path in &self.diff.removed {
            lines.push(format!("removed: {}", path.display()));
        }
        for path in &self.diff.changed {
            lines.push(format!("changed: {}", path.display()));
        }
        format!("\n{}\n", lines.join("\n"))
    }
}

pub fn load_workspace_instructions(
    cwd: &Path,
    manager: &AgentsMdManager,
) -> Option<AgentsMdSnapshot> {
    manager.load(cwd)
}

fn default_shell_name() -> String {
    #[cfg(target_os = "windows")]
    {
        return default_shell_windows();
    }

    #[cfg(target_os = "android")]
    {
        return default_shell_android();
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    {
        return default_shell_unix();
    }

    #[allow(unreachable_code)]
    "sh".to_string()
}

#[cfg(target_os = "windows")]
fn default_shell_windows() -> String {
    if let Some(shell) = env::var_os("COMSPEC")
        && !shell.is_empty()
    {
        return shell.to_string_lossy().into_owned();
    }

    "cmd.exe".to_string()
}

#[cfg(target_os = "android")]
fn default_shell_android() -> String {
    if let Some(shell) = env::var_os("SHELL")
        && !shell.is_empty()
    {
        return shell.to_string_lossy().into_owned();
    }

    "/system/bin/sh".to_string()
}

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
fn default_shell_unix() -> String {
    if let Some(shell) = env::var_os("SHELL")
        && !shell.is_empty()
    {
        return shell.to_string_lossy().into_owned();
    }

    "/bin/sh".to_string()
}

fn shell_basename() -> String {
    let shell = default_shell_name();

    Path::new(&shell)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or(shell.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::path::PathBuf;

    use devo_protocol::UserInput;
    use pretty_assertions::assert_eq;

    use super::EnvironmentContext;
    use super::LanguageContext;
    use super::SessionContext;
    use super::TurnContext;
    use crate::AgentsMdSnapshot;
    use crate::ContextualUserFragment;
    use crate::Model;
    use crate::ReasoningEffort;
    use crate::ThinkingCapability;
    use crate::context::user_instructions::UserInstructions;

    #[test]
    fn session_context_prefix_contains_locked_environment() {
        let context = SessionContext::capture(
            &Model {
                base_instructions: "base".into(),
                ..Model::default()
            },
            Some("enabled"),
            Path::new("/tmp/project"),
            Some(AgentsMdSnapshot {
                cwd: PathBuf::from("/tmp/project"),
                project_root: PathBuf::from("/tmp"),
                documents: Vec::new(),
                rendered_instructions: "workspace".into(),
            }),
            Some("<available_skills>skills</available_skills>".to_string()),
        );

        let prefix = context.prefix_user_inputs();
        assert_eq!(
            prefix,
            vec![
                UserInput::Text {
                    text: "<available_skills>skills</available_skills>".to_string(),
                    text_elements: Vec::new(),
                },
                UserInput::Text {
                    text: UserInstructions {
                        directory: "/tmp/project".to_string(),
                        text: "workspace".to_string(),
                    }
                    .render(),
                    text_elements: Vec::new(),
                },
                UserInput::Text {
                    text: context.environment.render(),
                    text_elements: Vec::new(),
                },
                UserInput::Text {
                    text: context.language.render(),
                    text_elements: Vec::new(),
                },
            ]
        );
    }

    #[test]
    fn language_context_renders_language_preference() {
        let context = LanguageContext::default();

        assert_eq!(
            context.render(),
            "<language_preference>Reply in the same natural language as the user's latest message. If the latest user message mixes languages, use the primary language of that message. Preserve technical terms, code identifiers, file paths, commands, API names, and quoted text in their original form unless the user explicitly asks to translate them. This language rule also applies to Proposed Plan: any content inside <proposed_plan></proposed_plan> must follow the same natural language as the user's latest message.</language_preference>"
        );
    }

    #[test]
    fn turn_context_diff_reports_model_and_reasoning_changes() {
        let previous = TurnContext {
            environment: EnvironmentContext {
                cwd: PathBuf::from("/tmp/a"),
                shell: "bash".into(),
                current_date: "2026-04-27".into(),
                timezone: "UTC".into(),
            },
            persona: super::Persona::Default,
            model: Model {
                slug: "a".into(),
                ..Model::default()
            },
            thinking_selection: Some("enabled".into()),
            reasoning_effort: Some(ReasoningEffort::Medium),
            observed_agents_snapshot: None,
            collaboration_mode: devo_protocol::CollaborationMode::Build,
        };
        let current = TurnContext {
            environment: EnvironmentContext {
                cwd: PathBuf::from("/tmp/b"),
                shell: "bash".into(),
                current_date: "2026-04-28".into(),
                timezone: "UTC".into(),
            },
            persona: super::Persona::Default,
            model: Model {
                slug: "b".into(),
                thinking_capability: ThinkingCapability::Toggle,
                ..Model::default()
            },
            thinking_selection: Some("disabled".into()),
            reasoning_effort: None,
            observed_agents_snapshot: None,
            collaboration_mode: devo_protocol::CollaborationMode::Build,
        };

        let diff = current.context_changes_since(Some(&previous));
        let rendered = diff.render();
        assert!(rendered.contains("<metadata>"));
        assert!(rendered.contains("<name>model</name>"));
        assert!(rendered.contains("<previous>a</previous>"));
        assert!(rendered.contains("<current>b</current>"));
        assert!(rendered.contains("<name>thinking_selection</name>"));
        assert!(rendered.contains("<name>reasoning_effort</name>"));
        assert!(rendered.contains("<previous>/tmp/a</previous>"));
        assert!(rendered.contains("<current>/tmp/b</current>"));
    }

    #[test]
    fn context_changes_fragment_roundtrips_to_message() {
        let context = TurnContext {
            environment: EnvironmentContext {
                cwd: PathBuf::from("/tmp/a"),
                shell: "bash".into(),
                current_date: "2026-04-27".into(),
                timezone: "UTC".into(),
            },
            persona: super::Persona::Default,
            model: Model::default(),
            thinking_selection: None,
            reasoning_effort: None,
            observed_agents_snapshot: None,
            collaboration_mode: devo_protocol::CollaborationMode::Build,
        };
        let fragment = context.context_changes_since(None);

        let message = fragment.to_message();
        assert_eq!(message.role, devo_protocol::Role::User);
        assert_eq!(message.content.len(), 1);
    }

    /// Trace: L2-DES-CONTEXT-001
    /// Verifies: Turn context diffs include collaboration-mode changes without full mode prompts.
    #[test]
    fn turn_context_diff_reports_collaboration_mode_changes_without_prompt() {
        let previous = TurnContext {
            environment: EnvironmentContext {
                cwd: PathBuf::from("/tmp/a"),
                shell: "bash".into(),
                current_date: "2026-04-27".into(),
                timezone: "UTC".into(),
            },
            persona: super::Persona::Default,
            model: Model {
                slug: "model-a".into(),
                ..Model::default()
            },
            thinking_selection: None,
            reasoning_effort: None,
            observed_agents_snapshot: None,
            collaboration_mode: devo_protocol::CollaborationMode::Plan,
        };
        let current = TurnContext {
            collaboration_mode: devo_protocol::CollaborationMode::Build,
            ..previous.clone()
        };

        let diff = current.context_changes_since(Some(&previous));
        let rendered = diff.render();

        assert!(rendered.contains("<collaboration_mode>"));
        assert!(rendered.contains("<previous>plan</previous>"));
        assert!(rendered.contains("<current>build</current>"));
        assert!(rendered.contains("<transition>plan -> build</transition>"));
        assert!(rendered.contains(
            "<note>any previous instructions for other modes (e.g. Plan mode) are no longer active.</note>"
        ));
        assert!(!rendered.contains("<collaboration_mode_build>"));
        assert!(!rendered.contains("<collaboration_mode_plan>"));
    }

    #[test]
    fn session_context_system_prompt_uses_stable_mode_introductions() {
        let context = SessionContext::capture(
            &Model {
                base_instructions: "base instructions".into(),
                ..Model::default()
            },
            None,
            Path::new("/tmp/a"),
            None,
            /*available_skills*/ None,
        );

        assert_eq!(
            context.build_system_prompt(),
            format!(
                "base instructions\n\n{}",
                crate::collaboration_mode_prompts::mode_introductions_prompt()
            )
        );
    }
}
