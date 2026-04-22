use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, anyhow};
use serde::Serialize;

use crate::TASK_NAME;
use crate::model::AutostartMethod;

#[derive(Debug, Clone, Serialize)]
pub struct AutostartStatus {
    pub installed: bool,
    pub method: Option<AutostartMethod>,
    pub target: Option<String>,
}

#[cfg(windows)]
pub fn install_autostart(executable: &Path) -> anyhow::Result<AutostartMethod> {
    let command = format!("\"{}\" watch --headless", executable.display());

    match create_scheduled_task(&command) {
        Ok(output) if output.status.success() => {
            let _ = remove_startup_launcher();
            Ok(AutostartMethod::ScheduledTask)
        }
        Ok(output) => install_startup_fallback(executable, &render_output(&output)),
        Err(error) => install_startup_fallback(executable, &error.to_string()),
    }
}

#[cfg(not(windows))]
pub fn install_autostart(_executable: &Path) -> anyhow::Result<AutostartMethod> {
    Err(anyhow!("Поддерживается только Windows"))
}

#[cfg(windows)]
pub fn remove_autostart() -> anyhow::Result<()> {
    let mut errors = Vec::new();

    match delete_scheduled_task() {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            let message = render_output(&output);
            if !task_not_found(&message) {
                errors.push(format!("schtasks /Delete: {message}"));
            }
        }
        Err(error) => errors.push(error.to_string()),
    }

    if let Err(error) = remove_startup_launcher() {
        errors.push(error.to_string());
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(errors.join(" | ")))
    }
}

#[cfg(not(windows))]
pub fn remove_autostart() -> anyhow::Result<()> {
    Err(anyhow!("Поддерживается только Windows"))
}

#[cfg(windows)]
pub fn query_autostart() -> AutostartStatus {
    let startup_path = startup_launcher_path().ok();

    if query_scheduled_task().is_some() {
        return AutostartStatus {
            installed: true,
            method: Some(AutostartMethod::ScheduledTask),
            target: Some(TASK_NAME.to_string()),
        };
    }

    if let Some(path) = startup_path {
        if path.exists() {
            return AutostartStatus {
                installed: true,
                method: Some(AutostartMethod::StartupFolder),
                target: Some(path.display().to_string()),
            };
        }
    }

    AutostartStatus {
        installed: false,
        method: None,
        target: None,
    }
}

#[cfg(not(windows))]
pub fn query_autostart() -> AutostartStatus {
    AutostartStatus {
        installed: false,
        method: None,
        target: None,
    }
}

#[cfg(windows)]
fn create_scheduled_task(command: &str) -> anyhow::Result<Output> {
    Command::new("schtasks")
        .args([
            "/Create", "/SC", "ONLOGON", "/TN", TASK_NAME, "/TR", command, "/RL", "LIMITED", "/F",
        ])
        .output()
        .context("Не удалось создать Scheduled Task")
}

#[cfg(windows)]
fn delete_scheduled_task() -> anyhow::Result<Output> {
    Command::new("schtasks")
        .args(["/Delete", "/TN", TASK_NAME, "/F"])
        .output()
        .context("Не удалось удалить Scheduled Task")
}

#[cfg(windows)]
fn query_scheduled_task() -> Option<Output> {
    Command::new("schtasks")
        .args(["/Query", "/TN", TASK_NAME])
        .output()
        .ok()
        .filter(|output| output.status.success())
}

#[cfg(windows)]
fn install_startup_launcher(executable: &Path) -> anyhow::Result<()> {
    let path = startup_launcher_path()?;
    let body = startup_launcher_body(executable);
    fs::write(&path, body).with_context(|| format!("Не удалось записать {}", path.display()))?;
    Ok(())
}

#[cfg(windows)]
fn install_startup_fallback(
    executable: &Path,
    scheduled_task_error: &str,
) -> anyhow::Result<AutostartMethod> {
    install_startup_launcher(executable)
        .map(|_| AutostartMethod::StartupFolder)
        .map_err(|startup_error| {
            anyhow!(
                "schtasks /Create вернул ошибку: {scheduled_task_error} | startup_folder: {startup_error}"
            )
        })
}

#[cfg(windows)]
fn remove_startup_launcher() -> anyhow::Result<()> {
    let path = startup_launcher_path()?;
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("Не удалось удалить {}", path.display()))?;
    }
    Ok(())
}

#[cfg(windows)]
fn startup_launcher_path() -> anyhow::Result<PathBuf> {
    let appdata = env::var("APPDATA").context("APPDATA не найден")?;
    Ok(PathBuf::from(appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Startup")
        .join("ProtoSwitch.cmd"))
}

#[cfg(windows)]
fn startup_launcher_body(executable: &Path) -> String {
    format!(
        "@echo off\r\npowershell -NoProfile -WindowStyle Hidden -Command \"Start-Process -WindowStyle Hidden -FilePath '{}' -ArgumentList 'watch --headless'\"\r\n",
        executable.display()
    )
}

#[cfg(windows)]
fn task_not_found(message: &str) -> bool {
    let value = message.to_lowercase();
    value.contains("cannot find the file specified")
        || value.contains("не удается найти указанный файл")
        || value.contains("не удаётся найти указанный файл")
}

#[cfg(windows)]
fn render_output(output: &Output) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_task_not_found_text() {
        assert!(task_not_found(
            "ERROR: The system cannot find the file specified."
        ));
        assert!(task_not_found("Ошибка: Не удается найти указанный файл."));
        assert!(!task_not_found("ERROR: Access is denied."));
    }

    #[test]
    fn startup_script_contains_hidden_process_launch() {
        let body = startup_launcher_body(Path::new(r"C:\Tools\protoswitch.exe"));
        assert!(body.contains("Start-Process -WindowStyle Hidden"));
        assert!(body.contains("watch --headless"));
    }
}
