//! Keep installed interpreters independent of host Python and Node settings.

use std::ffi::OsStr;
use std::process::Command;

/// The private interpreter whose host settings must be excluded.
#[derive(Clone, Copy)]
pub enum Interpreter {
    /// Node's preload options and global module search path.
    Node,
    /// Python's prefix, module search path, user site, and startup settings.
    Python,
}

/// Whether a host setting can redirect an interpreter or inject its modules.
/// Match names case-insensitively because Windows environment keys are folded.
pub fn is_interpreter_env_var(key: &OsStr, interpreter: Interpreter) -> bool {
    let key = key.to_string_lossy().to_ascii_uppercase();
    match interpreter {
        Interpreter::Node => matches!(key.as_str(), "NODE_OPTIONS" | "NODE_PATH"),
        Interpreter::Python => key.starts_with("PYTHON"),
    }
}

/// Remove host interpreter settings from an installed runtime's child command.
/// Apply Devo's explicit Python settings afterward. Source development commands
/// retain their environment unless the caller identifies an installed bundle.
pub fn remove_interpreter_environment(command: &mut Command, interpreter: Interpreter) {
    let keys: Vec<_> = std::env::vars_os()
        .map(|(key, _)| key)
        .chain(command.get_envs().map(|(key, _)| key.to_os_string()))
        .filter(|key| is_interpreter_env_var(key, interpreter))
        .collect();
    for key in keys {
        command.env_remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn installed_command_discards_interpreter_settings_and_preserves_other_env() {
        let mut command = Command::new("private-interpreter");
        let unwanted = [
            "PYTHONHOME",
            "PYTHONPATH",
            "PythonUserBase",
            "PYTHONSTARTUP",
            "NODE_OPTIONS",
            "NODE_PATH",
        ];
        for key in unwanted {
            command.env(key, "host-specific setting");
        }
        command.env("DEVO_HOME", "devo-home");
        command.env("PATH", "workspace-tools");
        remove_interpreter_environment(&mut command, Interpreter::Python);
        let node_settings: Vec<_> = command
            .get_envs()
            .filter(|(key, _)| is_interpreter_env_var(key, Interpreter::Node))
            .map(|(_, value)| value.unwrap().to_str().unwrap())
            .collect();
        assert_eq!(node_settings, vec!["host-specific setting"; 2]);
        remove_interpreter_environment(&mut command, Interpreter::Node);
        let actual: Vec<_> = unwanted
            .iter()
            .map(|key| {
                command
                    .get_envs()
                    .find(|(name, _)| name == key)
                    .map(|(_, value)| value)
            })
            .collect();
        assert_eq!(actual, vec![Some(None); unwanted.len()]);
        let retained: Vec<_> = command
            .get_envs()
            .filter(|(key, _)| matches!(key.to_str(), Some("DEVO_HOME" | "PATH")))
            .map(|(key, value)| (key.to_str().unwrap(), value.unwrap().to_str().unwrap()))
            .collect();
        assert_eq!(
            retained,
            vec![("DEVO_HOME", "devo-home"), ("PATH", "workspace-tools")]
        );
    }
}
