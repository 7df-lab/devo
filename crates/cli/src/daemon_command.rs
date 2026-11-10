//! Opt-in process orchestration; never installs a service or changes permissions.
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use devo_server::{ProtocolSet, ServerProcessArgs, ServerProcessRunOptions, ServerTransportMode};

#[derive(Debug, Subcommand)]
pub(crate) enum DaemonCommand {
    /// Start a detached, user-owned Native runtime (Unix).
    Start,
    /// Run the independent runtime in the foreground for an external supervisor.
    Run,
    /// Show authenticated runtime status.
    Status,
    /// Explicitly stop the independent runtime and wait for ownership release.
    Stop,
    /// Attach an NDJSON Native client; fail if no daemon is running.
    Attach,
}

pub(crate) async fn run_daemon(command: &DaemonCommand) -> Result<()> {
    let home = devo_server::validate_daemon_home(&devo_util_paths::find_devo_home()?)?;
    match command {
        DaemonCommand::Run | DaemonCommand::Attach => {
            devo_server::run_server_process(
                ServerProcessArgs {
                    transport: if matches!(command, DaemonCommand::Run) {
                        ServerTransportMode::Daemon
                    } else {
                        ServerTransportMode::Attach
                    },
                    protocols: ProtocolSet::default(),
                    status: false,
                    shutdown: false,
                },
                ServerProcessRunOptions::default(),
            )
            .await
        }
        DaemonCommand::Status => {
            match devo_server::daemon_status(&home).await? {
                Some(status) => println!(
                    "devo daemon running\npid: {}\nstarted_at: {}",
                    status.pid, status.started_at
                ),
                None => println!("devo daemon is not running"),
            }
            Ok(())
        }
        DaemonCommand::Stop => {
            devo_server::stop_daemon(&home).await?;
            println!("devo daemon stopped");
            Ok(())
        }
        DaemonCommand::Start => {
            // A concurrent launcher may own the lock before its metadata or
            // authenticated listener is ready. Never spawn a replacement based
            // on that uncertainty; wait for a confirmed owner or absence.
            let existing = tokio::time::timeout(Duration::from_secs(45), async {
                loop {
                    match devo_server::daemon_status(&home).await {
                        Ok(status) => return status,
                        Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
                    }
                }
            }).await.context("existing owner could not be confirmed as a daemon; run devo daemon status or devo daemon run to inspect the error")?;
            if let Some(status) = existing {
                println!("devo daemon already running (pid {})", status.pid);
                return Ok(());
            }
            let mut command =
                Command::new(std::env::current_exe().context("locate devo executable")?);
            command
                .args(["daemon", "run"])
                .env("DEVO_HOME", &home)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                // No stdio inherited from the caller and no controlling terminal.
                // Only the child changes its session; the user's shell is untouched.
                unsafe {
                    command.pre_exec(|| {
                        if libc::setsid() == -1 {
                            return Err(std::io::Error::last_os_error());
                        }
                        Ok(())
                    });
                }
            }
            let mut child = command.spawn().context("launch detached daemon")?;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
            loop {
                if let Ok(Some(status)) = devo_server::daemon_status(&home).await {
                    println!(
                        "devo daemon ready (pid {}). Closing clients will not stop it. Use devo daemon stop to stop it.",
                        status.pid
                    );
                    return Ok(());
                }
                if let Some(status) = child.try_wait()? {
                    bail!(
                        "daemon child exited before readiness: {status}; run devo daemon run to inspect startup errors"
                    );
                }
                if tokio::time::Instant::now() >= deadline {
                    // Readiness timeout does not prove failure. Do not kill an
                    // unverified owner or spawn a replacement behind the user's back.
                    bail!(
                        "daemon readiness unconfirmed after 45 seconds; it may still be starting. Check devo daemon status before retrying"
                    );
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[derive(Parser)]
    struct Arguments {
        #[command(subcommand)]
        command: DaemonCommand,
    }
    #[test]
    fn all_explicit_lifecycle_commands_parse() {
        for command in ["start", "run", "status", "stop", "attach"] {
            assert!(Arguments::try_parse_from(["daemon", command]).is_ok());
        }
        assert!(Arguments::try_parse_from(["daemon", "install"]).is_err());
    }
}
