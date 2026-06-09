use devo_protocol::{CollaborationMode, Message};

use crate::context::ContextualUserFragment;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextChangesFragment {
    collaboration_mode: CollaborationModeContextChange,
    metadata: Vec<MetadataContextChange>,
}

impl ContextChangesFragment {
    pub(crate) fn new(
        current_collaboration_mode: CollaborationMode,
        previous_collaboration_mode: Option<CollaborationMode>,
        collaboration_mode_note: Option<String>,
        metadata: Vec<MetadataContextChange>,
    ) -> Self {
        Self {
            collaboration_mode: CollaborationModeContextChange {
                previous: previous_collaboration_mode,
                current: current_collaboration_mode,
                note: collaboration_mode_note,
            },
            metadata,
        }
    }

    pub fn to_message(&self) -> Message {
        Message::user(self.render())
    }
}

impl ContextualUserFragment for ContextChangesFragment {
    const ROLE: &'static str = "user";
    const START_MARKER: &'static str = "<context_changes>";
    const END_MARKER: &'static str = "</context_changes>";

    fn body(&self) -> String {
        let mut sections = vec![self.collaboration_mode.render()];
        if !self.metadata.is_empty() {
            let metadata = self
                .metadata
                .iter()
                .map(MetadataContextChange::render)
                .collect::<Vec<_>>()
                .join("\n");
            sections.push(format!("<metadata>\n{metadata}\n</metadata>"));
        }
        format!("\n{}\n", sections.join("\n"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CollaborationModeContextChange {
    previous: Option<CollaborationMode>,
    current: CollaborationMode,
    note: Option<String>,
}

impl CollaborationModeContextChange {
    fn render(&self) -> String {
        let current = collaboration_mode_label(self.current);
        let mut lines = vec!["<collaboration_mode>".to_string()];
        if let Some(previous) = self.previous {
            let previous = collaboration_mode_label(previous);
            lines.push(format!(
                "<previous>{}</previous>",
                escape_context_xml(previous)
            ));
            lines.push(format!(
                "<current>{}</current>",
                escape_context_xml(current)
            ));
            lines.push(format!(
                "<transition>{} -> {}</transition>",
                escape_context_xml(previous),
                escape_context_xml(current)
            ));
        } else {
            lines.push(format!(
                "<current>{}</current>",
                escape_context_xml(current)
            ));
        }
        if let Some(note) = &self.note {
            lines.push(format!("<note>{}</note>", escape_context_xml(note)));
        }
        lines.push("</collaboration_mode>".to_string());
        lines.join("\n")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MetadataContextChange {
    name: &'static str,
    previous: String,
    current: String,
}

impl MetadataContextChange {
    pub(crate) fn new(name: &'static str, previous: String, current: String) -> Self {
        Self {
            name,
            previous,
            current,
        }
    }

    fn render(&self) -> String {
        format!(
            "<change>\n<name>{}</name>\n<previous>{}</previous>\n<current>{}</current>\n</change>",
            escape_context_xml(self.name),
            escape_context_xml(&self.previous),
            escape_context_xml(&self.current)
        )
    }
}

fn collaboration_mode_label(collaboration_mode: CollaborationMode) -> &'static str {
    match collaboration_mode {
        CollaborationMode::Build => "build",
        CollaborationMode::Plan => "plan",
    }
}

fn escape_context_xml(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;")
}
