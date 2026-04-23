use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, anyhow};
use serde::Serialize;

use crate::TASK_NAME;
use crate::model::AutostartMethod;
use crate::text::decode_output;

#[derive(Debug, Clone, Serialize)]
pub struct AutostartStatus {
    pub installed: bool,
    pub method: Option<AutostartMethod>,
    pub target: Option<String>,
}

#[cfg(windows)]
pub fn install_autostart(executable: &Path) -> anyhow::Result<AutostartMethod> {
    let command = format!("\"{}\" tray", executable.display());

    match create_scheduled_task(&command) {
        Ok(output) if output.status.success() => {
            let _ = remove_startup_launcher();
            Ok(AutostartMethod::ScheduledTask)
        }
        Ok(output) => install_startup_fallback(executable, &decode_output(&output)),
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
            let message = decode_output(&output);
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
    let _ = migrate_legacy_startup_launcher();
    let startup_path = startup_launcher_path().ok();
    let legacy_path = legacy_startup_script_path().ok();

    if query_scheduled_task().is_some() {
        return AutostartStatus {
            installed: true,
            method: Some(AutostartMethod::ScheduledTask),
            target: Some(TASK_NAME.to_string()),
        };
    }

    if let Some(path) = startup_path
        && path.exists()
    {
        return AutostartStatus {
            installed: true,
            method: Some(AutostartMethod::StartupFolder),
            target: Some(path.display().to_string()),
        };
    }

    if let Some(path) = legacy_path
        && path.exists()
    {
        return AutostartStatus {
            installed: true,
            method: Some(AutostartMethod::StartupFolder),
            target: Some(path.display().to_string()),
        };
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
    let _ = remove_legacy_startup_script();
    let script = startup_shortcut_script(executable, &path);
    let status = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &script,
        ])
        .status()
        .context("Не удалось запустить PowerShell для создания startup shortcut")?;

    if !status.success() {
        return Err(anyhow!(
            "PowerShell не смог создать startup shortcut {}",
            path.display()
        ));
    }

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
    remove_legacy_startup_script()
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
        .join("ProtoSwitch.lnk"))
}

#[cfg(windows)]
fn legacy_startup_script_path() -> anyhow::Result<PathBuf> {
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
fn remove_legacy_startup_script() -> anyhow::Result<()> {
    let legacy = legacy_startup_script_path()?;
    if legacy.exists() {
        fs::remove_file(&legacy)
            .with_context(|| format!("Не удалось удалить {}", legacy.display()))?;
    }
    Ok(())
}

#[cfg(windows)]
fn migrate_legacy_startup_launcher() -> anyhow::Result<()> {
    let shortcut = startup_launcher_path()?;
    let legacy = legacy_startup_script_path()?;

    if shortcut.exists() {
        return remove_legacy_startup_script();
    }

    if legacy.exists() {
        let executable =
            std::env::current_exe().context("Не удалось определить путь к protoswitch.exe")?;
        install_startup_launcher(&executable)?;
    }

    Ok(())
}

#[cfg(windows)]
fn startup_shortcut_script(executable: &Path, shortcut_path: &Path) -> String {
    let executable_value = executable.display().to_string();
    let shortcut_path = ps_literal(shortcut_path.display().to_string());
    let working_dir = ps_literal(
        executable
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .display()
            .to_string(),
    );
    let powershell_path = ps_literal("powershell.exe".to_string());
    let arguments = ps_literal(startup_launcher_arguments(&executable_value));
    let icon_location = ps_literal(format!("{},0", executable_value));

    format!(
        "$ErrorActionPreference = 'Stop'\n$ws = New-Object -ComObject WScript.Shell\n$shortcut = $ws.CreateShortcut({shortcut_path})\n$shortcut.TargetPath = {powershell_path}\n$shortcut.Arguments = {arguments}\n$shortcut.WorkingDirectory = {working_dir}\n$shortcut.IconLocation = {icon_location}\n$shortcut.WindowStyle = 7\n$shortcut.Save()\n",
    )
}

#[cfg(windows)]
fn startup_launcher_arguments(executable: &str) -> String {
    format!(
        "-NoProfile -WindowStyle Hidden -Command \"Start-Process -WindowStyle Hidden -FilePath '{}' -ArgumentList 'tray'\"",
        executable.replace('\'', "''")
    )
}

#[cfg(windows)]
fn ps_literal(value: String) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(windows)]
fn task_not_found(message: &str) -> bool {
    let value = message.to_lowercase();
    value.contains("cannot find the file specified")
        || value.contains("не удается найти указанный файл")
        || value.contains("не удаётся найти указанный файл")
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
    fn startup_shortcut_uses_hidden_powershell_launch() {
        let args = startup_launcher_arguments(r"C:\Tools\protoswitch.exe");
        assert!(args.contains("Start-Process -WindowStyle Hidden"));
        assert!(args.contains("'tray'"));
    }

    #[test]
    fn startup_shortcut_path_uses_lnk_extension() {
        let path = startup_launcher_path().unwrap();
        assert_eq!(
            path.extension().and_then(|value| value.to_str()),
            Some("lnk")
        );
    }
}
