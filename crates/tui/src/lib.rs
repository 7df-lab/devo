//! Interactive terminal UI.
//!
//! public entry point for launching the CLI TUI.
#![allow(dead_code)]
mod app;
mod events;
mod onboarding;
mod v2;
mod worker;

pub use app::AppExit;
pub use app::InitialTuiSession;
pub use app::InteractiveTuiConfig;
pub use events::SavedModelEntry;
pub use v2::*;
