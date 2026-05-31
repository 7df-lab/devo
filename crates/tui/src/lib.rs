//! Interactive terminal UI.
//!
//! public entry point for launching the CLI TUI.
#![allow(dead_code)]
mod app;
pub(crate) mod app_command;
mod app_event;
mod app_event_sender;
mod bottom_pane;
mod chatwidget;
#[cfg(test)]
mod chatwidget_tests;
mod clipboard_copy;
mod clipboard_paste;
mod color;
mod custom_terminal;
#[cfg(test)]
mod custom_terminal_clear_tests;
mod diff_render;
mod events;
mod exec_cell;
mod exec_command;
mod get_git_diff;
mod history_cell;
mod host_overlay;
mod insert_history;
mod interactive;
mod key_hint;
mod line_truncation;
mod live_wrap;
mod markdown;
pub mod markdown_render;
mod markdown_stream;
mod onboarding;
mod onboarding_widget;
mod pager_overlay;
mod render;
mod shimmer;
mod slash_command;
mod startup_header;
mod state;
mod status_indicator_widget;
mod streaming;
mod style;
mod terminal_palette;
mod test_backend;
mod text_formatting;
mod theme;
#[cfg(test)]
mod tool_rendering_e2e_tests;
mod tool_result_cell;
mod tui;
mod ui_consts;
mod version;
mod worker;
mod wrapping;

pub use interactive::run_interactive_tui;

pub use app::AppExit;
pub use app::InitialTuiSession;
pub use app::InteractiveTuiConfig;
pub use events::SavedModelEntry;
