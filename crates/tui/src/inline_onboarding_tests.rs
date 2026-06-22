//! Inline onboarding transcript and header behavior tests.

use std::path::PathBuf;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyEventState;
use crossterm::event::KeyModifiers;
use devo_protocol::Model;
use devo_protocol::ProviderModelBinding;
use devo_protocol::ProviderVendor;
use devo_protocol::ProviderWireApi;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::chatwidget::ChatWidget;
use crate::chatwidget::ChatWidgetInit;
use crate::chatwidget::TuiSessionState;
use crate::events::WorkerEvent;
use crate::render::renderable::Renderable;
use crate::tui::frame_requester::FrameRequester;

fn onboarding_widget_with_available_model(
    model: Model,
    cwd: PathBuf,
) -> (ChatWidget, mpsc::UnboundedReceiver<AppEvent>) {
    let (app_event_tx, app_event_rx) = mpsc::unbounded_channel();
    let widget = ChatWidget::new_with_app_event(ChatWidgetInit {
        frame_requester: FrameRequester::test_dummy(),
        app_event_tx: AppEventSender::new(app_event_tx),
        initial_session: TuiSessionState::new(cwd, Some(model.clone())),
        initial_reasoning_effort_selection: None,
        initial_permission_preset: devo_protocol::PermissionPreset::Default,
        initial_user_message: None,
        enhanced_keys_supported: true,
        is_first_run: false,
        available_models: vec![model],
        saved_models: Vec::new(),
        show_model_onboarding: true,
        startup_tooltip_override: None,
        initial_theme_name: None,
    });
    (widget, app_event_rx)
}

fn test_model() -> Model {
    Model {
        slug: "deepseek-v4-flash".to_string(),
        display_name: "Deepseek V4 Flash".to_string(),
        ..Model::default()
    }
}

fn deepseek_vendor() -> ProviderVendor {
    ProviderVendor {
        name: "Deepseek".to_string(),
        base_url: Some("https://api.deepseek.com".to_string()),
        credential: Some("deepseek_api_key".to_string()),
        headers: None,
        wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
        enabled: true,
    }
}

fn press_key(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn rendered_rows(widget: &ChatWidget, width: u16, height: u16) -> Vec<String> {
    let area = ratatui::layout::Rect::new(0, 0, width, height);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    widget.render(area, &mut buf);
    (0..area.height)
        .map(|row| {
            (0..area.width)
                .map(|col| buf[(col, row)].symbol())
                .collect::<String>()
        })
        .collect()
}

fn scrollback_plain_lines(lines: &[crate::history_cell::ScrollbackLine]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

#[test]
fn first_run_onboarding_starts_with_logo_and_renders_inline() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let (mut widget, _app_event_rx) = onboarding_widget_with_available_model(test_model(), cwd);

    let scrollback = scrollback_plain_lines(&widget.drain_scrollback_lines(100)).join("\n");
    assert!(scrollback.contains("██████"));
    assert!(!scrollback.contains("Workspace"));
    assert!(!scrollback.contains("Model      deepseek-v4-flash"));

    let rows = rendered_rows(&widget, 100, 24).join("\n");
    assert!(rows.contains("Choose model profile"));
    assert!(rows.contains("Complete onboarding to start chatting"));
    assert!(widget.desired_height(100) < u16::MAX);
}

#[test]
fn onboarding_completion_appends_header_after_success_record() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let (mut widget, mut app_event_rx) = onboarding_widget_with_available_model(test_model(), cwd);

    let _ = app_event_rx.try_recv().expect("provider list command");
    widget.handle_worker_event(WorkerEvent::ProviderVendorsListed {
        provider_vendors: vec![deepseek_vendor()],
    });
    widget.handle_key_event(press_key(KeyCode::Enter));
    widget.handle_key_event(press_key(KeyCode::Enter));
    widget.handle_key_event(press_key(KeyCode::Enter));
    widget.handle_key_event(press_key(KeyCode::Enter));
    widget.handle_key_event(press_key(KeyCode::Enter));
    let _ = app_event_rx.try_recv().expect("onboard command");

    widget.handle_worker_event(WorkerEvent::ProviderValidationSucceeded {
        reply_preview: "OK".to_string(),
    });
    assert_eq!(widget.is_onboarding_active(), true);

    widget.handle_worker_event(WorkerEvent::ProviderVendorUpserted {
        provider_vendor: deepseek_vendor(),
        model_binding: Some(ProviderModelBinding {
            binding_id: "deepseek-v4-flash-deepseek".to_string(),
            model_slug: "deepseek-v4-flash".to_string(),
            provider: "Deepseek".to_string(),
            model_name: "DeepSeek-V4-Flash".to_string(),
            display_name: Some("DeepSeek-V4-Flash".to_string()),
            invocation_method: ProviderWireApi::OpenAIChatCompletions,
            default_reasoning_effort: None,
            enabled: true,
        }),
    });

    assert_eq!(widget.is_onboarding_active(), false);
    assert_eq!(widget.placeholder_text(), "Ask Devo");
    assert_eq!(
        widget.current_model().map(|model| model.slug.as_str()),
        Some("deepseek-v4-flash")
    );
    assert!(widget.status_summary_text().contains("DeepSeek-V4-Flash"));

    let lines = scrollback_plain_lines(&widget.drain_scrollback_lines(100));
    let success_idx = lines
        .iter()
        .position(|line| line.contains("Provider configured successfully"))
        .expect("success record should be present");
    let header_idx = lines
        .iter()
        .rposition(|line| line.contains("Workspace"))
        .expect("final session header should be appended");
    assert!(header_idx > success_idx);
    assert!(!lines.join("\n").contains("Provider saved: Deepseek"));
}
