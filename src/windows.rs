use std::path::Path;
use std::process::Command;

use anyhow::{Context, anyhow};

use crate::TASK_NAME;

#[cfg(windows)]
pub fn install_autostart(executable: &Path) -> anyhow::Result<()> {
    let command = format!("\"{}\" watch --headless", executable.display());
    let output = Command::new("schtasks")
        .args([
            "/Create", "/SC", "ONLOGON", "/TN", TASK_NAME, "/TR", &command, "/RL", "LIMITED", "/F",
        ])
        .output()
        .context("Не удалось создать Scheduled Task")?;

    if !output.status.success() {
        return Err(anyhow!(
            "schtasks /Create вернул ошибку: {}",
            render_output(&output)
        ));
    }

    Ok(())
}

#[cfg(not(windows))]
pub fn install_autostart(_executable: &Path) -> anyhow::Result<()> {
    Err(anyhow!("Поддерживается только Windows"))
}

#[cfg(windows)]
pub fn remove_autostart() -> anyhow::Result<()> {
    let output = Command::new("schtasks")
        .args(["/Delete", "/TN", TASK_NAME, "/F"])
        .output()
        .context("Не удалось удалить Scheduled Task")?;

    if !output.status.success() {
        return Err(anyhow!(
            "schtasks /Delete вернул ошибку: {}",
            render_output(&output)
        ));
    }

    Ok(())
}

#[cfg(not(windows))]
pub fn remove_autostart() -> anyhow::Result<()> {
    Err(anyhow!("Поддерживается только Windows"))
}

#[cfg(windows)]
pub fn query_autostart() -> bool {
    Command::new("schtasks")
        .args(["/Query", "/TN", TASK_NAME])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[cfg(not(windows))]
pub fn query_autostart() -> bool {
    false
}

#[cfg(windows)]
fn render_output(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stdout.is_empty() {
        return stdout;
    }

    "без текста ошибки".to_string()
}
