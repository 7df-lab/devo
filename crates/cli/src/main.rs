use anyhow::Result;
use clap::{Parser, Subcommand};
use clawcr_core::{
    AppConfig, AppConfigLoader, FileSystemAppConfigLoader, LoggingBootstrap, LoggingRuntime,
};
use clawcr_server::{run_server_process, ServerProcessArgs};
use clawcr_utils::find_clawcr_home;

mod agent;
mod config;

use agent::{run_agent, AgentCli};

/// Top-level `clawcr` command that dispatches to interactive agent mode or one
/// of the supporting runtime subcommands.
#[derive(Debug, Parser)]
#[command(name = "clawcr", version, about = "ClawCR CLI")]
struct Cli {
    #[command(flatten)]
    agent: AgentCli,

    #[command(subcommand)]
    command: Option<Commands>,
}

/// Subcommands exposed by the primary `clawcr` executable.
#[derive(Debug, Subcommand)]
enum Commands {
    /// Start the transport-facing server runtime.
    Server(ServerProcessArgs),
    /// Open the interactive model onboarding flow.
    Onboard,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let _logging = install_logging(&cli)?;

    match cli.command {
        Some(Commands::Server(args)) => run_server_process(args).await,
        Some(Commands::Onboard) => {
            let mut agent = cli.agent;
            agent.query = None;
            agent.print = None;
            run_agent(agent, true).await
        }
        None => run_agent(cli.agent, false).await,
    }
}

fn install_logging(cli: &Cli) -> Result<LoggingRuntime> {
    let home_dir = find_clawcr_home()?;
    let loader = FileSystemAppConfigLoader::new(home_dir.clone());
    let current_dir = std::env::current_dir()?;
    let workspace_root = match &cli.command {
        Some(Commands::Server(args)) => args.workspace_root.as_deref(),
        _ => Some(current_dir.as_path()),
    };
    let app_config = loader.load(workspace_root).unwrap_or_else(|err| {
        eprintln!("warning: failed to load app config for logging: {err}");
        AppConfig::default()
    });
    LoggingBootstrap {
        process_name: "cli",
        config: app_config.logging,
        home_dir,
    }
    .install()
    .map_err(Into::into)
}
