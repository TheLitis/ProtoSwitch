mod app;
mod cli;
mod model;
mod platform;
mod paths;
mod provider;
mod tdesktop;
mod telegram;
mod text;
mod ui;

pub const APP_NAME: &str = "ProtoSwitch";
pub const TASK_NAME: &str = "ProtoSwitch";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn run() -> anyhow::Result<()> {
    app::run()
}
