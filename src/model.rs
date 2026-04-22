use std::collections::VecDeque;
use std::fmt;
use std::fs;
use std::path::Path;

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::APP_VERSION;
use crate::paths::AppPaths;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MtProtoProxy {
    pub server: String,
    pub port: u16,
    pub secret: String,
}

impl MtProtoProxy {
    pub fn deep_link(&self) -> String {
        format!(
            "tg://proxy?server={}&port={}&secret={}",
            self.server, self.port, self.secret
        )
    }

    pub fn masked_secret(&self) -> String {
        if self.secret.len() <= 12 {
            return self.secret.clone();
        }

        let prefix = &self.secret[..8];
        let suffix = &self.secret[self.secret.len() - 4..];
        format!("{prefix}...{suffix}")
    }

    pub fn short_label(&self) -> String {
        format!("{}:{} ({})", self.server, self.port, self.masked_secret())
    }
}

impl fmt::Display for MtProtoProxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.server, self.port)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub app_version: String,
    pub provider: ProviderConfig,
    pub watcher: WatcherConfig,
    pub autostart: AutostartConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            app_version: APP_VERSION.to_string(),
            provider: ProviderConfig::default(),
            watcher: WatcherConfig::default(),
            autostart: AutostartConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn load(paths: &AppPaths) -> anyhow::Result<Self> {
        if !paths.config_file.exists() {
            return Ok(Self::default());
        }

        let raw = fs::read_to_string(&paths.config_file)
            .with_context(|| format!("Не удалось прочитать {}", paths.config_file.display()))?;
        let mut config: Self = toml::from_str(&raw)
            .with_context(|| format!("Не удалось разобрать {}", paths.config_file.display()))?;
        config.app_version = APP_VERSION.to_string();
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let body = toml::to_string_pretty(self).context("Не удалось сериализовать config.toml")?;
        fs::write(path, body).with_context(|| format!("Не удалось записать {}", path.display()))?;
        Ok(())
    }

    pub fn apply_overrides(&mut self, overrides: &InitOverrides) {
        if let Some(check_interval_secs) = overrides.check_interval_secs {
            self.watcher.check_interval_secs = check_interval_secs.max(5);
        }
        if let Some(connect_timeout_secs) = overrides.connect_timeout_secs {
            self.watcher.connect_timeout_secs = connect_timeout_secs.max(1);
        }
        if let Some(failure_threshold) = overrides.failure_threshold {
            self.watcher.failure_threshold = failure_threshold.max(1);
        }
        if let Some(history_size) = overrides.history_size {
            self.watcher.history_size = history_size.max(1);
        }
        if let Some(enabled) = overrides.autostart_enabled {
            self.autostart.enabled = enabled;
        }
        self.app_version = APP_VERSION.to_string();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub source_url: String,
    pub fetch_attempts: usize,
    pub fetch_retry_delay_ms: u64,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            source_url: "https://mtproto.ru/personal.php".to_string(),
            fetch_attempts: 8,
            fetch_retry_delay_ms: 1_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WatcherConfig {
    pub check_interval_secs: u64,
    pub connect_timeout_secs: u64,
    pub failure_threshold: u32,
    pub history_size: usize,
    pub auto_cleanup_dead_proxies: bool,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            check_interval_secs: 30,
            connect_timeout_secs: 4,
            failure_threshold: 3,
            history_size: 6,
            auto_cleanup_dead_proxies: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AutostartMethod {
    #[default]
    ScheduledTask,
    StartupFolder,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AutostartConfig {
    pub enabled: bool,
    pub method: AutostartMethod,
}

impl Default for AutostartConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            method: AutostartMethod::ScheduledTask,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AppState {
    pub current_proxy: Option<ProxyRecord>,
    pub pending_proxy: Option<ProxyRecord>,
    pub last_fetch_at: Option<DateTime<Utc>>,
    pub last_apply_at: Option<DateTime<Utc>>,
    pub current_proxy_status: String,
    pub source_status: String,
    pub watcher: WatcherSnapshot,
    pub recent_proxies: VecDeque<ProxyRecord>,
    pub last_error: Option<String>,
}

impl AppState {
    pub fn load(paths: &AppPaths) -> anyhow::Result<Self> {
        if !paths.state_file.exists() {
            return Ok(Self::default());
        }

        let raw = fs::read_to_string(&paths.state_file)
            .with_context(|| format!("Не удалось прочитать {}", paths.state_file.display()))?;
        let state: Self = serde_json::from_str(&raw)
            .with_context(|| format!("Не удалось разобрать {}", paths.state_file.display()))?;
        Ok(state)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let body =
            serde_json::to_string_pretty(self).context("Не удалось сериализовать state.json")?;
        fs::write(path, body).with_context(|| format!("Не удалось записать {}", path.display()))?;
        Ok(())
    }

    pub fn push_recent(&mut self, record: ProxyRecord, limit: usize) {
        self.recent_proxies
            .retain(|candidate| candidate.proxy != record.proxy);
        self.recent_proxies.push_front(record);
        while self.recent_proxies.len() > limit {
            self.recent_proxies.pop_back();
        }
    }

    pub fn recent_proxy_values(&self) -> Vec<MtProtoProxy> {
        let mut values = Vec::new();
        if let Some(current) = &self.current_proxy {
            values.push(current.proxy.clone());
        }
        if let Some(pending) = &self.pending_proxy {
            values.push(pending.proxy.clone());
        }
        for candidate in &self.recent_proxies {
            if !values.contains(&candidate.proxy) {
                values.push(candidate.proxy.clone());
            }
        }
        values
    }

    pub fn mark_healthy(&mut self) {
        self.watcher.failure_streak = 0;
        self.last_error = None;
        self.current_proxy_status = "работает".to_string();
    }

    pub fn mark_failure(&mut self) -> u32 {
        self.watcher.failure_streak = self.watcher.failure_streak.saturating_add(1);
        self.watcher.failure_streak
    }

    pub fn set_current_proxy_status(&mut self, status: impl Into<String>) {
        self.current_proxy_status = status.into();
    }

    pub fn set_source_status(&mut self, status: impl Into<String>) {
        self.source_status = status.into();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyRecord {
    pub proxy: MtProtoProxy,
    pub source: String,
    pub captured_at: DateTime<Utc>,
}

impl ProxyRecord {
    pub fn new(proxy: MtProtoProxy, source: impl Into<String>) -> Self {
        Self {
            proxy,
            source: source.into(),
            captured_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WatcherSnapshot {
    pub mode: WatcherMode,
    pub failure_streak: u32,
    pub telegram_running: bool,
    pub last_check_at: Option<DateTime<Utc>>,
    pub next_check_at: Option<DateTime<Utc>>,
}

impl Default for WatcherSnapshot {
    fn default() -> Self {
        Self {
            mode: WatcherMode::Idle,
            failure_streak: 0,
            telegram_running: false,
            last_check_at: None,
            next_check_at: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WatcherMode {
    #[default]
    Idle,
    Watching,
    WaitingForTelegram,
    Switching,
    Error,
}

#[derive(Debug, Clone, Default)]
pub struct InitOverrides {
    pub check_interval_secs: Option<u64>,
    pub connect_timeout_secs: Option<u64>,
    pub failure_threshold: Option<u32>,
    pub history_size: Option<usize>,
    pub autostart_enabled: Option<bool>,
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::paths::AppPaths;

    #[test]
    fn recent_history_is_deduplicated_and_trimmed() {
        let mut state = AppState::default();

        for index in 0..4 {
            state.push_recent(
                ProxyRecord::new(
                    MtProtoProxy {
                        server: format!("10.0.0.{index}"),
                        port: 443,
                        secret: format!("secret-{index}"),
                    },
                    "test",
                ),
                3,
            );
        }

        assert_eq!(state.recent_proxies.len(), 3);
        assert_eq!(
            state.recent_proxies.front().unwrap().proxy.server,
            "10.0.0.3"
        );
    }

    #[test]
    fn config_and_state_roundtrip() {
        let root = tempdir().unwrap();
        let appdata = root.path().join("app");
        let local = root.path().join("local");
        fs::create_dir_all(&appdata).unwrap();
        fs::create_dir_all(&local).unwrap();

        let paths = AppPaths::from_base_dirs(appdata, local);
        paths.ensure_dirs().unwrap();

        let mut config = AppConfig::default();
        config.autostart.enabled = true;
        config.autostart.method = AutostartMethod::StartupFolder;
        config.watcher.auto_cleanup_dead_proxies = false;
        config.save(&paths.config_file).unwrap();

        let loaded_config = AppConfig::load(&paths).unwrap();
        assert!(loaded_config.autostart.enabled);
        assert_eq!(
            loaded_config.autostart.method,
            AutostartMethod::StartupFolder
        );
        assert!(!loaded_config.watcher.auto_cleanup_dead_proxies);

        let mut state = AppState::default();
        state.mark_failure();
        state.save(&paths.state_file).unwrap();

        let loaded_state = AppState::load(&paths).unwrap();
        assert_eq!(loaded_state.watcher.failure_streak, 1);
    }
}
