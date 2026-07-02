//! Read-only sub-agent selection and transcript projection for `ChatWidget`.
//!
//! Worker events update per-child transcript state. Ctrl+X opens a lightweight
//! selector for live direct children, and Enter asks the host overlay to render
//! the selected child through the normal transcript pager.

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;
use std::time::Instant;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use devo_core::ItemId;
use devo_core::SessionId;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

use crate::ansi_escape::ansi_escape_line;
use crate::app_event::AppEvent;
use crate::events::PlanStepStatus;
use crate::events::SubagentMonitorAgent;
use crate::events::SubagentMonitorEvent;
use crate::events::TextItemKind;
use crate::history_cell;
use crate::history_cell::HistoryCell;
use crate::tool_result_cell::ToolResultCell;

use super::ActiveCellTranscriptKey;
use super::ChatWidget;
use super::DotStatus;
use super::TranscriptOverlayCell;
use super::UserMessage;
use super::subagent_live_list;
use super::subagent_live_list::SubagentLiveListRow;
use super::subagent_live_list::SubagentLiveListRowKey;

const SUBAGENT_INACTIVE_GRACE: Duration = Duration::from_secs(12);
const SUBAGENT_CTRL_X_REVEAL: Duration = Duration::from_secs(15);

#[derive(Debug)]
pub(super) struct SubagentMonitorState {
    live_list_focused: bool,
    list_reveal_until: Option<Instant>,
    agents: Vec<SubagentMonitorAgent>,
    selected: Option<SessionId>,
    user_selected: bool,
    sessions: HashMap<SessionId, SubagentSessionView>,
    live_list_rows_dirty: Cell<bool>,
    live_list_rows_cache: RefCell<Vec<SubagentLiveListRow>>,
}

impl Default for SubagentMonitorState {
    fn default() -> Self {
        Self {
            live_list_focused: false,
            list_reveal_until: None,
            agents: Vec::new(),
            selected: None,
            user_selected: false,
            sessions: HashMap::new(),
            live_list_rows_dirty: Cell::new(true),
            live_list_rows_cache: RefCell::new(Vec::new()),
        }
    }
}

#[derive(Debug, Default)]
struct SubagentSessionView {
    agent: Option<SubagentMonitorAgent>,
    status: String,
    transcript: Vec<MonitorTranscriptItem>,
    active_text: HashMap<String, MonitorTextItem>,
    active_tools: HashMap<String, MonitorToolItem>,
    active_turn: Option<devo_core::TurnId>,
    latest_preview: String,
    has_runtime_update: bool,
    last_activity_at: Option<Instant>,
    revision: u64,
}

#[derive(Debug)]
struct MonitorTextItem {
    kind: TextItemKind,
    text: String,
    preview_tail: String,
}

#[derive(Debug)]
struct MonitorToolItem {
    title: String,
    output: String,
    is_error: bool,
}

#[derive(Clone, Copy, Debug)]
enum MonitorTranscriptKind {
    User,
    Assistant,
    Reasoning,
    Tool,
    Plan,
    Status,
}

#[derive(Debug)]
struct MonitorTranscriptItem {
    kind: MonitorTranscriptKind,
    title: String,
    body: String,
    is_error: bool,
}

impl ChatWidget {
    pub(super) fn is_subagent_live_list_focused(&self) -> bool {
        self.subagent_monitor.live_list_focused
    }

    pub(super) fn focus_subagent_live_list(&mut self) {
        if self.subagent_monitor.agents.is_empty() {
            self.subagent_monitor.live_list_focused = false;
            self.set_status_message("No active sub-agents");
            self.frame_requester.schedule_frame();
            return;
        }

        let now = Instant::now();
        if !self.has_visible_subagent_list(now) {
            self.subagent_monitor.list_reveal_until = Some(now + SUBAGENT_CTRL_X_REVEAL);
            self.frame_requester
                .schedule_frame_in(SUBAGENT_CTRL_X_REVEAL);
        }

        self.subagent_monitor.live_list_focused = true;
        self.ensure_visible_subagent_selected(now);
        self.set_status_message("Select sub-agent");
        self.frame_requester.schedule_frame();
    }

    pub(super) fn tick_subagent_monitor(&mut self, now: Instant) {
        let reveal_expired = self
            .subagent_monitor
            .list_reveal_until
            .is_some_and(|until| until <= now);
        if reveal_expired {
            self.subagent_monitor.list_reveal_until = None;
            self.sync_subagent_hint(now);
            self.frame_requester.schedule_frame();
            return;
        }

        let had_visible = self.has_visible_subagent_list(now);
        self.sync_subagent_hint(now);
        if had_visible
            && !self.has_visible_subagent_list(now)
            && self.subagent_monitor.agents.iter().any(|agent| {
                self.subagent_monitor
                    .sessions
                    .contains_key(&agent.session_id)
            })
        {
            self.frame_requester
                .schedule_frame_in(SUBAGENT_INACTIVE_GRACE);
        }
    }

    pub(super) fn handle_subagent_live_list_key_event(&mut self, key: KeyEvent) {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.subagent_monitor.live_list_focused = false;
                self.set_status_message("Ready");
                self.frame_requester.schedule_frame();
            }
            KeyCode::Up => {
                self.select_relative_live_subagent(-1);
            }
            KeyCode::Down => {
                self.select_relative_live_subagent(1);
            }
            KeyCode::Enter => {
                if let Some(session_id) = self.selected_live_subagent() {
                    self.subagent_monitor.live_list_focused = false;
                    self.app_event_tx
                        .send(AppEvent::OpenSubagentOverlay { session_id });
                    self.frame_requester.schedule_frame();
                }
            }
            KeyCode::Backspace
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Char(_)
            | KeyCode::F(_)
            | KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Delete
            | KeyCode::Insert
            | KeyCode::Null
            | KeyCode::CapsLock
            | KeyCode::ScrollLock
            | KeyCode::NumLock
            | KeyCode::PrintScreen
            | KeyCode::Pause
            | KeyCode::Menu
            | KeyCode::KeypadBegin
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::Media(_)
            | KeyCode::Modifier(_) => {}
        }
    }

    pub(super) fn invalidate_subagent_live_list_cache(&mut self) {
        self.subagent_monitor.live_list_rows_dirty.set(true);
    }

    pub(super) fn subagent_live_list_desired_height(&self) -> u16 {
        subagent_live_list::desired_height(self.subagent_live_list_rows().len())
    }

    pub(super) fn render_subagent_live_list(&self, area: Rect, buf: &mut Buffer) {
        let rows = self.subagent_live_list_rows();
        subagent_live_list::render(
            area,
            buf,
            &rows,
            self.subagent_monitor.selected,
            self.subagent_monitor.live_list_focused,
            self.active_accent_color(),
        );
    }

    pub(crate) fn on_subagent_discovered(&mut self, agent: SubagentMonitorAgent) {
        tracing::debug!(
            target: "devo_tui::subagent",
            session_id = ?agent.session_id,
            parent_session_id = ?agent.parent_session_id,
            nickname = %agent.nickname,
            status = %agent.status,
            "subagent discovered by chat widget"
        );
        self.upsert_subagent(agent);
        self.sync_subagent_hint(Instant::now());
        self.invalidate_subagent_live_list_cache();
        self.frame_requester.schedule_frame();
    }

    pub(crate) fn on_subagent_monitor_event(&mut self, event: SubagentMonitorEvent) {
        let session_id = event.session_id();
        let event_kind = subagent_monitor_event_kind(&event);
        let old_preview = self
            .subagent_monitor
            .sessions
            .get(&session_id)
            .map(|view| view.latest_preview.clone());
        let old_status = self
            .subagent_monitor
            .sessions
            .get(&session_id)
            .map(|view| view.status.clone());
        let status = {
            let view = self
                .subagent_monitor
                .sessions
                .entry(session_id)
                .or_default();
            view.apply_event(event);
            view.status.clone()
        };

        let updated_agent = self
            .subagent_monitor
            .agents
            .iter_mut()
            .find(|agent| agent.session_id == session_id)
            .map(|agent| {
                agent.status = status.clone();
                agent.clone()
            });
        if let Some(updated_agent) = updated_agent
            && let Some(view) = self.subagent_monitor.sessions.get_mut(&session_id)
        {
            view.agent = Some(updated_agent);
        }

        self.sync_subagent_hint(Instant::now());
        let view = self.subagent_monitor.sessions.get(&session_id);
        let latest_preview = view.map(|view| view.latest_preview.as_str()).unwrap_or("");
        let has_runtime_update = view.is_some_and(|view| view.has_runtime_update);
        let transcript_len = view.map(|view| view.transcript.len()).unwrap_or(0);
        tracing::debug!(
            target: "devo_tui::subagent",
            ?session_id,
            event_kind,
            old_status = old_status.as_deref().unwrap_or(""),
            new_status = %status,
            preview_changed = old_preview.as_deref() != Some(latest_preview),
            latest_preview,
            has_runtime_update,
            transcript_len,
            live_count = self.live_subagent_ids().len(),
            selected = ?self.subagent_monitor.selected,
            focused = self.subagent_monitor.live_list_focused,
            "subagent monitor event applied"
        );
        self.invalidate_subagent_live_list_cache();
        let now = Instant::now();
        if self.subagent_monitor.live_list_focused || self.has_visible_subagent_list(now) {
            self.frame_requester.schedule_frame();
        }
    }

    pub(crate) fn reset_subagent_monitor(&mut self) {
        self.subagent_monitor = SubagentMonitorState::default();
        self.bottom_pane.set_subagent_hint_visible(false);
    }

    pub(crate) fn subagent_overlay_title(&self, session_id: SessionId) -> Option<String> {
        let view = self.subagent_monitor.sessions.get(&session_id)?;
        let status = view.status.as_str();
        let label = view
            .agent
            .as_ref()
            .map(|agent| agent.nickname.as_str())
            .or_else(|| {
                self.subagent_agent(session_id)
                    .map(|agent| agent.nickname.as_str())
            })
            .unwrap_or("sub-agent");
        Some(format!("{label} [{status}]"))
    }

    pub(crate) fn subagent_transcript_overlay_cell_count(
        &self,
        session_id: SessionId,
    ) -> Option<usize> {
        let view = self.subagent_monitor.sessions.get(&session_id)?;
        Some(view.transcript.len())
    }

    pub(crate) fn subagent_transcript_overlay_cells(
        &self,
        session_id: SessionId,
        width: u16,
    ) -> Option<Vec<TranscriptOverlayCell>> {
        let view = self.subagent_monitor.sessions.get(&session_id)?;
        Some(
            view.transcript
                .iter()
                .map(|item| self.subagent_transcript_item_cell(item, width))
                .collect(),
        )
    }

    pub(crate) fn subagent_transcript_overlay_live_tail_key(
        &self,
        session_id: SessionId,
    ) -> Option<ActiveCellTranscriptKey> {
        let view = self.subagent_monitor.sessions.get(&session_id)?;
        view.has_live_tail().then_some(ActiveCellTranscriptKey {
            revision: view.revision,
            is_stream_continuation: false,
            animation_tick: None,
        })
    }

    pub(crate) fn subagent_transcript_overlay_live_tail_lines(
        &self,
        session_id: SessionId,
        width: u16,
    ) -> Option<Vec<Line<'static>>> {
        let view = self.subagent_monitor.sessions.get(&session_id)?;
        view.has_live_tail()
            .then(|| self.subagent_live_tail_lines(view, width))
    }

    #[cfg(test)]
    pub(crate) fn expire_subagent_inactivity_for_test(&mut self) {
        let expired = Instant::now()
            .checked_sub(SUBAGENT_INACTIVE_GRACE + Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
        for view in self.subagent_monitor.sessions.values_mut() {
            view.last_activity_at = Some(expired);
        }
        self.subagent_monitor.list_reveal_until = None;
        self.sync_subagent_hint(Instant::now());
    }

    #[cfg(test)]
    pub(crate) fn is_subagent_monitor_open_for_test(&self) -> bool {
        self.subagent_monitor.live_list_focused
    }

    #[cfg(test)]
    pub(crate) fn selected_subagent_for_test(&self) -> Option<SessionId> {
        self.subagent_monitor.selected
    }

    #[cfg(test)]
    pub(crate) fn has_live_subagents_for_test(&self) -> bool {
        self.has_live_subagents()
    }

    fn upsert_subagent(&mut self, agent: SubagentMonitorAgent) {
        let session_id = agent.session_id;
        if let Some(existing) = self
            .subagent_monitor
            .agents
            .iter_mut()
            .find(|existing| existing.session_id == session_id)
        {
            *existing = agent.clone();
        } else {
            self.subagent_monitor.agents.push(agent.clone());
        }

        let view = self
            .subagent_monitor
            .sessions
            .entry(session_id)
            .or_default();
        let previous_status = view.status.clone();
        view.status = agent.status.clone();
        if is_terminal_status(&previous_status) && !is_terminal_status(&view.status) {
            view.status = previous_status;
        }
        view.agent = Some(agent.clone());
        if is_active_subagent_status(&normalize_subagent_display_status(&agent.status)) {
            view.touch_activity();
            if !is_terminal_status(&view.status) {
                view.status = "working".to_string();
            }
        }
        if let Some(message) = agent.last_task_message.as_deref() {
            view.seed_initial_task_message(message);
        }
        self.invalidate_subagent_live_list_cache();
    }

    fn sync_subagent_hint(&mut self, now: Instant) {
        let has_active = self.has_active_subagents();
        let has_visible = self.has_visible_subagent_list(now);
        if !has_visible {
            self.subagent_monitor.live_list_focused = false;
            self.subagent_monitor.selected = None;
            self.subagent_monitor.user_selected = false;
        } else {
            self.ensure_visible_subagent_selected(now);
        }
        self.bottom_pane.set_subagent_hint_visible(has_active);
        self.prune_terminal_subagents(now);
        tracing::debug!(
            target: "devo_tui::subagent",
            has_active,
            has_visible,
            live_count = self.active_subagent_ids().len(),
            visible_count = self.visible_subagent_ids(now).len(),
            selected = ?self.subagent_monitor.selected,
            focused = self.subagent_monitor.live_list_focused,
            user_selected = self.subagent_monitor.user_selected,
            "subagent live hint synced"
        );
    }

    fn prune_terminal_subagents(&mut self, now: Instant) {
        let mut removed = Vec::new();
        self.subagent_monitor.agents.retain(|agent| {
            let Some(view) = self.subagent_monitor.sessions.get(&agent.session_id) else {
                return false;
            };
            if view.active_turn.is_some() || view.has_live_tail() {
                return true;
            }
            if is_active_subagent_status(&normalize_subagent_display_status(&view.status)) {
                return true;
            }
            if !is_terminal_status(&view.status) {
                return true;
            }
            let retain = view
                .last_activity_at
                .is_some_and(|at| now.duration_since(at) < SUBAGENT_INACTIVE_GRACE);
            if !retain {
                removed.push(agent.session_id);
            }
            retain
        });
        let had_removals = !removed.is_empty();
        for session_id in removed {
            self.subagent_monitor.sessions.remove(&session_id);
            if self.subagent_monitor.selected == Some(session_id) {
                self.subagent_monitor.selected = None;
                self.subagent_monitor.user_selected = false;
            }
        }
        if had_removals {
            self.invalidate_subagent_live_list_cache();
        }
    }

    fn has_active_subagents(&self) -> bool {
        self.subagent_monitor.agents.iter().any(|agent| {
            is_active_subagent_status(&self.subagent_display_status(agent))
                || self
                    .subagent_monitor
                    .sessions
                    .get(&agent.session_id)
                    .is_some_and(SubagentSessionView::has_live_tail)
        })
    }

    fn has_visible_subagent_list(&self, now: Instant) -> bool {
        if self
            .subagent_monitor
            .list_reveal_until
            .is_some_and(|until| until > now)
        {
            return !self.subagent_monitor.agents.is_empty();
        }
        self.subagent_monitor
            .agents
            .iter()
            .any(|agent| self.is_agent_visible_in_list(agent, now))
    }

    fn is_agent_visible_in_list(&self, agent: &SubagentMonitorAgent, now: Instant) -> bool {
        if is_active_subagent_status(&self.subagent_display_status(agent)) {
            return true;
        }
        let Some(view) = self.subagent_monitor.sessions.get(&agent.session_id) else {
            return false;
        };
        if view.has_live_tail() {
            return true;
        }
        view.last_activity_at
            .is_some_and(|at| now.duration_since(at) < SUBAGENT_INACTIVE_GRACE)
    }

    fn has_live_subagents(&self) -> bool {
        self.has_active_subagents()
    }

    fn selected_live_subagent(&self) -> Option<SessionId> {
        let selected = self.subagent_monitor.selected?;
        let now = Instant::now();
        self.subagent_agent(selected)
            .filter(|agent| self.is_agent_visible_in_list(agent, now))
            .map(|agent| agent.session_id)
    }

    fn ensure_visible_subagent_selected(&mut self, now: Instant) {
        if self.selected_live_subagent().is_some()
            && (self.subagent_monitor.live_list_focused || self.subagent_monitor.user_selected)
        {
            return;
        }
        self.subagent_monitor.selected = self
            .visible_subagent_ids(now)
            .last()
            .copied()
            .or_else(|| self.active_subagent_ids().last().copied());
        self.subagent_monitor.user_selected = false;
    }

    fn ensure_live_subagent_selected(&mut self) {
        self.ensure_visible_subagent_selected(Instant::now());
    }

    fn select_relative_live_subagent(&mut self, delta: isize) {
        let now = Instant::now();
        let visible_ids = self.visible_subagent_ids(now);
        if visible_ids.is_empty() {
            return;
        }
        let current = self
            .subagent_monitor
            .selected
            .and_then(|selected| {
                visible_ids
                    .iter()
                    .position(|session_id| *session_id == selected)
            })
            .unwrap_or(0);
        let next = if delta.is_negative() {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            current
                .saturating_add(delta as usize)
                .min(visible_ids.len().saturating_sub(1))
        };
        self.subagent_monitor.selected = Some(visible_ids[next]);
        self.subagent_monitor.user_selected = true;
        self.invalidate_subagent_live_list_cache();
        self.frame_requester.schedule_frame();
    }

    fn active_subagent_ids(&self) -> Vec<SessionId> {
        self.subagent_monitor
            .agents
            .iter()
            .filter(|agent| {
                is_active_subagent_status(&self.subagent_display_status(agent))
                    || self
                        .subagent_monitor
                        .sessions
                        .get(&agent.session_id)
                        .is_some_and(SubagentSessionView::has_live_tail)
            })
            .map(|agent| agent.session_id)
            .collect()
    }

    fn visible_subagent_ids(&self, now: Instant) -> Vec<SessionId> {
        let mut indexed = self
            .subagent_monitor
            .agents
            .iter()
            .enumerate()
            .filter(|(_, agent)| self.is_agent_visible_in_list(agent, now))
            .collect::<Vec<_>>();
        indexed.sort_by(|(left_index, left), (right_index, right)| {
            let left_active = is_active_subagent_status(&self.subagent_display_status(left))
                || self
                    .subagent_monitor
                    .sessions
                    .get(&left.session_id)
                    .is_some_and(SubagentSessionView::has_live_tail);
            let right_active = is_active_subagent_status(&self.subagent_display_status(right))
                || self
                    .subagent_monitor
                    .sessions
                    .get(&right.session_id)
                    .is_some_and(SubagentSessionView::has_live_tail);
            match (left_active, right_active) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => left_index.cmp(right_index),
            }
        });
        indexed
            .into_iter()
            .map(|(_, agent)| agent.session_id)
            .collect()
    }

    fn live_subagent_ids(&self) -> Vec<SessionId> {
        self.active_subagent_ids()
    }

    fn subagent_live_list_rows(&self) -> std::cell::Ref<'_, Vec<SubagentLiveListRow>> {
        if self.subagent_monitor.live_list_rows_dirty.get() {
            let rows = self.compute_subagent_live_list_rows();
            *self.subagent_monitor.live_list_rows_cache.borrow_mut() = rows;
            self.subagent_monitor.live_list_rows_dirty.set(false);
        }
        self.subagent_monitor.live_list_rows_cache.borrow()
    }

    fn compute_subagent_live_list_rows(&self) -> Vec<SubagentLiveListRow> {
        let now = Instant::now();
        let mut rows = self
            .visible_subagent_ids(now)
            .into_iter()
            .filter_map(|session_id| {
                let agent = self.subagent_agent(session_id)?;
                let view = self.subagent_monitor.sessions.get(&agent.session_id);
                let preview = if let Some(view) = view
                    && view.has_runtime_update
                {
                    single_line_preview(&view.latest_preview)
                        .unwrap_or_else(|| "Waiting for updates".to_string())
                } else {
                    agent
                        .last_task_message
                        .as_deref()
                        .and_then(tail_preview)
                        .unwrap_or_else(|| "Waiting for updates".to_string())
                };
                Some(SubagentLiveListRow {
                    key: SubagentLiveListRowKey::Session(agent.session_id),
                    name: agent.nickname.clone(),
                    status: self.subagent_display_status(agent),
                    preview,
                })
            })
            .collect::<Vec<_>>();
        rows.extend(
            self.research_task_previews
                .iter()
                .map(|preview| SubagentLiveListRow {
                    key: SubagentLiveListRowKey::Research(preview.item_id),
                    name: preview.title.clone(),
                    status: "working".to_string(),
                    preview: preview.preview.clone(),
                }),
        );
        rows
    }

    fn subagent_agent(&self, session_id: SessionId) -> Option<&SubagentMonitorAgent> {
        self.subagent_monitor
            .agents
            .iter()
            .find(|agent| agent.session_id == session_id)
    }

    fn subagent_status_for_agent(&self, agent: &SubagentMonitorAgent) -> String {
        self.subagent_monitor
            .sessions
            .get(&agent.session_id)
            .map(|view| view.status.clone())
            .filter(|status| !status.is_empty())
            .unwrap_or_else(|| agent.status.clone())
    }

    fn subagent_display_status(&self, agent: &SubagentMonitorAgent) -> String {
        let stored = self.subagent_status_for_agent(agent);
        let normalized = normalize_subagent_display_status(&stored);
        if is_terminal_status(&normalized) {
            return normalized;
        }
        let Some(view) = self.subagent_monitor.sessions.get(&agent.session_id) else {
            return normalized;
        };
        if view.has_live_tail() || view.active_turn.is_some() {
            return "working".to_string();
        }
        normalized
    }

    fn subagent_transcript_item_cell(
        &self,
        item: &MonitorTranscriptItem,
        width: u16,
    ) -> TranscriptOverlayCell {
        let lines = match item.kind {
            MonitorTranscriptKind::User => Vec::new(),
            MonitorTranscriptKind::Assistant => history_cell::AgentMarkdownCell::new(
                item.body.clone(),
                &self.session.cwd,
                Self::completed_dot_prefix(),
                "  ",
            )
            .transcript_lines(width),
            MonitorTranscriptKind::Reasoning => history_cell::AgentMessageCell::new_with_prefix(
                titled_body_lines(&item.title, &item.body),
                Self::reasoning_dot_prefix(DotStatus::Completed),
                "  ",
                false,
            )
            .transcript_lines(width),
            MonitorTranscriptKind::Tool => {
                let title_line = (!item.title.is_empty()).then(|| Self::ran_tool_line(&item.title));
                ToolResultCell::new(
                    title_line,
                    item.body.clone(),
                    self.dot_prefix(if item.is_error {
                        DotStatus::Failed
                    } else {
                        DotStatus::Completed
                    }),
                    Line::from("  "),
                    Self::tool_text_style(),
                    false,
                )
                .transcript_lines(width)
            }
            MonitorTranscriptKind::Plan | MonitorTranscriptKind::Status => {
                let dot_prefix = if item.is_error {
                    Self::failed_dot_prefix()
                } else {
                    Self::completed_dot_prefix()
                };
                history_cell::AgentMessageCell::new_with_prefix(
                    titled_body_lines(&item.title, &item.body),
                    dot_prefix,
                    "  ",
                    false,
                )
                .transcript_lines(width)
            }
        };

        TranscriptOverlayCell {
            lines,
            is_stream_continuation: false,
            user_message: matches!(item.kind, MonitorTranscriptKind::User)
                .then(|| UserMessage::from(item.body.clone())),
            is_selected_user: false,
        }
    }

    fn subagent_live_tail_lines(
        &self,
        view: &SubagentSessionView,
        width: u16,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let mut text_keys = view.active_text.keys().cloned().collect::<Vec<_>>();
        text_keys.sort();
        for key in text_keys {
            let Some(text) = view.active_text.get(&key) else {
                continue;
            };
            let next = match text.kind {
                TextItemKind::Assistant => history_cell::AgentMarkdownCell::new(
                    text.text.clone(),
                    &self.session.cwd,
                    Self::pending_dot_prefix(),
                    "  ",
                )
                .transcript_lines(width),
                TextItemKind::Reasoning => history_cell::AgentMessageCell::new_with_prefix(
                    titled_body_lines("Reasoning", &text.text),
                    Self::reasoning_dot_prefix(DotStatus::Pending),
                    "  ",
                    false,
                )
                .transcript_lines(width),
                TextItemKind::ResearchArtifact => history_cell::AgentMessageCell::new_with_prefix(
                    titled_body_lines("Research", &text.text),
                    Self::pending_dot_prefix(),
                    "  ",
                    false,
                )
                .transcript_lines(width),
            };
            extend_lines_with_separator(&mut lines, next);
        }

        let mut tool_keys = view.active_tools.keys().cloned().collect::<Vec<_>>();
        tool_keys.sort();
        for key in tool_keys {
            let Some(tool) = view.active_tools.get(&key) else {
                continue;
            };
            let mut tool_lines = vec![Line::from(vec![
                Span::styled("Running ", Self::tool_status_running_style()),
                Span::styled(tool.title.clone(), Self::tool_text_style()),
            ])];
            tool_lines.extend(tool_output_lines(&tool.output));
            let next = history_cell::AgentMessageCell::new_with_prefix(
                tool_lines,
                Self::pending_dot_prefix(),
                "  ",
                false,
            )
            .transcript_lines(width);
            extend_lines_with_separator(&mut lines, next);
        }
        lines
    }
}

impl SubagentSessionView {
    fn apply_event(&mut self, event: SubagentMonitorEvent) {
        self.revision = self.revision.wrapping_add(1);
        match event {
            SubagentMonitorEvent::TurnStarted {
                session_id: _,
                turn_id,
            } => {
                self.mark_working();
                self.active_turn = Some(turn_id);
                self.set_latest_preview("Started turn");
            }
            SubagentMonitorEvent::TextItemStarted {
                session_id: _,
                item_id,
                kind,
            } => {
                self.mark_working();
                self.active_text.insert(
                    text_key(Some(item_id), kind),
                    MonitorTextItem {
                        kind,
                        text: String::new(),
                        preview_tail: String::new(),
                    },
                );
                self.set_latest_preview(format!("{} started", text_title(kind)));
            }
            SubagentMonitorEvent::TextItemDelta {
                session_id: _,
                item_id,
                kind,
                delta,
            } => {
                self.mark_working();
                let latest_preview = {
                    let latest = self
                        .active_text
                        .entry(text_key(item_id, kind))
                        .or_insert_with(|| MonitorTextItem {
                            kind,
                            text: String::new(),
                            preview_tail: String::new(),
                        });
                    latest.text.push_str(&delta);
                    update_preview_tail(&mut latest.preview_tail, &delta)
                };
                if let Some(latest_preview) = latest_preview {
                    self.set_latest_preview_tail(latest_preview);
                }
            }
            SubagentMonitorEvent::TextItemCompleted {
                session_id: _,
                item_id,
                kind,
                final_text,
            } => {
                self.active_text.remove(&text_key(item_id, kind));
                self.set_latest_preview_tail(final_text.clone());
                self.transcript.push(MonitorTranscriptItem {
                    kind: transcript_kind_for_text(kind),
                    title: text_title(kind).to_string(),
                    body: final_text,
                    is_error: false,
                });
            }
            SubagentMonitorEvent::ToolCall {
                session_id: _,
                tool_use_id,
                summary,
            }
            | SubagentMonitorEvent::ToolCallUpdated {
                session_id: _,
                tool_use_id,
                summary,
            } => {
                self.mark_working();
                let latest_preview = format!("Running {summary}");
                self.active_tools
                    .entry(tool_use_id)
                    .and_modify(|tool| tool.title = summary.clone())
                    .or_insert_with(|| MonitorToolItem {
                        title: summary.clone(),
                        output: String::new(),
                        is_error: false,
                    });
                self.set_latest_preview_tail(latest_preview);
            }
            SubagentMonitorEvent::ToolOutputDelta {
                session_id: _,
                tool_use_id,
                delta,
            } => {
                self.mark_working();
                let latest_preview = {
                    let tool = self
                        .active_tools
                        .entry(tool_use_id)
                        .or_insert(MonitorToolItem {
                            title: "tool".to_string(),
                            output: String::new(),
                            is_error: false,
                        });
                    tool.output.push_str(&delta);
                    if tool.output.trim().is_empty() {
                        format!("Running {}", tool.title)
                    } else {
                        format!("{}: {}", tool.title, tool.output)
                    }
                };
                self.set_latest_preview_tail(latest_preview);
            }
            SubagentMonitorEvent::ToolResult {
                session_id: _,
                tool_use_id,
                title,
                preview,
                is_error,
            } => {
                self.active_tools.remove(&tool_use_id);
                if preview.trim().is_empty() {
                    self.set_latest_preview_tail(title.clone());
                } else {
                    self.set_latest_preview_tail(preview.clone());
                }
                self.transcript.push(MonitorTranscriptItem {
                    kind: MonitorTranscriptKind::Tool,
                    title,
                    body: preview,
                    is_error,
                });
            }
            SubagentMonitorEvent::PlanUpdated {
                session_id: _,
                explanation,
                steps,
            } => {
                let mut body = explanation.unwrap_or_default();
                for step in steps {
                    if !body.is_empty() {
                        body.push('\n');
                    }
                    body.push_str(match step.status {
                        PlanStepStatus::Pending => "[ ] ",
                        PlanStepStatus::InProgress => "[~] ",
                        PlanStepStatus::Completed => "[x] ",
                        PlanStepStatus::Cancelled => "[-] ",
                    });
                    body.push_str(&step.text);
                }
                if body.trim().is_empty() {
                    self.set_latest_preview("Plan updated");
                } else {
                    self.set_latest_preview_tail(body.clone());
                }
                self.transcript.push(MonitorTranscriptItem {
                    kind: MonitorTranscriptKind::Plan,
                    title: "Plan updated".to_string(),
                    body,
                    is_error: false,
                });
            }
            SubagentMonitorEvent::TurnFinished {
                session_id: _,
                status,
            } => {
                self.touch_activity();
                self.status = normalize_terminal_subagent_status(&status);
                self.active_turn = None;
                self.flush_active_items();
                self.set_latest_preview(format!("Turn {}", self.status));
                self.transcript.push(MonitorTranscriptItem {
                    kind: MonitorTranscriptKind::Status,
                    title: format!("Turn {}", self.status),
                    body: String::new(),
                    is_error: self.status.to_ascii_lowercase().contains("failed"),
                });
            }
            SubagentMonitorEvent::TurnFailed {
                session_id: _,
                message,
            } => {
                self.touch_activity();
                self.status = "failed".to_string();
                self.active_turn = None;
                self.flush_active_items();
                self.set_latest_preview_tail(message.clone());
                self.transcript.push(MonitorTranscriptItem {
                    kind: MonitorTranscriptKind::Status,
                    title: "Turn failed".to_string(),
                    body: message,
                    is_error: true,
                });
            }
            SubagentMonitorEvent::TaskMessage {
                session_id: _,
                message,
            } => {
                self.mark_working();
                self.append_task_message(message);
            }
            SubagentMonitorEvent::SessionStatusChanged {
                session_id: _,
                status,
            } => {
                if is_terminal_status(&self.status) {
                    return;
                }
                let normalized = normalize_runtime_status_for_subagent(status);
                if normalized == "idle" && self.active_turn.is_none() && !self.has_live_tail() {
                    self.touch_activity();
                    self.status = "done".to_string();
                    self.set_latest_preview("Turn done".to_string());
                    return;
                }
                if !is_terminal_status(&normalized) {
                    self.status = normalized;
                    self.set_latest_preview(format!("Status {}", self.status));
                }
            }
        }
    }

    fn touch_activity(&mut self) {
        self.last_activity_at = Some(Instant::now());
    }

    fn mark_working(&mut self) {
        self.touch_activity();
        if !is_terminal_status(&self.status) {
            self.status = "working".to_string();
        }
    }

    fn seed_initial_task_message(&mut self, message: &str) {
        if message.trim().is_empty()
            || self
                .transcript
                .iter()
                .any(|item| matches!(item.kind, MonitorTranscriptKind::User))
        {
            return;
        }
        self.transcript.insert(
            0,
            MonitorTranscriptItem {
                kind: MonitorTranscriptKind::User,
                title: String::new(),
                body: message.to_string(),
                is_error: false,
            },
        );
        self.touch_activity();
    }

    fn append_task_message(&mut self, message: String) {
        if message.trim().is_empty() {
            return;
        }
        if self.transcript.last().is_some_and(|item| {
            matches!(item.kind, MonitorTranscriptKind::User) && item.body == message
        }) {
            return;
        }
        self.transcript.push(MonitorTranscriptItem {
            kind: MonitorTranscriptKind::User,
            title: String::new(),
            body: message,
            is_error: false,
        });
        self.touch_activity();
    }

    fn has_live_tail(&self) -> bool {
        !self.active_text.is_empty() || !self.active_tools.is_empty()
    }

    fn set_latest_preview(&mut self, preview: impl Into<String>) {
        let preview = preview.into();
        if let Some(preview) = single_line_preview(&preview) {
            self.latest_preview = preview;
            self.has_runtime_update = true;
        }
    }

    fn set_latest_preview_tail(&mut self, preview: impl Into<String>) {
        let preview = preview.into();
        if let Some(preview) = tail_preview(&preview) {
            self.latest_preview = preview;
            self.has_runtime_update = true;
        }
    }

    fn flush_active_items(&mut self) {
        for text in self.active_text.drain().map(|(_, text)| text) {
            if !text.text.trim().is_empty() {
                self.transcript.push(MonitorTranscriptItem {
                    kind: transcript_kind_for_text(text.kind),
                    title: text_title(text.kind).to_string(),
                    body: text.text,
                    is_error: false,
                });
            }
        }
        for tool in self.active_tools.drain().map(|(_, tool)| tool) {
            self.transcript.push(MonitorTranscriptItem {
                kind: MonitorTranscriptKind::Tool,
                title: tool.title,
                body: tool.output,
                is_error: tool.is_error,
            });
        }
    }
}

impl SubagentMonitorEvent {
    fn session_id(&self) -> SessionId {
        match self {
            Self::TurnStarted { session_id, .. }
            | Self::TextItemStarted { session_id, .. }
            | Self::TextItemDelta { session_id, .. }
            | Self::TextItemCompleted { session_id, .. }
            | Self::ToolCall { session_id, .. }
            | Self::ToolCallUpdated { session_id, .. }
            | Self::ToolOutputDelta { session_id, .. }
            | Self::ToolResult { session_id, .. }
            | Self::PlanUpdated { session_id, .. }
            | Self::TurnFinished { session_id, .. }
            | Self::TurnFailed { session_id, .. }
            | Self::TaskMessage { session_id, .. }
            | Self::SessionStatusChanged { session_id, .. } => *session_id,
        }
    }
}

fn subagent_monitor_event_kind(event: &SubagentMonitorEvent) -> &'static str {
    match event {
        SubagentMonitorEvent::TurnStarted { .. } => "turn_started",
        SubagentMonitorEvent::TextItemStarted { .. } => "text_item_started",
        SubagentMonitorEvent::TextItemDelta { .. } => "text_item_delta",
        SubagentMonitorEvent::TextItemCompleted { .. } => "text_item_completed",
        SubagentMonitorEvent::ToolCall { .. } => "tool_call",
        SubagentMonitorEvent::ToolCallUpdated { .. } => "tool_call_updated",
        SubagentMonitorEvent::ToolOutputDelta { .. } => "tool_output_delta",
        SubagentMonitorEvent::ToolResult { .. } => "tool_result",
        SubagentMonitorEvent::PlanUpdated { .. } => "plan_updated",
        SubagentMonitorEvent::TurnFinished { .. } => "turn_finished",
        SubagentMonitorEvent::TurnFailed { .. } => "turn_failed",
        SubagentMonitorEvent::TaskMessage { .. } => "task_message",
        SubagentMonitorEvent::SessionStatusChanged { .. } => "session_status_changed",
    }
}

fn is_active_subagent_status(status: &str) -> bool {
    matches!(
        status.to_ascii_lowercase().as_str(),
        "working" | "running" | "spawning" | "activeturn"
    )
}

fn normalize_subagent_display_status(status: &str) -> String {
    match status.to_ascii_lowercase().as_str() {
        "running" | "spawning" | "activeturn" => "working".to_string(),
        "completed" | "idle" | "success" => "done".to_string(),
        other => other.to_string(),
    }
}

fn normalize_terminal_subagent_status(status: &str) -> String {
    normalize_subagent_display_status(status)
}

fn normalize_runtime_status_for_subagent(status: devo_protocol::SessionRuntimeStatus) -> String {
    match status {
        devo_protocol::SessionRuntimeStatus::ActiveTurn => "working".to_string(),
        devo_protocol::SessionRuntimeStatus::Idle => "idle".to_string(),
        other => format!("{other:?}").to_lowercase(),
    }
}

fn is_terminal_status(status: &str) -> bool {
    matches!(
        status.to_ascii_lowercase().as_str(),
        "done" | "completed" | "failed" | "cancelled" | "canceled" | "interrupted" | "closed"
    )
}

fn single_line_preview(text: &str) -> Option<String> {
    let preview = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();
    (!preview.is_empty()).then_some(preview)
}

fn tail_preview(text: &str) -> Option<String> {
    let last_line = text
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let normalized = last_line.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return None;
    }
    Some(tail_chars(&normalized, MAX_PREVIEW_TAIL_CHARS))
}

const MAX_PREVIEW_TAIL_CHARS: usize = 80;

fn update_preview_tail(current: &mut String, delta: &str) -> Option<String> {
    if delta.contains('\n') {
        if let Some(last_line) = delta
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty())
        {
            let normalized = last_line.split_whitespace().collect::<Vec<_>>().join(" ");
            *current = tail_chars(&normalized, MAX_PREVIEW_TAIL_CHARS);
        }
    } else {
        current.push_str(delta);
        let normalized = current.split_whitespace().collect::<Vec<_>>().join(" ");
        *current = tail_chars(&normalized, MAX_PREVIEW_TAIL_CHARS);
    }
    (!current.is_empty()).then(|| current.clone())
}

fn tail_chars(text: &str, max_chars: usize) -> String {
    let total_chars = text.chars().count();
    if total_chars <= max_chars {
        return text.to_string();
    }
    text.chars().skip(total_chars - max_chars).collect()
}

fn text_key(item_id: Option<ItemId>, kind: TextItemKind) -> String {
    item_id
        .map(|item_id| item_id.to_string())
        .unwrap_or_else(|| format!("legacy-{kind:?}"))
}

fn text_title(kind: TextItemKind) -> &'static str {
    match kind {
        TextItemKind::Assistant => "Assistant",
        TextItemKind::Reasoning => "Reasoning",
        TextItemKind::ResearchArtifact => "Research",
    }
}

fn transcript_kind_for_text(kind: TextItemKind) -> MonitorTranscriptKind {
    match kind {
        TextItemKind::Assistant => MonitorTranscriptKind::Assistant,
        TextItemKind::Reasoning => MonitorTranscriptKind::Reasoning,
        TextItemKind::ResearchArtifact => MonitorTranscriptKind::Assistant,
    }
}

fn titled_body_lines(title: &str, body: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if !title.is_empty() {
        lines.push(Line::from(title.to_string()).bold());
    }
    lines.extend(tool_output_lines(body));
    lines
}

fn tool_output_lines(body: &str) -> Vec<Line<'static>> {
    body.lines().map(ansi_escape_line).collect()
}

fn extend_lines_with_separator(target: &mut Vec<Line<'static>>, mut next: Vec<Line<'static>>) {
    if next.is_empty() {
        return;
    }

    let should_insert_separator =
        !target.is_empty() && target.last().is_some_and(|line| !is_blank_line(line));
    if should_insert_separator && next.first().is_some_and(|line| !is_blank_line(line)) {
        target.push(Line::from(""));
    }
    target.append(&mut next);
}

fn is_blank_line(line: &Line<'_>) -> bool {
    line.spans.iter().all(|span| span.content.trim().is_empty())
}
