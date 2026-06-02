use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyEventState;
use crossterm::event::KeyModifiers;
use devo_protocol::Model;
use devo_protocol::ProviderVendor;
use devo_protocol::ProviderWireApi;
use devo_protocol::ReasoningEffort;
use devo_protocol::ThinkingCapability;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc;

use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::onboarding_widget::OnboardingWidget;
use crate::render::renderable::Renderable;
use crate::tui::frame_requester::FrameRequester;

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn shift_char(ch: char) -> KeyEvent {
    KeyEvent {
        code: KeyCode::Char(ch),
        modifiers: KeyModifiers::SHIFT,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn plain_char(ch: char) -> KeyEvent {
    KeyEvent {
        code: KeyCode::Char(ch),
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn rendered_rows(widget: &OnboardingWidget, width: u16, height: u16) -> Vec<String> {
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

fn next_shell_command(app_event_rx: &mut mpsc::UnboundedReceiver<AppEvent>) -> String {
    loop {
        if let AppEvent::Command(AppCommand::RunUserShellCommand { command }) =
            app_event_rx.try_recv().expect("expected queued app event")
        {
            return command;
        }
    }
}

fn deepseek_model() -> Model {
    devo_core::ModelPreset {
        slug: "deepseek-v4-flash".to_string(),
        display_name: "Deepseek V4 Flash".to_string(),
        thinking_capability: ThinkingCapability::Toggle,
        supported_reasoning_levels: vec![ReasoningEffort::High, ReasoningEffort::Max],
        default_reasoning_effort: Some(ReasoningEffort::High),
        ..devo_core::ModelPreset::default()
    }
    .into()
}

#[test]
fn onboarding_existing_provider_renders_values_after_labels_and_masks_saved_key() {
    let models = vec![deepseek_model()];
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let mut widget = OnboardingWidget::new(
        &models,
        AppEventSender::new(app_event_tx),
        FrameRequester::test_dummy(),
        true,
    );
    assert_eq!(
        next_shell_command(&mut app_event_rx),
        "provider list".to_string()
    );

    widget.on_provider_vendors_listed(vec![ProviderVendor {
        name: "Deepseek".to_string(),
        base_url: Some("https://api.deepseek.com".to_string()),
        credential: Some("deepseek_api_key".to_string()),
        wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
        enabled: true,
    }]);

    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));

    let rows = rendered_rows(&widget, 160, 40);
    let provider_row = rows
        .iter()
        .find(|row| row.contains("Provider Name:"))
        .expect("provider row");
    let provider_hint_row = rows
        .iter()
        .find(|row| row.contains("Enter a name to recognize this provider later."))
        .expect("provider hint row");
    let base_url_row = rows
        .iter()
        .find(|row| row.contains("Base URL:"))
        .expect("base url row");
    let api_key_row = rows
        .iter()
        .find(|row| row.contains("API Key:"))
        .expect("api key row");

    assert_eq!(provider_row.contains("Provider Name: Deepseek"), true);
    assert_eq!(
        provider_hint_row
            .trim()
            .contains("Enter a name to recognize this provider later."),
        true
    );
    assert_eq!(
        base_url_row.contains("Base URL: https://api.deepseek.com"),
        true
    );
    assert_eq!(api_key_row.contains("API Key: ****...***"), true);
}

#[test]
fn onboarding_required_provider_name_and_base_url_do_not_advance_when_empty() {
    let models = vec![deepseek_model()];
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let mut widget = OnboardingWidget::new(
        &models,
        AppEventSender::new(app_event_tx),
        FrameRequester::test_dummy(),
        true,
    );
    assert_eq!(
        next_shell_command(&mut app_event_rx),
        "provider list".to_string()
    );

    widget.on_provider_vendors_listed(Vec::new());

    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));

    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(shift_char('D'));

    let provider_rows = rendered_rows(&widget, 160, 40);
    let provider_row = provider_rows
        .iter()
        .find(|row| row.contains("Provider Name:"))
        .expect("provider row after blocked advance");
    assert_eq!(provider_row.contains("Provider Name: D"), true);

    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(plain_char('h'));

    let base_url_rows = rendered_rows(&widget, 160, 40);
    let base_url_row = base_url_rows
        .iter()
        .find(|row| row.contains("Base URL:"))
        .expect("base url row after blocked advance");
    assert_eq!(base_url_row.contains("Base URL: h"), true);
}

#[test]
fn onboarding_invocation_and_reasoning_popups_render_inline_and_use_model_presets() {
    let models = vec![deepseek_model()];
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let mut widget = OnboardingWidget::new(
        &models,
        AppEventSender::new(app_event_tx),
        FrameRequester::test_dummy(),
        true,
    );
    assert_eq!(
        next_shell_command(&mut app_event_rx),
        "provider list".to_string()
    );

    widget.on_provider_vendors_listed(vec![ProviderVendor {
        name: "Deepseek".to_string(),
        base_url: Some("https://api.deepseek.com".to_string()),
        credential: Some("deepseek_api_key".to_string()),
        wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
        enabled: true,
    }]);

    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));

    let invocation_view = rendered_rows(&widget, 160, 60).join("\n");
    assert_eq!(invocation_view.contains("Configure provider binding"), true);
    assert_eq!(
        invocation_view.contains("Invocation Method: OpenAI Chat Completions"),
        true
    );
    assert_eq!(invocation_view.contains("> OpenAI Chat Completions"), true);

    widget.handle_key_event(press(KeyCode::Enter));

    let reasoning_view = rendered_rows(&widget, 160, 60).join("\n");
    assert_eq!(reasoning_view.contains("Reason Effort: High"), true);
    assert_eq!(reasoning_view.contains("> High"), true);
    assert_eq!(reasoning_view.contains(" Max"), true);
    assert_eq!(reasoning_view.contains("Medium"), false);
    assert_eq!(reasoning_view.contains("XHigh"), false);

    widget.handle_key_event(press(KeyCode::Enter));

    let command = next_shell_command(&mut app_event_rx);
    let payload = command
        .strip_prefix("onboard ")
        .expect("onboard command prefix");
    let payload: serde_json::Value = serde_json::from_str(payload).expect("valid onboarding json");

    assert_eq!(
        payload["provider_credential_id"],
        serde_json::Value::String("deepseek_api_key".to_string())
    );
    assert_eq!(
        payload["default_reasoning_effort"],
        serde_json::Value::String("high".to_string())
    );
    assert_eq!(payload["api_key"], serde_json::Value::Null);
}
