#![allow(dead_code)]

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use tokio::process::Command;
use tokio::sync::OnceCell;

static DEVO_BINARY: OnceCell<PathBuf> = OnceCell::const_new();
static MCP_BINARY: OnceCell<PathBuf> = OnceCell::const_new();

pub(crate) async fn devo_command() -> Result<Command> {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_devo").map(PathBuf::from)
        && path.is_file()
    {
        return Ok(Command::new(path));
    }
    let path = DEVO_BINARY
        .get_or_try_init(|| build_binary("devo-cli", "devo"))
        .await?;
    Ok(Command::new(path))
}

pub(crate) async fn mcp_server_binary() -> Result<PathBuf> {
    Ok(MCP_BINARY
        .get_or_try_init(|| build_binary("devo-rmcp-client", "test_stdio_server"))
        .await?
        .clone())
}

pub(crate) fn current_binary_path(name: &str) -> Result<PathBuf> {
    Ok(TestProfile::from_executable(&std::env::current_exe()?)?.binary_path(name))
}

async fn build_binary(package: &str, name: &str) -> Result<PathBuf> {
    let profile = TestProfile::from_executable(&std::env::current_exe()?)?;
    // Check freshness once per test process. Existence alone can silently run
    // an older CLI even though Cargo just rebuilt the test's server library.
    let status = profile
        .build_command(package, name)
        .status()
        .await
        .with_context(|| format!("build test binary {name}"))?;
    ensure!(status.success(), "test binary {name} build failed");
    let path = profile.binary_path(name);
    ensure!(path.is_file(), "test binary missing at {}", path.display());
    Ok(path)
}

#[derive(Debug, PartialEq, Eq)]
struct TestProfile {
    directory: PathBuf,
    cargo_name: String,
}

impl TestProfile {
    fn from_executable(executable: &Path) -> Result<Self> {
        let deps = executable.parent().context("test executable directory")?;
        ensure!(
            deps.file_name().is_some_and(|name| name == "deps"),
            "test executable must be under a Cargo deps directory"
        );
        let directory = deps
            .parent()
            .context("test profile directory")?
            .to_path_buf();
        let output_name = directory
            .file_name()
            .and_then(|name| name.to_str())
            .context("test profile name")?;
        let cargo_name = match output_name {
            "debug" => "dev",
            name => name,
        }
        .to_owned();
        Ok(Self {
            directory,
            cargo_name,
        })
    }

    fn binary_path(&self, name: &str) -> PathBuf {
        self.directory
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
    }

    fn build_command(&self, package: &str, name: &str) -> Command {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let mut command = Command::new(cargo);
        command.current_dir(workspace).args([
            "build",
            "--quiet",
            "--locked",
            "--profile",
            &self.cargo_name,
            "-p",
            package,
            "--bin",
            name,
        ]);
        command
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn binaries_and_builds_use_the_test_profile() -> Result<()> {
        for (output, cargo_name) in [
            ("debug", "dev"),
            ("dev-fast", "dev-fast"),
            ("release", "release"),
        ] {
            let directory = PathBuf::from("custom-target").join(output);
            let executable = directory.join("deps").join("contract-test");
            let profile = TestProfile::from_executable(&executable)?;
            assert_eq!(
                profile,
                TestProfile {
                    directory: directory.clone(),
                    cargo_name: cargo_name.to_owned()
                }
            );
            assert_eq!(
                profile.binary_path("devo"),
                directory.join(format!("devo{}", std::env::consts::EXE_SUFFIX))
            );
            let command = profile.build_command("devo-cli", "devo");
            assert_eq!(
                command.as_std().get_args().collect::<Vec<_>>(),
                [
                    "build",
                    "--quiet",
                    "--locked",
                    "--profile",
                    cargo_name,
                    "-p",
                    "devo-cli",
                    "--bin",
                    "devo",
                ]
                .map(std::ffi::OsStr::new)
            );
        }
        Ok(())
    }

    #[test]
    fn unexpected_test_executable_layout_is_rejected() {
        assert!(TestProfile::from_executable(&PathBuf::from("other").join("test")).is_err());
    }
}
