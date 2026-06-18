//! Git helper APIs for repository inspection, patch application, and snapshot workflows.
//!
//! Higher-level runtime code uses this crate to keep Git command orchestration
//! separate from agent control flow.

mod git_op;

pub use git_op::*;
