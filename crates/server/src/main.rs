use anyhow::Result;
use clap::Parser;
use clawcr_core::{
    AppConfig, AppConfigLoader, FileSystemAppConfigLoader, LoggingBootstrap, LoggingRuntime,
};
use clawcr_server::{run_server_process, ServerProcessArgs};
use clawcr_utils::find_clawcr_home;

#[tokio::main]
async fn main() -> Result<()> {
    let args = ServerProcessArgs::parse();
    let _logging = install_logging(&args)?;
    run_server_process(args).await
}

fn install_logging(args: &ServerProcessArgs) -> Result<LoggingRuntime> {
    let home_dir = find_clawcr_home()?;
    let loader = FileSystemAppConfigLoader::new(home_dir.clone());
    let app_config = loader
        .load(args.workspace_root.as_deref())
        .unwrap_or_else(|err| {
            eprintln!("warning: failed to load app config for logging: {err}");
            AppConfig::default()
        });
    LoggingBootstrap {
        process_name: "server",
        config: app_config.logging,
        home_dir,
    }
    .install()
    .map_err(Into::into)
}
