use anyhow::Result;
use crossterm::event::KeyCode;
use crossterm::event::KeyModifiers;
use devo_protocol::Model;
use devo_protocol::ModelCatalog;
use devo_protocol::ProviderWireApi;
use futures::StreamExt;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::mpsc;

use crate::app::AppExit;
use crate::app::InteractiveTuiConfig;
use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::chatwidget::ChatWidget;
use crate::chatwidget::ChatWidgetInit;
use crate::chatwidget::TuiSessionState;
use crate::events::WorkerEvent;
use crate::onboarding::save_last_used_model;
use crate::onboarding::save_onboarding_config;
use crate::render::renderable::Renderable;
use crate::tui::Tui;
use crate::tui::TuiEvent;
use crate::worker::QueryWorkerConfig;
use crate::worker::QueryWorkerHandle;

#[derive(Debug, Clone)]
struct PendingOnboarding {
    provider: ProviderWireApi,
    model: String,
    base_url: Option<String>,
    api_key: Option<String>,
}

#[derive(Debug, Default)]
struct InteractiveLoopState {
    turn_count: usize,
    total_input_tokens: usize,
    total_output_tokens: usize,
    pending_onboarding: Option<PendingOnboarding>,
    // indicate whther LLM worker is working, is started by TurnStarted,
    // it ended by TurnFailed/TurnFinished
    busy: bool,
    last_ctrl_c_at: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopAction {
    Continue,
    Exit,
    ClearAndExit,
}

/// Runs the interactive terminal UI until the user exits or the worker stops.
pub async fn run_interactive_tui(config: InteractiveTuiConfig) -> Result<AppExit> {
    // Build the initial terminal, session, and background worker state.
    let initial_session = config.initial_session.clone();
    let terminal = crate::tui::init()?;
    let mut tui = crate::tui::Tui::new(terminal);

    // spawn a worker with stdio transport with server, it'll emit events
    // such as `[WorkerEvent::TurnStarted]`, `[WorkerEvent::UsageUpdated]` etc.
    let mut worker = QueryWorkerHandle::spawn(QueryWorkerConfig {
        model: initial_session.model.clone(),
        cwd: initial_session.cwd.clone(),
        server_log_level: config.server_log_level,
        thinking_selection: initial_session.thinking_selection.clone(),
    });

    // TODO: Should we change it to bounded channel?
    // app events, such as `[AppEvent::Command]`, `[AppEvent::Exit]`
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let app_event_sender = AppEventSender::new(app_event_tx);

    // Resolve model metadata for the chat widget, falling back to the session slug.
    let available_models = config
        .model_catalog
        .list_visible()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();

    // TODO: there is a check: whether initial_session.model exists at model_catalog, I think the check should
    // outside `run_interactive_tui`, at who invoke run_interactive_tui, check, if not exist, then pick one
    // then update the config.toml file.
    let model = config
        .model_catalog
        .get(&initial_session.model)
        .cloned()
        .unwrap_or_else(|| Model {
            slug: initial_session.model.clone(),
            display_name: initial_session.model.clone(),
            provider: initial_session.provider,
            ..Model::default()
        });
    let cwd = initial_session.cwd.clone();

    // TODO: PnedingOnboarding lack ProviderWireAPI type, such as OpenAI Chat Completions, OpenAI Responses, Anthropic Messages.
    let mut loop_state = InteractiveLoopState::default();

    // Create the root chat widget that owns visible TUI state and input handling.
    let mut chat_widget = ChatWidget::new_with_app_event(ChatWidgetInit {
        frame_requester: tui.frame_requester(),
        app_event_tx: app_event_sender,
        initial_session: TuiSessionState::new(cwd.clone(), Some(model)),
        initial_user_message: None,
        enhanced_keys_supported: tui.enhanced_keys_supported(),
        is_first_run: true,
        available_models,
        show_model_onboarding: config.show_model_onboarding,
        startup_tooltip_override: Some(format!("Ready in {}", cwd.display())),
    });
    if let Some(thinking_selection) = initial_session.thinking_selection {
        chat_widget.set_thinking_selection(Some(thinking_selection));
    }

    // tui events, such as `[TuiEvent::Draw]`, `[TuiEvent::Key]`, `TuiEvent::Paste`
    let events = tui.event_stream();
    tokio::pin!(events);

    tui.frame_requester().schedule_frame();

    // Drive the TUI by racing terminal input, app commands, and worker events.
    loop {
        tokio::select! {
            tui_event = events.next() => {
                match handle_tui_event(
                    tui_event,
                    &mut tui,
                    &worker,
                    &mut chat_widget,
                    &mut loop_state,
                )? {
                    LoopAction::Continue => {}
                    LoopAction::Exit => break,
                    LoopAction::ClearAndExit => {
                        clear_before_exit(&mut tui)?;
                        break;
                    }
                }
            }
            app_event = app_event_rx.recv() => {
                match handle_app_event(
                    app_event,
                    &worker,
                    &mut chat_widget,
                    &config.model_catalog,
                    initial_session.provider,
                    &mut loop_state,
                )? {
                    LoopAction::Continue => {}
                    LoopAction::Exit => break,
                    LoopAction::ClearAndExit => {
                        clear_before_exit(&mut tui)?;
                        break;
                    }
                }
            }
            worker_event = worker.event_rx.recv() => {
                match handle_worker_event(
                    worker_event,
                    &worker,
                    &mut chat_widget,
                    &mut loop_state,
                )? {
                    LoopAction::Continue => {}
                    LoopAction::Exit => break,
                    LoopAction::ClearAndExit => {
                        clear_before_exit(&mut tui)?;
                        break;
                    }
                }
            }
        }
    }

    // Tear down the terminal wrapper before awaiting worker shutdown.
    drop(tui);
    worker.shutdown().await?;
    Ok(AppExit {
        turn_count: loop_state.turn_count,
        total_input_tokens: loop_state.total_input_tokens,
        total_output_tokens: loop_state.total_output_tokens,
    })
}

fn clear_before_exit(tui: &mut Tui) -> Result<()> {
    if tui.is_alt_screen_active() {
        tui.leave_alt_screen()?;
    }
    tui.terminal.clear_scrollback_and_visible_screen_ansi()?;
    Ok(())
}

fn handle_tui_event(
    tui_event: Option<TuiEvent>,
    tui: &mut Tui,
    worker: &QueryWorkerHandle,
    chat_widget: &mut ChatWidget,
    loop_state: &mut InteractiveLoopState,
) -> Result<LoopAction> {
    let Some(tui_event) = tui_event else {
        return Ok(LoopAction::Exit);
    };

    match tui_event {
        TuiEvent::Draw => {
            // Update time-sensitive widget state before measuring or rendering.
            chat_widget.pre_draw_tick();

            if !chat_widget.is_resume_browser_open() && tui.is_alt_screen_active() {
                tui.leave_alt_screen()?;
            }

            // Wrap pending scrollback using the current terminal width.
            let width = tui.terminal.size()?.width.max(1);
            let scrollback_lines = chat_widget.drain_scrollback_lines(width);
            if !scrollback_lines.is_empty() {
                tui.insert_history_lines(scrollback_lines);
            }

            // Size the chat area within the visible terminal and render the frame.
            let height = chat_widget
                .desired_height(width)
                .min(tui.terminal.size()?.height.saturating_sub(1))
                .max(3);
            tui.draw(height, |frame| {
                let area = frame.area();
                chat_widget.render(area, frame.buffer_mut());
                // Restore the terminal cursor to the widget-provided input position.
                if let Some((x, y)) = chat_widget.cursor_pos(area) {
                    frame.set_cursor_position((x, y));
                }
            })?;
        }
        TuiEvent::Key(key) => {
            // Let Ctrl-C interrupt active work first, then require a second press to exit.
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                if loop_state.busy {
                    worker.interrupt_turn()?;
                    chat_widget.set_status_message("Interrupted;");
                } else {
                    let now = Instant::now();
                    if loop_state
                        .last_ctrl_c_at
                        .is_some_and(|last| now.duration_since(last) <= Duration::from_secs(2))
                    {
                        return Ok(LoopAction::ClearAndExit);
                    }
                    loop_state.last_ctrl_c_at = Some(now);
                    chat_widget.set_status_message("Press Ctrl-C again to exit");
                }
            } else {
                loop_state.last_ctrl_c_at = None;
                chat_widget.handle_key_event(key);
            }
        }
        // TODO: Still BUG here, if user pasted long text to the cli, the cli would blow away, take each line as a prompt submit.
        TuiEvent::Paste(pasted) => {
            // Many terminals convert newlines to \r when pasting (e.g., iTerm2),
            // but tui-textarea expects \n. Normalize CR to LF.
            // [tui-textarea]: <https://github.com/rhysd/tui-textarea/blob/4d18622eeac13b309e0ff6a55a46ac6706da68cf/src/textarea.rs#L782-L783>
            // [iTerm2]: <https://github.com/gnachman/iTerm2/blob/5d0c0d9f68523cbd0494dad5422998964a2ecd8d/sources/iTermPasteHelper.m#L206-L216>
            let pasted = pasted.replace("\r", "\n");
            chat_widget.handle_paste(pasted);
        }
    }

    Ok(LoopAction::Continue)
}

fn handle_app_event(
    app_event: Option<AppEvent>,
    worker: &QueryWorkerHandle,
    chat_widget: &mut ChatWidget,
    model_catalog: &impl ModelCatalog,
    default_provider: ProviderWireApi,
    loop_state: &mut InteractiveLoopState,
) -> Result<LoopAction> {
    let Some(app_event) = app_event else {
        return Ok(LoopAction::Exit);
    };

    if let AppEvent::Exit(exit_mode) = &app_event {
        // Shutdown requests may need the normal screen restored before clearing it.
        if matches!(exit_mode, crate::app_event::ExitMode::ShutdownFirst) {
            return Ok(LoopAction::ClearAndExit);
        }
        return Ok(LoopAction::Exit);
    }

    if let AppEvent::Command(command) = &app_event {
        // Commands that affect sessions, providers, or turns are forwarded to the worker.
        handle_app_command(
            command,
            worker,
            chat_widget,
            model_catalog,
            default_provider,
            &mut loop_state.pending_onboarding,
        )?;
    }
    chat_widget.handle_app_event(app_event);

    Ok(LoopAction::Continue)
}

fn handle_worker_event(
    worker_event: Option<WorkerEvent>,
    worker: &QueryWorkerHandle,
    chat_widget: &mut ChatWidget,
    loop_state: &mut InteractiveLoopState,
) -> Result<LoopAction> {
    let Some(worker_event) = worker_event else {
        chat_widget.set_status_message("Background worker stopped");
        return Ok(LoopAction::Exit);
    };

    match &worker_event {
        WorkerEvent::TurnFinished {
            turn_count: next_turn_count,
            total_input_tokens: next_total_input_tokens,
            total_output_tokens: next_total_output_tokens,
            ..
        }
        | WorkerEvent::TurnFailed {
            turn_count: next_turn_count,
            total_input_tokens: next_total_input_tokens,
            total_output_tokens: next_total_output_tokens,
            ..
        } => {
            loop_state.busy = false;
            loop_state.turn_count = *next_turn_count;
            loop_state.total_input_tokens = *next_total_input_tokens;
            loop_state.total_output_tokens = *next_total_output_tokens;
        }
        WorkerEvent::TurnStarted { .. } => {
            loop_state.busy = true;
        }
        WorkerEvent::UsageUpdated {
            total_input_tokens: next_total_input_tokens,
            total_output_tokens: next_total_output_tokens,
        } => {
            loop_state.total_input_tokens = *next_total_input_tokens;
            loop_state.total_output_tokens = *next_total_output_tokens;
        }
        WorkerEvent::ProviderValidationSucceeded { .. } => {
            if let Some(pending) = loop_state.pending_onboarding.take() {
                // Persist successful onboarding before switching the worker provider.
                save_onboarding_config(
                    pending.provider,
                    &pending.model,
                    pending.base_url.as_deref(),
                    pending.api_key.as_deref(),
                )?;
                worker.reconfigure_provider(
                    pending.provider,
                    pending.model,
                    pending.base_url,
                    pending.api_key,
                )?;
            }
        }
        WorkerEvent::ProviderValidationFailed { .. } => {
            loop_state.pending_onboarding = None;
        }
        WorkerEvent::TextDelta(_)
        | WorkerEvent::ReasoningDelta(_)
        | WorkerEvent::AssistantMessageCompleted(_)
        | WorkerEvent::ReasoningCompleted(_)
        | WorkerEvent::ToolCall { .. }
        | WorkerEvent::ToolResult { .. }
        | WorkerEvent::SessionsListed { .. }
        | WorkerEvent::SkillsListed { .. }
        | WorkerEvent::NewSessionPrepared { .. }
        | WorkerEvent::SessionSwitched { .. }
        | WorkerEvent::SessionRenamed { .. }
        | WorkerEvent::SessionTitleUpdated { .. }
        | WorkerEvent::InputHistoryLoaded { .. } => {}
    }
    chat_widget.handle_worker_event(worker_event);

    Ok(LoopAction::Continue)
}

fn handle_app_command(
    command: &AppCommand,
    worker: &QueryWorkerHandle,
    chat_widget: &mut ChatWidget,
    model_catalog: &impl ModelCatalog,
    default_provider: ProviderWireApi,
    pending_onboarding: &mut Option<PendingOnboarding>,
) -> Result<()> {
    match command {
        AppCommand::UserTurn {
            input,
            model,
            thinking,
            ..
        } => {
            if let Some(model) = model {
                worker.set_model(model.clone())?;
            }
            worker.set_thinking(thinking.clone())?;
            let prompt = input
                .iter()
                .filter_map(|item| match item {
                    devo_protocol::InputItem::Text { text } => Some(text.as_str()),
                    devo_protocol::InputItem::Skill { .. }
                    | devo_protocol::InputItem::LocalImage { .. }
                    | devo_protocol::InputItem::Mention { .. } => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            worker.submit_prompt(prompt)?;
        }
        AppCommand::OverrideTurnContext {
            model, thinking, ..
        } => {
            if let Some(model) = model {
                worker.set_model(model.clone())?;
                save_last_used_model(/*wire_api*/ None, default_provider, model)?;
            }
            if let Some(thinking) = thinking {
                worker.set_thinking(thinking.clone())?;
            }
        }
        AppCommand::RunUserShellCommand { command } if command == "session new" => {
            worker.start_new_session()?;
        }
        AppCommand::RunUserShellCommand { command } if command == "session list" => {
            worker.list_sessions()?;
        }
        AppCommand::RunUserShellCommand { command } if command.starts_with("onboard ") => {
            let payload = command.trim_start_matches("onboard ");
            let value: serde_json::Value = serde_json::from_str(payload)?;
            let model = value
                .get("model")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let base_url = value
                .get("base_url")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned);
            let api_key = value
                .get("api_key")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned);
            let provider = model_catalog
                .get(&model)
                .map(Model::provider_wire_api)
                .unwrap_or(default_provider);
            *pending_onboarding = Some(PendingOnboarding {
                provider,
                model: model.clone(),
                base_url: base_url.clone(),
                api_key: api_key.clone(),
            });
            worker.validate_provider(provider, model, base_url, api_key)?;
        }
        AppCommand::BrowseInputHistory { direction } => {
            worker.browse_input_history(*direction)?;
        }
        AppCommand::SwitchSession { session_id } => {
            worker.switch_session(*session_id)?;
        }
        _ => {
            chat_widget.set_status_message(format!("Unsupported command: {}", command.kind()));
        }
    }
    Ok(())
}
