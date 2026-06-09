use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::text::Span;

use devo_protocol::InteractionMode;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum InputMode {
    #[default]
    Build,
    Plan,
    Shell,
}

impl InputMode {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Build => Self::Plan,
            Self::Plan => Self::Build,
            Self::Shell => Self::Build,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Build => "BUILD",
            Self::Plan => "PLAN",
            Self::Shell => "SHELL",
        }
    }

    pub(crate) fn styled_span(self, _show_cycle_hint: bool) -> Span<'static> {
        Span::styled(self.label(), Style::default().fg(self.color()))
    }

    pub(crate) fn color(self) -> Color {
        match self {
            Self::Build => Color::Cyan,
            Self::Plan => Color::Magenta,
            Self::Shell => Color::Rgb(245, 142, 53),
        }
    }

    pub(crate) fn interaction_mode(self) -> InteractionMode {
        match self {
            Self::Build | Self::Shell => InteractionMode::Build,
            Self::Plan => InteractionMode::Plan,
        }
    }

    pub(crate) fn from_interaction_mode(interaction_mode: InteractionMode) -> Self {
        match interaction_mode {
            InteractionMode::Build => Self::Build,
            InteractionMode::Plan => Self::Plan,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn input_mode_toggles_build_plan_and_styles_labels() {
        assert_eq!(InputMode::Build.next(), InputMode::Plan);
        assert_eq!(InputMode::Plan.next(), InputMode::Build);
        assert_eq!(InputMode::Shell.next(), InputMode::Build);

        assert_eq!(
            InputMode::Build.styled_span(false),
            Span::styled("BUILD", Style::default().fg(Color::Cyan))
        );
        assert_eq!(
            InputMode::Plan.styled_span(false),
            Span::styled("PLAN", Style::default().fg(Color::Magenta))
        );
        assert_eq!(
            InputMode::Shell.styled_span(false),
            Span::styled("SHELL", Style::default().fg(Color::Rgb(245, 142, 53)))
        );
        assert_eq!(InputMode::Build.interaction_mode(), InteractionMode::Build);
        assert_eq!(InputMode::Plan.interaction_mode(), InteractionMode::Plan);
        assert_eq!(InputMode::Shell.interaction_mode(), InteractionMode::Build);
        assert_eq!(
            InputMode::from_interaction_mode(InteractionMode::Build),
            InputMode::Build
        );
        assert_eq!(
            InputMode::from_interaction_mode(InteractionMode::Plan),
            InputMode::Plan
        );
    }
}
