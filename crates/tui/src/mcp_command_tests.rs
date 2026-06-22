//! Focused regression tests for the `/mcp` slash command wiring.

use std::path::PathBuf;

use devo_protocol::Model;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc;

use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::chatwidget::ChatWidget;
use crate::chatwidget::ChatWidgetInit;
use crate::chatwidget::TuiSessionState;
use crate::tui::frame_requester::FrameRequester;

fn widget_with_model(model: Model) -> (ChatWidget, mpsc::UnboundedReceiver<AppEvent>) {
    let (app_event_tx, app_event_rx) = mpsc::unbounded_channel();
    let widget = ChatWidget::new_with_app_event(ChatWidgetInit {
        frame_requester: FrameRequester::test_dummy(),
        app_event_tx: AppEventSender::new(app_event_tx),
        initial_session: TuiSessionState::new(PathBuf::from("."), Some(model)),
        initial_reasoning_effort_selection: None,
        initial_permission_preset: devo_protocol::PermissionPreset::Default,
        initial_user_message: None,
        enhanced_keys_supported: true,
        is_first_run: false,
        available_models: Vec::new(),
        saved_models: Vec::new(),
        show_model_onboarding: false,
        startup_tooltip_override: None,
        initial_theme_name: None,
    });
    (widget, app_event_rx)
}

#[test]
fn run_slash_command_mcp_queues_mcp_list_command() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model);

    widget.handle_app_event(AppEvent::RunSlashCommand {
        command: "mcp".to_string(),
    });

    assert_eq!(
        app_event_rx.try_recv().expect("queued app event"),
        AppEvent::Command(AppCommand::RunUserShellCommand {
            command: "mcp list".to_string(),
        })
    );
}
