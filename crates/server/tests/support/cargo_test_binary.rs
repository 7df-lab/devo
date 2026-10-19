#![allow(dead_code)]

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

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
    Ok(TestProfile::current()?.binary_path(name))
}

async fn build_binary(package: &str, name: &str) -> Result<PathBuf> {
    let profile = TestProfile::current()?;
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
    target_directory: PathBuf,
    target: Option<OsString>,
}

impl TestProfile {
    fn current() -> Result<Self> {
        let target_directory = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .parent()
            .context("Cargo target directory")?;
        let target_directory =
            std::fs::canonicalize(target_directory).context("canonical Cargo target directory")?;
        let executable =
            std::fs::canonicalize(std::env::current_exe()?).context("canonical test executable")?;
        Self::from_executable(&executable, &target_directory)
    }

    fn from_executable(executable: &Path, target_directory: &Path) -> Result<Self> {
        let deps = executable.parent().context("test executable directory")?;
        ensure!(
            deps.file_name().is_some_and(|name| name == "deps"),
            "test executable must be under a Cargo deps directory"
        );
        let directory = deps
            .parent()
            .context("test profile directory")?
            .to_path_buf();
        let relative = directory.strip_prefix(target_directory).with_context(|| {
            format!(
                "test profile {} is outside Cargo target directory {}",
                directory.display(),
                target_directory.display()
            )
        })?;
        let components = relative.components().collect::<Vec<_>>();
        let (target, output_name) = match components.as_slice() {
            [Component::Normal(profile)] => (None, *profile),
            [Component::Normal(target), Component::Normal(profile)] => {
                (Some(target.to_os_string()), *profile)
            }
            _ => anyhow::bail!("unexpected Cargo test output layout"),
        };
        let output_name = output_name.to_str().context("test profile name")?;
        let cargo_name = match output_name {
            "debug" => "dev",
            name => name,
        }
        .to_owned();
        Ok(Self {
            directory,
            cargo_name,
            target_directory: target_directory.to_path_buf(),
            target,
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
        command
            .current_dir(workspace)
            .args([
                "build",
                "--quiet",
                "--locked",
                "--profile",
                &self.cargo_name,
                "--target-dir",
            ])
            .arg(&self.target_directory)
            .args(["-p", package, "--bin", name]);
        if let Some(target) = &self.target {
            command.arg("--target").arg(target);
        }
        command
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    #[test]
    fn binaries_and_builds_use_the_test_profile_and_target() -> Result<()> {
        let root = TempDir::new()?;
        let target_directory = root.path().join("custom-output");
        for (output, cargo_name) in [
            ("debug", "dev"),
            ("dev-fast", "dev-fast"),
            ("release", "release"),
        ] {
            for target in [None, Some("x86_64-pc-windows-msvc")] {
                let directory = target
                    .map_or_else(
                        || target_directory.clone(),
                        |name| target_directory.join(name),
                    )
                    .join(output);
                let executable = directory.join("deps").join("contract-test");
                let profile = TestProfile::from_executable(&executable, &target_directory)?;
                assert_eq!(
                    profile,
                    TestProfile {
                        directory: directory.clone(),
                        cargo_name: cargo_name.to_owned(),
                        target_directory: target_directory.clone(),
                        target: target.map(OsString::from),
                    }
                );
                assert_eq!(
                    profile.binary_path("devo"),
                    directory.join(format!("devo{}", std::env::consts::EXE_SUFFIX))
                );
                let command = profile.build_command("devo-cli", "devo");
                let mut expected = [
                    "build",
                    "--quiet",
                    "--locked",
                    "--profile",
                    cargo_name,
                    "--target-dir",
                ]
                .map(OsString::from)
                .to_vec();
                expected.push(target_directory.as_os_str().to_owned());
                expected.extend(["-p", "devo-cli", "--bin", "devo"].map(OsString::from));
                if let Some(target) = target {
                    expected.extend(["--target", target].map(OsString::from));
                }
                assert_eq!(
                    command
                        .as_std()
                        .get_args()
                        .map(std::ffi::OsStr::to_owned)
                        .collect::<Vec<_>>(),
                    expected
                );
            }
        }
        Ok(())
    }

    #[test]
    fn unexpected_test_executable_layout_is_rejected() {
        let root = PathBuf::from("target");
        assert!(TestProfile::from_executable(&root.join("other").join("test"), &root).is_err());
        assert!(
            TestProfile::from_executable(
                &root
                    .join("a")
                    .join("b")
                    .join("debug")
                    .join("deps")
                    .join("test"),
                &root
            )
            .is_err()
        );
    }
}
