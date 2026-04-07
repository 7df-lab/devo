//! Interactive terminal UI for ClawCR.

mod app;
mod events;
mod input;
mod onboarding_config;
mod paste_burst;
mod render;
mod slash;
mod terminal;
mod worker;

pub use app::run_interactive_tui;
pub use app::AppExit;
pub use app::InteractiveTuiConfig;
