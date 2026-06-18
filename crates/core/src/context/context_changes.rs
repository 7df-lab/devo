//! Rendering for prompt-visible context-change fragments.
//!
//! These fragments are deliberately XML-like because downstream prompt
//! filtering recognizes them by stable outer markers. Keep rendering here
//! explicit so small formatting changes do not leak into the conversation
//! history or prompt normalization paths.

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
        let collaboration_mode = self.collaboration_mode.render();
        let mut body = String::with_capacity(collaboration_mode.len() + 2);
        body.push('\n');
        body.push_str(&collaboration_mode);
        if !self.metadata.is_empty() {
            body.push('\n');
            body.push_str("<metadata>\n");
            for (index, metadata) in self.metadata.iter().enumerate() {
                if index > 0 {
                    body.push('\n');
                }
                body.push_str(&metadata.render());
            }
            body.push_str("\n</metadata>");
        }
        body.push('\n');
        body
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
        let current = escape_context_xml(collaboration_mode_label(self.current));
        let mut rendered = String::from("<collaboration_mode>");
        if let Some(previous) = self.previous {
            let previous = escape_context_xml(collaboration_mode_label(previous));
            rendered.push_str("\n<previous>");
            rendered.push_str(&previous);
            rendered.push_str("</previous>");
            rendered.push_str("\n<current>");
            rendered.push_str(&current);
            rendered.push_str("</current>");
            rendered.push_str("\n<transition>");
            rendered.push_str(&previous);
            rendered.push_str(" -> ");
            rendered.push_str(&current);
            rendered.push_str("</transition>");
        } else {
            rendered.push_str("\n<current>");
            rendered.push_str(&current);
            rendered.push_str("</current>");
        }
        if let Some(note) = &self.note {
            rendered.push_str("\n<note>");
            rendered.push_str(&escape_context_xml(note));
            rendered.push_str("</note>");
        }
        rendered.push_str("\n</collaboration_mode>");
        rendered
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
    let Some(first_escape) = text.find(['&', '<']) else {
        return text.to_string();
    };

    let mut escaped = String::with_capacity(text.len());
    escaped.push_str(&text[..first_escape]);
    for ch in text[first_escape..].chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn context_changes_fragment_preserves_rendered_shape() {
        let fragment = ContextChangesFragment::new(
            CollaborationMode::Build,
            Some(CollaborationMode::Plan),
            Some("use <xml> & mode".to_string()),
            vec![MetadataContextChange::new(
                "cwd",
                "/tmp/<a>".to_string(),
                "/tmp/&b".to_string(),
            )],
        );

        assert_eq!(
            fragment.render(),
            "<context_changes>\n<collaboration_mode>\n<previous>plan</previous>\n<current>build</current>\n<transition>plan -> build</transition>\n<note>use &lt;xml> &amp; mode</note>\n</collaboration_mode>\n<metadata>\n<change>\n<name>cwd</name>\n<previous>/tmp/&lt;a></previous>\n<current>/tmp/&amp;b</current>\n</change>\n</metadata>\n</context_changes>"
        );
    }
}
