mod app;
mod cli;
mod model;
mod paths;
mod platform;
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

pub fn render_ui_preview(
    width: u16,
    height: u16,
    section: &str,
    sample: bool,
) -> anyhow::Result<String> {
    ui::render_preview(width, height, section, sample)
}
