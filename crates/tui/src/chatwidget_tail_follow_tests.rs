use std::path::PathBuf;

use devo_protocol::ItemId;
use devo_protocol::Model;
use ratatui::text::Line;
use tokio::sync::mpsc;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::chatwidget::ChatWidget;
use crate::chatwidget::ChatWidgetInit;
use crate::chatwidget::TuiSessionState;
use crate::events::TextItemKind;
use crate::events::WorkerEvent;
use crate::render::renderable::Renderable;
use crate::tui::frame_requester::FrameRequester;

fn widget_with_model(model: Model, cwd: PathBuf) -> ChatWidget {
    let (app_event_tx, _app_event_rx) = mpsc::unbounded_channel::<AppEvent>();
    ChatWidget::new_with_app_event(ChatWidgetInit {
        frame_requester: FrameRequester::test_dummy(),
        app_event_tx: AppEventSender::new(app_event_tx),
        initial_session: TuiSessionState::new(cwd, Some(model)),
        initial_thinking_selection: None,
        initial_permission_preset: devo_protocol::PermissionPreset::Default,
        initial_user_message: None,
        enhanced_keys_supported: true,
        is_first_run: false,
        available_models: Vec::new(),
        saved_models: Vec::new(),
        show_model_onboarding: false,
        startup_tooltip_override: None,
        initial_theme_name: None,
    })
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

fn line_text(line: &Line<'static>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn drain_assistant_stream(widget: &mut ChatWidget) {
    for _ in 0..16 {
        if widget.assistant_stream_queued_lines_for_test() == 0 {
            break;
        }
        widget.pre_draw_tick();
    }
}

#[test]
fn overflowing_live_assistant_viewport_follows_latest_tail() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let mut widget = widget_with_model(model, cwd);
    let assistant_id = ItemId::new();

    widget.handle_worker_event(WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        thinking: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_worker_event(WorkerEvent::TextItemStarted {
        item_id: assistant_id,
        kind: TextItemKind::Assistant,
    });

    for index in 0..28 {
        widget.handle_worker_event(WorkerEvent::TextItemDelta {
            item_id: assistant_id,
            kind: TextItemKind::Assistant,
            delta: format!("stream-tail-line-{index:02}\n"),
        });
        widget.pre_draw_tick();
        drain_assistant_stream(&mut widget);
    }

    let live_tail = widget
        .active_viewport_lines_for_area_for_test(/*width*/ 80, /*height*/ 8)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        live_tail.contains("stream-tail-line-27"),
        "expected latest assistant token line in live tail:\n{live_tail}"
    );
    assert!(
        !live_tail.contains("stream-tail-line-00"),
        "expected overflowing live viewport to drop earliest lines:\n{live_tail}"
    );

    let rows = rendered_rows(&widget, 80, 12).join("\n");
    assert!(
        rows.contains("stream-tail-line-27"),
        "expected rendered viewport to show latest assistant token line:\n{rows}"
    );
}
