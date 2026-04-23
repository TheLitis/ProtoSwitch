mod app;
mod cli;
mod model;
mod paths;
mod provider;
mod telegram;
mod text;
mod ui;
mod windows;

pub const APP_NAME: &str = "ProtoSwitch";
pub const TASK_NAME: &str = "ProtoSwitch";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn run() -> anyhow::Result<()> {
    app::run()
}
