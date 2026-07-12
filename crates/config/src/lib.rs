//! Configuration loading, persistence, and typed runtime settings.
//!
//! This crate owns the boundary between user/workspace TOML or auth files and
//! the normalized settings consumed by the runtime and provider layers.

mod app;
mod error;
mod experimental;
mod hooks;
mod logging;
mod mcp;
mod oauth;
mod provider;
mod server;
mod skills;
mod tools;

pub use app::*;
pub use error::*;
pub use experimental::*;
pub use hooks::*;
pub use logging::*;
pub use mcp::*;
pub use oauth::*;
pub use provider::*;
pub use server::*;
pub use skills::*;
pub use tools::*;

pub(crate) use provider::read_provider_config_document;
pub(crate) use provider::write_atomic;

#[cfg(test)]
mod tests;
