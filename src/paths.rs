use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::Utc;
use directories::ProjectDirs;

use crate::APP_NAME;

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub local_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub config_file: PathBuf,
    pub state_file: PathBuf,
    pub log_file: PathBuf,
}

impl AppPaths {
    pub fn resolve() -> Result<Self> {
        let project_dirs = ProjectDirs::from("", "", APP_NAME)
            .context("Не удалось определить системные каталоги ProtoSwitch")?;
        Ok(Self {
            config_dir: project_dirs.config_dir().to_path_buf(),
            local_dir: project_dirs.data_local_dir().to_path_buf(),
            logs_dir: project_dirs.data_local_dir().join("logs"),
            config_file: project_dirs.config_dir().join("config.toml"),
            state_file: project_dirs.data_local_dir().join("state.json"),
            log_file: project_dirs.data_local_dir().join("logs").join("watch.log"),
        })
    }

    #[cfg(test)]
    pub fn from_base_dirs(config_root: PathBuf, local_root: PathBuf) -> Self {
        let config_dir = config_root.join(APP_NAME);
        let local_dir = local_root.join(APP_NAME);
        let logs_dir = local_dir.join("logs");
        let config_file = config_dir.join("config.toml");
        let state_file = local_dir.join("state.json");
        let log_file = logs_dir.join("watch.log");

        Self {
            config_dir,
            local_dir,
            logs_dir,
            config_file,
            state_file,
            log_file,
        }
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        fs::create_dir_all(&self.config_dir)
            .with_context(|| format!("Не удалось создать {}", self.config_dir.display()))?;
        fs::create_dir_all(&self.local_dir)
            .with_context(|| format!("Не удалось создать {}", self.local_dir.display()))?;
        fs::create_dir_all(&self.logs_dir)
            .with_context(|| format!("Не удалось создать {}", self.logs_dir.display()))?;
        Ok(())
    }

    pub fn append_log(&self, message: impl AsRef<str>) -> Result<()> {
        self.append_log_entry("info", "app", message, None::<&str>)
    }

    pub fn append_log_entry(
        &self,
        level: &str,
        source: &str,
        message: impl AsRef<str>,
        context: Option<impl AsRef<str>>,
    ) -> Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_file)
            .with_context(|| format!("Не удалось открыть {}", self.log_file.display()))?;
        let rendered = format!(
            "{} level={} source={} message={}{}",
            Utc::now().to_rfc3339(),
            escape_log_value(level),
            escape_log_value(source),
            escape_log_value(message.as_ref()),
            context
                .map(|value| format!(" context={}", escape_log_value(value.as_ref())))
                .unwrap_or_default()
        );
        writeln!(file, "{rendered}")
            .with_context(|| format!("Не удалось записать {}", self.log_file.display()))?;
        Ok(())
    }
}

fn escape_log_value(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn writes_structured_log_entry() {
        let root = tempdir().unwrap();
        let paths = AppPaths::from_base_dirs(root.path().join("config"), root.path().join("data"));
        paths.ensure_dirs().unwrap();
        paths
            .append_log_entry("warn", "telegram", "proxy rejected", Some("check status timeout"))
            .unwrap();

        let raw = fs::read_to_string(&paths.log_file).unwrap();
        assert!(raw.contains("level=\"warn\""));
        assert!(raw.contains("source=\"telegram\""));
        assert!(raw.contains("message=\"proxy rejected\""));
        assert!(raw.contains("context=\"check status timeout\""));
    }
}
