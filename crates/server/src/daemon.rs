//! Explicit control of the independent, user-owned Native runtime.
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::singleton::{
    ServerControlAction, ServerLifetime, owner_lock_held, probe_singleton, run_server_control,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DaemonStatus {
    pub pid: u32,
    pub started_at: String,
}

/// Probe an authenticated daemon. Never promote a missing server to an owner.
pub async fn daemon_status(home: &Path) -> Result<Option<DaemonStatus>> {
    let home = crate::singleton_files::daemon_home(home)?;
    match probe_singleton(&home)? {
        None => Ok(None),
        Some(metadata) => {
            let result = run_server_control(&metadata, ServerControlAction::Status).await?;
            if result.lifetime != ServerLifetime::Daemon {
                bail!(
                    "a client-owned server is running; stop it explicitly before starting a daemon"
                );
            }
            Ok(Some(DaemonStatus {
                pid: metadata.pid,
                started_at: metadata.started_at,
            }))
        }
    }
}

/// Request graceful shutdown and confirm the singleton lock has been released.
/// Never signal a PID read from disk or silently stop a client-owned server.
pub async fn stop_daemon(home: &Path) -> Result<()> {
    let home = crate::singleton_files::daemon_home(home)?;
    match probe_singleton(&home)? {
        None => return Ok(()),
        Some(metadata) => {
            let status = run_server_control(&metadata, ServerControlAction::Status).await?;
            if status.lifetime != ServerLifetime::Daemon {
                bail!("refusing to stop a client-owned server");
            }
            run_server_control(&metadata, ServerControlAction::ShutdownDaemon).await?;
        }
    }
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if !owner_lock_held(&home)? {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .context("daemon acknowledged shutdown but did not release ownership within 30 seconds")??;
    Ok(())
}

/// Check the directory boundary before a detached child is launched.
pub fn validate_daemon_home(home: &Path) -> Result<std::path::PathBuf> {
    crate::singleton_files::daemon_home(home)
}

/// Service managers normally send SIGTERM; preserve the graceful shutdown path.
pub(crate) async fn wait_for_shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! { result = tokio::signal::ctrl_c() => result?, _ = terminate.recv() => {} }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await?;
        Ok(())
    }
}
