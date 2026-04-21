use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::Utc;

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
        let appdata = env::var("APPDATA").context("APPDATA не найден")?;
        let local_appdata = env::var("LOCALAPPDATA").context("LOCALAPPDATA не найден")?;
        Ok(Self::from_base_dirs(
            PathBuf::from(appdata),
            PathBuf::from(local_appdata),
        ))
    }

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
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_file)
            .with_context(|| format!("Не удалось открыть {}", self.log_file.display()))?;
        writeln!(file, "{} {}", Utc::now().to_rfc3339(), message.as_ref())
            .with_context(|| format!("Не удалось записать {}", self.log_file.display()))?;
        Ok(())
    }
}
