use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "protoswitch",
    version,
    about = "CLI/TUI для автосмены MTProto proxy в Telegram Desktop"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    Init(InitArgs),
    Watch(WatchArgs),
    Status(StatusArgs),
    Switch(SwitchArgs),
    Cleanup,
    Doctor(DoctorArgs),
    Repair,
    Shutdown,
    Tray,
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_without_command() {
        let cli = Cli::parse_from(["protoswitch"]);
        assert!(cli.command.is_none());
    }

    #[test]
    fn parses_status_command() {
        let cli = Cli::parse_from(["protoswitch", "status", "--plain"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Status(StatusArgs {
                plain: true,
                json: false
            }))
        ));
    }

    #[test]
    fn parses_shutdown_command() {
        let cli = Cli::parse_from(["protoswitch", "shutdown"]);
        assert!(matches!(cli.command, Some(Commands::Shutdown)));
    }

    #[test]
    fn parses_tray_command() {
        let cli = Cli::parse_from(["protoswitch", "tray"]);
        assert!(matches!(cli.command, Some(Commands::Tray)));
    }
}
