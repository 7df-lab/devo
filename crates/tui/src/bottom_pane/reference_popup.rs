//! Combined `@` reference popup for server-backed fuzzy search.
//!
//! This widget owns only visual state and selection behavior for token-local
//! reference search. The server owns fuzzy filtering and category aggregation
//! for skills, MCP servers, and files.

use std::path::PathBuf;

use crossterm::event::KeyCode;
use devo_protocol::ReferenceSearchResult;
use devo_protocol::ReferenceSearchResultKind;
use devo_protocol::ReferenceSearchSnapshot;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Widget;
use ratatui::widgets::WidgetRef;

use crate::key_hint;
use crate::render::Insets;
use crate::render::RectExt;
use crate::text_formatting::center_truncate_path;
use crate::text_formatting::truncate_text;

use super::popup_consts::MAX_POPUP_ROWS;
use super::scroll_state::ScrollState;
use super::selection_popup_common::GenericDisplayRow;
use super::selection_popup_common::render_rows_single_line;

const REFERENCE_NAME_TRUNCATE_LEN: usize = 34;
/// Minimum display width (columns) reserved for file path rendering in the
/// reference popup. Derived as: area.width - 2 (left inset) - 10 (prefix).
const FILE_NAME_CONTAINER_MIN_WIDTH: usize = 30;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReferenceSelection {
    Skill {
        insert_text: String,
        path: Option<String>,
    },
    Mcp {
        insert_text: String,
        path: String,
    },
    File {
        /// Absolute filesystem path (used for image attach and similar).
        path: PathBuf,
        /// Visible composer token, for example `@interactive.rs`.
        insert_text: String,
        /// Relative workspace path stored in the mention binding.
        mention_path: String,
    },
}

pub(crate) struct ReferencePopup {
    query: String,
    results: Vec<ReferenceSearchResult>,
    waiting: bool,
    state: ScrollState,
    accent_color: Color,
}

impl ReferencePopup {
    pub(crate) fn new(accent_color: Color) -> Self {
        Self {
            query: String::new(),
            results: Vec::new(),
            waiting: true,
            state: ScrollState::new(),
            accent_color,
        }
    }

    pub(crate) fn set_query(&mut self, query: &str) {
        if self.query == query {
            return;
        }

        self.query = query.to_string();
        self.results.clear();
        self.waiting = true;
        self.clamp_selection();
    }

    pub(crate) fn set_snapshot(&mut self, snapshot: ReferenceSearchSnapshot) {
        if snapshot.query != self.query {
            return;
        }

        self.results = snapshot.results.into_iter().take(MAX_POPUP_ROWS).collect();
        self.waiting = !snapshot.file_search_complete;
        self.clamp_selection();
    }

    pub(crate) fn calculate_required_height(&self, _width: u16) -> u16 {
        let visible = self.results.len().clamp(1, MAX_POPUP_ROWS);
        (visible as u16).saturating_add(2)
    }

    pub(crate) fn move_up(&mut self) {
        let len = self.results.len();
        self.state.move_up_wrap(len);
        self.state.ensure_visible(len, MAX_POPUP_ROWS.min(len));
    }

    pub(crate) fn move_down(&mut self) {
        let len = self.results.len();
        self.state.move_down_wrap(len);
        self.state.ensure_visible(len, MAX_POPUP_ROWS.min(len));
    }

    pub(crate) fn selected_reference(&self) -> Option<ReferenceSelection> {
        let idx = self.state.selected_idx?;
        let selected = self.results.get(idx)?;
        match selected.kind {
            ReferenceSearchResultKind::Skill => Some(ReferenceSelection::Skill {
                insert_text: selected.insert_text.clone(),
                path: selected.mention_path.clone(),
            }),
            ReferenceSearchResultKind::Mcp => {
                let path = selected.mention_path.clone()?;
                Some(ReferenceSelection::Mcp {
                    insert_text: selected.insert_text.clone(),
                    path,
                })
            }
            ReferenceSearchResultKind::File => {
                let mention_path = selected
                    .mention_path
                    .clone()
                    .unwrap_or_else(|| selected.display_name.clone());
                Some(ReferenceSelection::File {
                    path: selected
                        .file_path
                        .clone()
                        .unwrap_or_else(|| PathBuf::from(&mention_path)),
                    insert_text: selected.insert_text.clone(),
                    mention_path,
                })
            }
        }
    }

    fn clamp_selection(&mut self) {
        let len = self.results.len();
        self.state.clamp_selection(len);
        self.state.ensure_visible(len, MAX_POPUP_ROWS.min(len));
    }

    fn rows(&self, name_container_width: usize) -> Vec<GenericDisplayRow> {
        self.results
            .iter()
            .map(|result| {
                let name = match result.kind {
                    ReferenceSearchResultKind::File => {
                        center_truncate_path(&result.display_name, name_container_width)
                    }
                    _ => truncate_text(&result.display_name, REFERENCE_NAME_TRUNCATE_LEN),
                };
                GenericDisplayRow {
                    name,
                    name_prefix_spans: vec![category_prefix(result.kind)],
                    match_indices: result.match_indices.clone(),
                    display_shortcut: None,
                    description: result.description.clone(),
                    category_tag: None,
                    is_disabled: result.is_disabled,
                    disabled_reason: result.disabled_reason.clone(),
                    wrap_indent: None,
                }
            })
            .collect()
    }
}

fn category_prefix(kind: ReferenceSearchResultKind) -> Span<'static> {
    let (label, color) = category_prefix_parts(kind);
    Span::styled(format!("{label} "), Style::default().fg(color))
}

fn category_prefix_parts(kind: ReferenceSearchResultKind) -> (&'static str, Color) {
    match kind {
        ReferenceSearchResultKind::Skill => (" [▦ Skill]", Color::LightCyan),
        ReferenceSearchResultKind::Mcp => (" [⬡ MCP]", Color::LightMagenta),
        ReferenceSearchResultKind::File => (" [≣ FILE]", Color::LightYellow),
    }
}

impl WidgetRef for ReferencePopup {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        let (list_area, hint_area) = if area.height > 2 {
            let [list_area, _spacer_area, hint_area] = Layout::vertical([
                Constraint::Length(area.height - 2),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .areas(area);
            (list_area, Some(hint_area))
        } else {
            (area, None)
        };
        // Available columns for the name: area width minus 2 (left inset in
        // render_rows_single_line) minus 10 (prefix like " [≣ FILE] ").
        let name_container_width = (area.width as usize)
            .saturating_sub(12)
            .max(FILE_NAME_CONTAINER_MIN_WIDTH);
        let rows = self.rows(name_container_width);
        let empty_message = if self.waiting {
            "loading..."
        } else {
            "no matches"
        };
        render_rows_single_line(
            list_area.inset(Insets::tlbr(
                /*top*/ 0, /*left*/ 2, /*bottom*/ 0, /*right*/ 0,
            )),
            buf,
            &rows,
            &self.state,
            MAX_POPUP_ROWS,
            empty_message,
            self.accent_color,
        );
        if let Some(hint_area) = hint_area {
            let hint_area = Rect {
                x: hint_area.x + 2,
                y: hint_area.y,
                width: hint_area.width.saturating_sub(2),
                height: hint_area.height,
            };
            reference_popup_hint_line().render(hint_area, buf);
        }
    }
}

fn reference_popup_hint_line() -> Line<'static> {
    Line::from(vec![
        "Press ".into(),
        key_hint::plain(KeyCode::Enter).into(),
        " to insert or ".into(),
        key_hint::plain(KeyCode::Esc).into(),
        " to close".into(),
    ])
}

#[cfg(test)]
mod tests {
    use devo_protocol::ReferenceSearchId;
    use pretty_assertions::assert_eq;

    use super::*;

    /// Default name container width used in tests.
    const TEST_NAME_WIDTH: usize = 60;

    fn result(kind: ReferenceSearchResultKind, display_name: &str) -> ReferenceSearchResult {
        let insert_text = match kind {
            ReferenceSearchResultKind::Skill => format!("@{display_name}"),
            ReferenceSearchResultKind::Mcp => format!("@mcp:{}", display_name.to_ascii_lowercase()),
            ReferenceSearchResultKind::File => {
                let basename = std::path::Path::new(display_name)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(display_name);
                format!("@{basename}")
            }
        };
        ReferenceSearchResult {
            kind,
            display_name: display_name.to_string(),
            description: None,
            insert_text,
            mention_path: match kind {
                ReferenceSearchResultKind::Skill => Some(format!("skills/{display_name}/SKILL.md")),
                ReferenceSearchResultKind::Mcp => Some(format!(
                    "mcp://server/{}",
                    display_name.to_ascii_lowercase()
                )),
                ReferenceSearchResultKind::File => Some(display_name.to_string()),
            },
            file_path: (kind == ReferenceSearchResultKind::File)
                .then(|| PathBuf::from(display_name)),
            match_indices: None,
            is_disabled: false,
            disabled_reason: None,
        }
    }

    fn snapshot(query: &str, results: Vec<ReferenceSearchResult>) -> ReferenceSearchSnapshot {
        ReferenceSearchSnapshot {
            search_id: ReferenceSearchId::new(),
            query: query.to_string(),
            results,
            total_file_match_count: 0,
            scanned_file_count: 0,
            file_search_complete: true,
        }
    }

    fn row_name_prefix(row: &GenericDisplayRow) -> Option<(String, Option<Color>)> {
        row.name_prefix_spans
            .first()
            .map(|span| (span.content.to_string(), span.style.fg))
    }

    /// Trace: L2-DES-CLIENT-002
    /// Verifies: Empty query results keep skill, MCP, then file category ordering with category prefixes.
    #[test]
    fn empty_query_orders_skill_mcp_then_file() {
        let mut popup = ReferencePopup::new(Color::Cyan);
        popup.set_query("");
        popup.set_snapshot(snapshot(
            "",
            vec![
                result(ReferenceSearchResultKind::Skill, "openai-docs"),
                result(ReferenceSearchResultKind::Mcp, "Docs"),
                result(ReferenceSearchResultKind::File, "src/main.rs"),
            ],
        ));

        assert_eq!(
            popup
                .rows(TEST_NAME_WIDTH)
                .into_iter()
                .map(|row| (row_name_prefix(&row), row.name, row.category_tag))
                .collect::<Vec<_>>(),
            vec![
                (
                    Some((" [▦ Skill] ".to_string(), Some(Color::LightCyan))),
                    "openai-docs".to_string(),
                    None,
                ),
                (
                    Some((" [⬡ MCP] ".to_string(), Some(Color::LightMagenta))),
                    "Docs".to_string(),
                    None,
                ),
                (
                    Some((" [≣ FILE] ".to_string(), Some(Color::LightYellow))),
                    "src/main.rs".to_string(),
                    None,
                ),
            ]
        );
    }

    /// Trace: L2-DES-CLIENT-002
    /// Verifies: Non-empty fuzzy queries render server-filtered categories in order with category prefixes.
    #[test]
    fn non_empty_query_uses_server_filtered_category_order() {
        let mut popup = ReferencePopup::new(Color::Cyan);
        popup.set_query("docs");
        popup.set_snapshot(snapshot(
            "docs",
            vec![
                result(ReferenceSearchResultKind::Skill, "docs-skill"),
                result(ReferenceSearchResultKind::Mcp, "Docs"),
                result(ReferenceSearchResultKind::File, "docs/tui-chat-composer.md"),
            ],
        ));

        assert_eq!(
            popup
                .rows(TEST_NAME_WIDTH)
                .into_iter()
                .map(|row| (row_name_prefix(&row), row.name, row.category_tag))
                .collect::<Vec<_>>(),
            vec![
                (
                    Some((" [▦ Skill] ".to_string(), Some(Color::LightCyan))),
                    "docs-skill".to_string(),
                    None,
                ),
                (
                    Some((" [⬡ MCP] ".to_string(), Some(Color::LightMagenta))),
                    "Docs".to_string(),
                    None,
                ),
                (
                    Some((" [≣ FILE] ".to_string(), Some(Color::LightYellow))),
                    "docs/tui-chat-composer.md".to_string(),
                    None,
                ),
            ]
        );
    }

    /// Trace: L2-DES-CLIENT-002
    /// Verifies: MCP selections use a stable @mcp reference token and mention target.
    #[test]
    fn selected_mcp_reference_uses_stable_insert_token() {
        let mut popup = ReferencePopup::new(Color::Cyan);
        popup.set_query("docs");
        popup.set_snapshot(snapshot(
            "docs",
            vec![ReferenceSearchResult {
                insert_text: "@mcp:docs".to_string(),
                mention_path: Some("mcp://server/docs".to_string()),
                ..result(ReferenceSearchResultKind::Mcp, "Docs")
            }],
        ));

        assert_eq!(
            popup.selected_reference(),
            Some(ReferenceSelection::Mcp {
                insert_text: "@mcp:docs".to_string(),
                path: "mcp://server/docs".to_string(),
            })
        );
    }

    /// Trace: L2-DES-CLIENT-002
    /// Verifies: File selections use an `@basename` insert token and relative mention path.
    #[test]
    fn selected_file_reference_uses_basename_insert_token() {
        let mut popup = ReferencePopup::new(Color::Cyan);
        popup.set_query("interactive");
        popup.set_snapshot(snapshot(
            "interactive",
            vec![result(
                ReferenceSearchResultKind::File,
                "crates/tui/src/interactive.rs",
            )],
        ));

        assert_eq!(
            popup.selected_reference(),
            Some(ReferenceSelection::File {
                path: PathBuf::from("crates/tui/src/interactive.rs"),
                insert_text: "@interactive.rs".to_string(),
                mention_path: "crates/tui/src/interactive.rs".to_string(),
            })
        );
    }

    /// Trace: L2-DES-CLIENT-002
    /// Verifies: Stale reference-search snapshots are rejected.
    #[test]
    fn stale_file_results_are_ignored() {
        let mut popup = ReferencePopup::new(Color::Cyan);
        popup.set_query("new");
        popup.set_snapshot(snapshot(
            "old",
            vec![result(ReferenceSearchResultKind::File, "src/old.rs")],
        ));

        assert_eq!(popup.rows(TEST_NAME_WIDTH).len(), 0);
    }
}
