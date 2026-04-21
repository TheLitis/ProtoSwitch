use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "protoswitch",
    version,
    about = "CLI/TUI для автосмены MTProto proxy в Telegram Desktop"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    Init(InitArgs),
    Watch(WatchArgs),
    Status(StatusArgs),
    Switch(SwitchArgs),
    Doctor(DoctorArgs),
    Autostart {
        #[command(subcommand)]
        command: AutostartCommand,
    },
}

#[derive(Debug, Args)]
pub struct InitArgs {
    #[arg(long)]
    pub non_interactive: bool,
    #[arg(long)]
    pub autostart: bool,
    #[arg(long)]
    pub no_autostart: bool,
    #[arg(long)]
    pub check_interval: Option<u64>,
    #[arg(long)]
    pub connect_timeout: Option<u64>,
    #[arg(long)]
    pub failure_threshold: Option<u32>,
    #[arg(long)]
    pub history_size: Option<usize>,
}

#[derive(Debug, Args)]
pub struct WatchArgs {
    #[arg(long)]
    pub headless: bool,
    #[arg(long, hide = true)]
    pub once: bool,
}

#[derive(Debug, Args)]
pub struct StatusArgs {
    #[arg(long)]
    pub plain: bool,
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct SwitchArgs {
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct DoctorArgs {
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum AutostartCommand {
    Install,
    Remove,
}
