use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::Value;
use tempfile::TempDir;

use super::*;
use crate::model::{
    ProviderSource, ProviderSourceKind, TelegramBackendMode, TelegramProxy, WatcherSnapshot,
};
use crate::tdesktop::{
    DesktopProxy, DesktopProxyMode, DesktopProxySettings, DesktopProxyType,
    read_test_proxy_settings, seed_test_proxy_settings,
};

struct FixtureServer {
    base_url: String,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl FixtureServer {
    fn new(routes: HashMap<String, String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            while !stop_flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut request = [0_u8; 1024];
                        let _ = stream.read(&mut request);
                        let request_line = String::from_utf8_lossy(&request);
                        let path = request_line
                            .lines()
                            .next()
                            .and_then(|line| line.split_whitespace().nth(1))
                            .unwrap_or("/");
                        let (status, body) = routes
                            .get(path)
                            .map(|body| ("200 OK", body.as_str()))
                            .unwrap_or(("404 Not Found", "missing"));
                        let response = format!(
                            "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(response.as_bytes());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            base_url: format!("http://{addr}"),
            stop,
            handle: Some(handle),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct LiveProxyListener {
    port: u16,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl LiveProxyListener {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            while !stop_flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((_stream, _)) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            port,
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for LiveProxyListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct WatcherHarness {
    _root: TempDir,
    paths: AppPaths,
    tdata_dir: PathBuf,
    server: FixtureServer,
}

impl WatcherHarness {
    fn new(routes: HashMap<String, String>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_base_dirs(root.path().join("config"), root.path().join("data"));
        paths.ensure_dirs().unwrap();
        let tdata_dir = root.path().join("telegram").join("tdata");
        fs::create_dir_all(&tdata_dir).unwrap();
        Self {
            _root: root,
            paths,
            tdata_dir,
            server: FixtureServer::new(routes),
        }
    }

    fn config_with_sources(&self, sources: Vec<ProviderSource>) -> AppConfig {
        let mut config = AppConfig::default();
        config.telegram.backend_mode = TelegramBackendMode::Hybrid;
        config.telegram.data_dir = Some(self.tdata_dir.display().to_string());
        config.provider.sources = sources;
        config.provider.source_url = config
            .provider
            .sources
            .first()
            .map(|source| source.url.clone())
            .unwrap_or_default();
        config.provider.fetch_attempts = 1;
        config.provider.fetch_retry_delay_ms = 1;
        config.provider.enable_socks5_fallback = true;
        config.watcher.failure_threshold = 1;
        config.watcher.connect_timeout_secs = 1;
        config.watcher.history_size = 4;
        config
    }

    fn save_config_and_state(&self, config: &AppConfig, state: &AppState) {
        config.save(&self.paths.config_file).unwrap();
        state.save(&self.paths.state_file).unwrap();
    }
}

fn e2e_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
}

fn link_list_source(name: &str, url: String) -> ProviderSource {
    ProviderSource::new(name, url, ProviderSourceKind::TelegramLinkList)
}

fn mtproto_proxy(port: u16, secret: &str) -> TelegramProxy {
    TelegramProxy::mtproto("127.0.0.1", port, secret)
}

fn mtproto_proxy_at(server: &str, port: u16, secret: &str) -> TelegramProxy {
    TelegramProxy::mtproto(server, port, secret)
}

fn unused_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

fn user_proxy_settings() -> DesktopProxySettings {
    DesktopProxySettings {
        mode: DesktopProxyMode::Enabled,
        selected: Some(DesktopProxy {
            kind: DesktopProxyType::Socks5,
            host: "user-proxy.local".to_string(),
            port: 1080,
            user: "demo".to_string(),
            password: "pass".to_string(),
        }),
        list: vec![DesktopProxy {
            kind: DesktopProxyType::Socks5,
            host: "user-proxy.local".to_string(),
            port: 1080,
            user: "demo".to_string(),
            password: "pass".to_string(),
        }],
        ..DesktopProxySettings::default()
    }
}

fn user_and_managed_settings(managed: &TelegramProxy) -> DesktopProxySettings {
    let mut settings = user_proxy_settings();
    settings.list.push(DesktopProxy::from_managed(managed));
    settings.selected = Some(DesktopProxy::from_managed(managed));
    settings
}

fn status_json(paths: &AppPaths) -> Value {
    let (config, state, autostart) = load_status_snapshot(paths).unwrap();
    status_snapshot_json_value(&config, &state, &autostart)
}

#[test]
fn watcher_e2e_keeps_healthy_proxy_without_touching_settings() {
    let _serial = e2e_lock();
    let harness = WatcherHarness::new(HashMap::new());
    let live_proxy = LiveProxyListener::new();
    let config = harness.config_with_sources(Vec::new());
    let current_record =
        ProxyRecord::new(mtproto_proxy(live_proxy.port, "healthy-secret"), "current");
    let state = AppState {
        current_proxy: Some(current_record.clone()),
        watcher: WatcherSnapshot {
            mode: WatcherMode::Watching,
            ..WatcherSnapshot::default()
        },
        ..AppState::default()
    };

    seed_test_proxy_settings(&harness.tdata_dir, &user_proxy_settings()).unwrap();
    let before = read_test_proxy_settings(&harness.tdata_dir).unwrap();
    harness.save_config_and_state(&config, &state);

    let provider = MtProtoProvider::new(config.provider.clone()).unwrap();
    let _guard = telegram::override_is_running(false);
    let message = watch_cycle(&harness.paths, &config, &provider, true).unwrap();
    let after_state = AppState::load(&harness.paths).unwrap();
    let after_settings = read_test_proxy_settings(&harness.tdata_dir).unwrap();

    assert_eq!(message, "Proxy остаётся рабочим.");
    assert!(matches!(after_state.watcher.mode, WatcherMode::Watching));
    assert_eq!(
        after_state
            .current_proxy
            .as_ref()
            .map(|record| &record.proxy),
        Some(&current_record.proxy)
    );
    assert!(after_state.pending_proxy.is_none());
    assert!(after_state.source_status.contains("текущий proxy активен"));
    assert_eq!(before, after_settings);
}

#[test]
fn watcher_e2e_saves_pending_proxy_when_telegram_is_closed() {
    let _serial = e2e_lock();
    let live_candidate = LiveProxyListener::new();
    let candidate = mtproto_proxy(live_candidate.port, "replacement-secret");
    let candidate_link = candidate.deep_link();
    let harness = WatcherHarness::new(HashMap::from([(
        "/candidate.txt".to_string(),
        candidate_link,
    )]));
    let config = harness.config_with_sources(vec![link_list_source(
        "fixture-live",
        harness.server.url("/candidate.txt"),
    )]);
    let current_record = ProxyRecord::new(mtproto_proxy(unused_port(), "dead-secret"), "current");
    let state = AppState {
        current_proxy: Some(current_record.clone()),
        watcher: WatcherSnapshot {
            mode: WatcherMode::Watching,
            ..WatcherSnapshot::default()
        },
        ..AppState::default()
    };

    seed_test_proxy_settings(&harness.tdata_dir, &user_proxy_settings()).unwrap();
    let before = read_test_proxy_settings(&harness.tdata_dir).unwrap();
    harness.save_config_and_state(&config, &state);

    let provider = MtProtoProvider::new(config.provider.clone()).unwrap();
    let _guard = telegram::override_is_running(false);
    let message = watch_cycle(&harness.paths, &config, &provider, true).unwrap();
    let after_state = AppState::load(&harness.paths).unwrap();
    let after_settings = read_test_proxy_settings(&harness.tdata_dir).unwrap();

    assert!(message.is_empty());
    assert!(matches!(after_state.watcher.mode, WatcherMode::Watching));
    assert!(!after_state.backend_restart_required);
    assert_eq!(
        after_state
            .current_proxy
            .as_ref()
            .map(|record| &record.proxy),
        Some(&candidate)
    );
    assert!(after_state.pending_proxy.is_none());
    assert!(after_state.source_status.contains("Найден рабочий proxy"));
    assert_ne!(before, after_settings);
    assert_eq!(
        after_settings.selected.as_ref().map(|proxy| proxy.port),
        Some(live_candidate.port)
    );
}

#[test]
fn watcher_e2e_autoselects_first_proxy_without_manual_switch() {
    let _serial = e2e_lock();
    let live_candidate = LiveProxyListener::new();
    let candidate = mtproto_proxy(live_candidate.port, "first-managed-secret");
    let harness = WatcherHarness::new(HashMap::from([(
        "/first.txt".to_string(),
        candidate.deep_link(),
    )]));
    let config = harness.config_with_sources(vec![link_list_source(
        "fixture-first",
        harness.server.url("/first.txt"),
    )]);
    let state = AppState {
        watcher: WatcherSnapshot {
            mode: WatcherMode::Watching,
            ..WatcherSnapshot::default()
        },
        ..AppState::default()
    };

    seed_test_proxy_settings(&harness.tdata_dir, &user_proxy_settings()).unwrap();
    harness.save_config_and_state(&config, &state);

    let provider = MtProtoProvider::new(config.provider.clone()).unwrap();
    let _guard = telegram::override_is_running(false);
    let message = watch_cycle(&harness.paths, &config, &provider, true).unwrap();
    let after_state = AppState::load(&harness.paths).unwrap();
    let managed_settings = read_test_proxy_settings(&harness.tdata_dir).unwrap();

    assert!(message.is_empty());
    assert!(matches!(after_state.watcher.mode, WatcherMode::Watching));
    assert!(!after_state.backend_restart_required);
    assert!(after_state.pending_proxy.is_none());
    assert_eq!(
        after_state
            .current_proxy
            .as_ref()
            .map(|record| &record.proxy),
        Some(&candidate)
    );
    assert_eq!(
        managed_settings.selected.as_ref().map(|proxy| proxy.port),
        Some(live_candidate.port)
    );
    assert!(managed_settings.proxy_rotation_enabled);
}

#[test]
fn watcher_e2e_writes_managed_settings_when_telegram_is_open() {
    let _serial = e2e_lock();
    let live_candidate = LiveProxyListener::new();
    let candidate = mtproto_proxy(live_candidate.port, "replacement-secret");
    let old_managed = mtproto_proxy_at("127.0.0.2", 443, "old-secret");
    let candidate_link = candidate.deep_link();
    let harness = WatcherHarness::new(HashMap::from([(
        "/candidate.txt".to_string(),
        candidate_link,
    )]));
    let config = harness.config_with_sources(vec![link_list_source(
        "fixture-live",
        harness.server.url("/candidate.txt"),
    )]);
    let state = AppState {
        current_proxy: Some(ProxyRecord::new(old_managed.clone(), "current")),
        recent_proxies: std::collections::VecDeque::from([ProxyRecord::new(
            old_managed.clone(),
            "owned",
        )]),
        watcher: WatcherSnapshot {
            mode: WatcherMode::Watching,
            telegram_running: true,
            ..WatcherSnapshot::default()
        },
        ..AppState::default()
    };

    seed_test_proxy_settings(&harness.tdata_dir, &user_and_managed_settings(&old_managed)).unwrap();
    harness.save_config_and_state(&config, &state);

    let provider = MtProtoProvider::new(config.provider.clone()).unwrap();
    let _guard = telegram::override_is_running(true);
    assert_ne!(old_managed, candidate);
    assert!(telegram::check_proxy(
        &candidate,
        config.watcher.connect_timeout_secs
    ));
    watch_cycle(&harness.paths, &config, &provider, true).unwrap();

    let after_state = AppState::load(&harness.paths).unwrap();
    let managed_settings = read_test_proxy_settings(&harness.tdata_dir).unwrap();
    let doctor = doctor_snapshot_v2(&harness.paths).unwrap();
    let status = status_json(&harness.paths);

    assert!(matches!(after_state.watcher.mode, WatcherMode::Watching));
    assert!(!after_state.backend_restart_required);
    assert!(after_state.pending_proxy.is_none());
    assert_eq!(
        after_state
            .current_proxy
            .as_ref()
            .map(|record| &record.proxy),
        Some(&candidate)
    );
    assert!(
        after_state
            .current_proxy_status
            .contains("Telegram settings")
    );
    assert!(after_state.backend_route.contains("settingss"));
    assert_eq!(
        managed_settings
            .selected
            .as_ref()
            .map(|proxy| proxy.host.as_str()),
        Some("127.0.0.1")
    );
    assert!(managed_settings.list.iter().any(|proxy| {
        proxy.kind == DesktopProxyType::Socks5 && proxy.host == "user-proxy.local"
    }));
    assert!(managed_settings.list.iter().any(|proxy| {
        proxy.kind == DesktopProxyType::Mtproto
            && proxy.host == "127.0.0.1"
            && proxy.port == live_candidate.port
    }));
    assert!(!managed_settings.list.iter().any(|proxy| {
        proxy.kind == DesktopProxyType::Mtproto && proxy.password == "old-secret"
    }));
    assert!(managed_settings.proxy_rotation_enabled);
    assert!(!managed_settings.proxy_rotation_preferred_indices.is_empty());
    assert!(!doctor.backend_restart_required);
    assert!(doctor.backend_route.contains("settingss"));
    assert_eq!(
        status["state"]["backend_restart_required"],
        Value::Bool(false)
    );
    assert_eq!(
        status["state"]["watcher"]["mode"],
        Value::String("watching".to_string())
    );
    assert!(status["state"]["pending_proxy"].is_null());
}

#[test]
fn watcher_e2e_skips_dead_candidate_and_uses_next_live_source() {
    let _serial = e2e_lock();
    let dead_candidate = mtproto_proxy(unused_port(), "dead-candidate");
    let live_candidate = LiveProxyListener::new();
    let live_proxy = mtproto_proxy(live_candidate.port, "live-candidate");
    let harness = WatcherHarness::new(HashMap::from([
        ("/dead.txt".to_string(), dead_candidate.deep_link()),
        ("/live.txt".to_string(), live_proxy.deep_link()),
    ]));
    let config = harness.config_with_sources(vec![
        link_list_source("fixture-dead", harness.server.url("/dead.txt")),
        link_list_source("fixture-live", harness.server.url("/live.txt")),
    ]);
    let state = AppState {
        current_proxy: Some(ProxyRecord::new(
            mtproto_proxy(unused_port(), "current-dead"),
            "current",
        )),
        watcher: WatcherSnapshot {
            mode: WatcherMode::Watching,
            ..WatcherSnapshot::default()
        },
        ..AppState::default()
    };

    seed_test_proxy_settings(&harness.tdata_dir, &user_proxy_settings()).unwrap();
    harness.save_config_and_state(&config, &state);

    let provider = MtProtoProvider::new(config.provider.clone()).unwrap();
    let _guard = telegram::override_is_running(false);
    watch_cycle(&harness.paths, &config, &provider, true).unwrap();
    let after_state = AppState::load(&harness.paths).unwrap();

    assert_eq!(
        after_state
            .current_proxy
            .as_ref()
            .map(|record| &record.proxy),
        Some(&live_proxy)
    );
    assert!(after_state.pending_proxy.is_none());
    let log_output = fs::read_to_string(&harness.paths.log_file).unwrap();
    assert!(log_output.contains("candidate rejected"));
}

#[test]
fn watcher_e2e_marks_empty_sources_without_touching_settings() {
    let _serial = e2e_lock();
    let harness = WatcherHarness::new(HashMap::from([("/empty.txt".to_string(), String::new())]));
    let config = harness.config_with_sources(vec![link_list_source(
        "fixture-empty",
        harness.server.url("/empty.txt"),
    )]);
    let state = AppState {
        current_proxy: Some(ProxyRecord::new(
            mtproto_proxy(unused_port(), "dead-secret"),
            "current",
        )),
        watcher: WatcherSnapshot {
            mode: WatcherMode::Watching,
            ..WatcherSnapshot::default()
        },
        ..AppState::default()
    };

    seed_test_proxy_settings(&harness.tdata_dir, &user_proxy_settings()).unwrap();
    let before = read_test_proxy_settings(&harness.tdata_dir).unwrap();
    harness.save_config_and_state(&config, &state);

    let provider = MtProtoProvider::new(config.provider.clone()).unwrap();
    let _guard = telegram::override_is_running(false);
    let error = watch_cycle(&harness.paths, &config, &provider, true).unwrap_err();
    let after_state = AppState::load(&harness.paths).unwrap();
    let after_settings = read_test_proxy_settings(&harness.tdata_dir).unwrap();
    let doctor = doctor_snapshot_v2(&harness.paths).unwrap();

    assert!(!error.to_string().trim().is_empty());
    assert!(matches!(after_state.watcher.mode, WatcherMode::Error));
    assert!(after_state.source_status.contains("источник пуст"));
    assert!(after_state.pending_proxy.is_none());
    assert_eq!(before, after_settings);
    assert!(doctor.source_status.contains("источник пуст"));
}
