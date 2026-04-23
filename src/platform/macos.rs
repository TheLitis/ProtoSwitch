use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Serialize;

use crate::model::AutostartMethod;

const PLIST_NAME: &str = "com.thelitis.protoswitch.plist";

#[derive(Debug, Clone, Serialize)]
pub struct AutostartStatus {
    pub installed: bool,
    pub method: Option<AutostartMethod>,
    pub target: Option<String>,
}

pub fn install_autostart(executable: &Path) -> anyhow::Result<AutostartMethod> {
    let path = launch_agent_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Не удалось создать {}", parent.display()))?;
    }
    fs::write(&path, launch_agent_plist(executable))
        .with_context(|| format!("Не удалось записать {}", path.display()))?;
    Ok(AutostartMethod::LaunchAgent)
}

pub fn remove_autostart() -> anyhow::Result<()> {
    let path = launch_agent_path()?;
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("Не удалось удалить {}", path.display()))?;
    }
    Ok(())
}

pub fn query_autostart() -> AutostartStatus {
    match launch_agent_path() {
        Ok(path) if path.exists() => AutostartStatus {
            installed: true,
            method: Some(AutostartMethod::LaunchAgent),
            target: Some(path.display().to_string()),
        },
        _ => AutostartStatus {
            installed: false,
            method: None,
            target: None,
        },
    }
}

fn launch_agent_path() -> anyhow::Result<PathBuf> {
    let home = directories::BaseDirs::new()
        .context("Не удалось определить домашний каталог")?
        .home_dir()
        .to_path_buf();
    Ok(home.join("Library").join("LaunchAgents").join(PLIST_NAME))
}

fn launch_agent_plist(executable: &Path) -> String {
    let executable = xml_escape(&executable.display().to_string());
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>com.thelitis.protoswitch</string>\n  <key>ProgramArguments</key>\n  <array>\n    <string>{executable}</string>\n    <string>watch</string>\n    <string>--headless</string>\n  </array>\n  <key>RunAtLoad</key>\n  <true/>\n  <key>KeepAlive</key>\n  <false/>\n  <key>WorkingDirectory</key>\n  <string>{}</string>\n</dict>\n</plist>\n",
        xml_escape(
            &executable
                .rsplit_once(['/', '\\'])
                .map(|(parent, _)| parent.to_string())
                .unwrap_or_else(|| ".".to_string())
        )
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_agent_contains_program_arguments() {
        let body = launch_agent_plist(Path::new(
            "/Applications/ProtoSwitch.app/Contents/MacOS/protoswitch",
        ));
        assert!(body.contains("<string>watch</string>"));
        assert!(body.contains("<string>--headless</string>"));
        assert!(body.contains("com.thelitis.protoswitch"));
    }
}
