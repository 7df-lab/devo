//! Shell command parsing and safety classification helpers.
//!
//! The crate keeps shell-specific parsing separate from tool execution so
//! callers can decide whether a command is known-safe or potentially dangerous
//! before handing it to any runtime.

mod shell_command;

pub use shell_command::bash;
pub use shell_command::is_dangerous_command;
pub use shell_command::is_safe_command;
pub use shell_command::parse_command;
pub use shell_command::powershell;
