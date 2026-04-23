use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use serde::Serialize;

use crate::model::AutostartMethod;

const DESKTOP_FILE_NAME: &str = "protoswitch.desktop";

#[derive(Debug, Clone, Serialize)]
pub struct AutostartStatus {
    pub installed: bool,
    pub method: Option<AutostartMethod>,
    pub target: Option<String>,
}

pub fn install_autostart(executable: &Path) -> anyhow::Result<AutostartMethod> {
    let path = desktop_entry_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Не удалось создать {}", parent.display()))?;
    }
    fs::write(&path, desktop_entry(executable))
        .with_context(|| format!("Не удалось записать {}", path.display()))?;
    Ok(AutostartMethod::XdgDesktop)
}

pub fn remove_autostart() -> anyhow::Result<()> {
    let path = desktop_entry_path()?;
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("Не удалось удалить {}", path.display()))?;
    }
    Ok(())
}

pub fn query_autostart() -> AutostartStatus {
    match desktop_entry_path() {
        Ok(path) if path.exists() => AutostartStatus {
            installed: true,
            method: Some(AutostartMethod::XdgDesktop),
            target: Some(path.display().to_string()),
        },
        _ => AutostartStatus {
            installed: false,
            method: None,
            target: None,
        },
    }
}

fn desktop_entry_path() -> anyhow::Result<PathBuf> {
    if let Ok(config_home) = env::var("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(config_home)
            .join("autostart")
            .join(DESKTOP_FILE_NAME));
    }

    let home = directories::BaseDirs::new()
        .context("Не удалось определить домашний каталог")?
        .home_dir()
        .to_path_buf();
    Ok(home.join(".config").join("autostart").join(DESKTOP_FILE_NAME))
}

fn desktop_entry(executable: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nVersion=1.0\nName=ProtoSwitch\nExec={} watch --headless\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        desktop_exec_argument(&executable.display().to_string())
    )
}

fn desktop_exec_argument(value: &str) -> String {
    value
        .chars()
        .flat_map(|ch| match ch {
            '\\' => ['\\', '\\'].into_iter().collect::<Vec<_>>(),
            ' ' => ['\\', ' '].into_iter().collect::<Vec<_>>(),
            '"' => ['\\', '"'].into_iter().collect::<Vec<_>>(),
            '\t' => ['\\', 't'].into_iter().collect::<Vec<_>>(),
            '\n' => ['\\', 'n'].into_iter().collect::<Vec<_>>(),
            _ => vec![ch],
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_contains_headless_watcher_exec() {
        let body = desktop_entry(Path::new("/opt/Proto Switch/protoswitch"));
        assert!(body.contains("watch --headless"));
        assert!(body.contains("Exec=/opt/Proto\\ Switch/protoswitch"));
    }
}
