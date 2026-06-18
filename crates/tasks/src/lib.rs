//! Background task lifecycle primitives.
//!
//! The task manager keeps long-running work observable by storing task state and
//! draining notifications back into the conversation loop.

mod manager;
mod task;

pub use manager::*;
pub use task::*;
