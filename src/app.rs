use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

use anyhow::Context;
use chrono::{Duration as ChronoDuration, Local, Utc};
use clap::Parser;
use serde::Serialize;
use sysinfo::{ProcessesToUpdate, System};

use crate::cli::{
    AutostartCommand, Cli, Commands, DoctorArgs, InitArgs, StatusArgs, SwitchArgs, WatchArgs,
};
use crate::model::{
    AppConfig, AppState, AutostartMethod, InitOverrides, MtProtoProxy, ProxyRecord, WatcherMode,
};
use crate::paths::AppPaths;
use crate::provider::MtProtoProvider;
use crate::telegram;
use crate::ui;
use crate::windows;
use crate::{APP_NAME, APP_VERSION};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DoctorSnapshot {
    pub app_version: String,
    pub config_exists: bool,
    pub state_exists: bool,
    pub log_exists: bool,
    pub tg_protocol_handler: Option<String>,
    pub telegram_executable: Option<String>,
    pub telegram_running: bool,
    pub autostart: windows::AutostartStatus,
    pub enabled_sources: Vec<String>,
    pub provider_probe: Result<String, String>,
}

pub fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let paths = AppPaths::resolve()?;
    paths.ensure_dirs()?;

    match cli.command {
        Some(Commands::Init(args)) => handle_init(&paths, args),
        Some(Commands::Watch(args)) => handle_watch(&paths, args),
        Some(Commands::Status(args)) => handle_status(&paths, args),
        Some(Commands::Switch(args)) => handle_switch(&paths, args),
        Some(Commands::Cleanup) => handle_cleanup(&paths),
        Some(Commands::Doctor(args)) => handle_doctor(&paths, args),
        Some(Commands::Autostart { command }) => handle_autostart(&paths, command),
        None => handle_launch(&paths),
    }
}

fn handle_launch(paths: &AppPaths) -> anyhow::Result<()> {
    if !paths.config_file.exists() {
        handle_init(
            paths,
            InitArgs {
                non_interactive: !ui::stdout_is_terminal(),
                autostart: false,
                no_autostart: false,
                check_interval: None,
                connect_timeout: None,
                failure_threshold: None,
                history_size: None,
            },
        )?;
    }

    let _ = ensure_watcher_running(paths)?;

    handle_status(
        paths,
        StatusArgs {
            plain: !ui::stdout_is_terminal(),
            json: false,
        },
    )
}

fn handle_init(paths: &AppPaths, args: InitArgs) -> anyhow::Result<()> {
    let mut config = AppConfig::load(paths)?;
    config.apply_overrides(&InitOverrides {
        check_interval_secs: args.check_interval,
        connect_timeout_secs: args.connect_timeout,
        failure_threshold: args.failure_threshold,
        history_size: args.history_size,
        autostart_enabled: match (args.autostart, args.no_autostart) {
            (true, false) => Some(true),
            (false, true) => Some(false),
            _ => None,
        },
    });
    let config = if args.non_interactive || !ui::stdout_is_terminal() {
        config
    } else {
        ui::run_setup(config)?
    };
    if !paths.state_file.exists() {
        AppState::default().save(&paths.state_file)?;
    }
    persist_config_with_restart(paths, config, false)?;
    let config = AppConfig::load(paths)?;
    let telegram_info = telegram::detect_installation()?;
    paths.append_log("init completed")?;
    println!("{} {} инициализирован.", APP_NAME, APP_VERSION);
    println!("config.toml: {}", paths.config_file.display());
    println!("state.json: {}", paths.state_file.display());
    println!("watch.log: {}", paths.log_file.display());
    println!(
        "tg:// handler: {}",
        if telegram_info.protocol_handler.is_some() {
            "найден"
        } else {
            "не найден"
        }
    );
    println!(
        "Telegram Desktop: {}",
        telegram_info
            .executable_path
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "не найден".to_string())
    );
    println!(
        "Автозапуск: {}",
        if config.autostart.enabled {
            format!("вкл ({})", autostart_method_label(&config.autostart.method))
        } else {
            "выкл".to_string()
        }
    );
    Ok(())
}

fn handle_watch(paths: &AppPaths, args: WatchArgs) -> anyhow::Result<()> {
    let config = AppConfig::load(paths)?;
    let provider = MtProtoProvider::new(config.provider.clone())?;
    let interval = Duration::from_secs(config.watcher.check_interval_secs.max(5));

    loop {
        match watch_cycle(paths, &config, &provider, args.headless) {
            Ok(report) if !args.headless => println!("{report}"),
            Ok(_) => {}
            Err(error) => {
                let _ = paths.append_log(format!("watch error: {error:#}"));
                let _ = persist_watch_error(paths, &error.to_string());
                if !args.headless {
                    eprintln!("watch error: {error:#}");
                }
            }
        }

        if args.once {
            break;
        }

        thread::sleep(interval);
    }

    Ok(())
}

fn handle_status(paths: &AppPaths, args: StatusArgs) -> anyhow::Result<()> {
    let (config, state, autostart) = load_status_snapshot(paths)?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "config": config,
                "state": state,
                "autostart": autostart,
            }))
            .context("Не удалось сериализовать status")?
        );
        return Ok(());
    }

    if !args.plain && ui::stdout_is_terminal() {
        return ui::run_status(paths);
    }

    print_plain_status_v2(paths, &config, &state, &autostart);
    Ok(())
}

fn handle_switch(paths: &AppPaths, args: SwitchArgs) -> anyhow::Result<()> {
    println!("{}", switch_to_candidate(paths, args.dry_run)?);
    Ok(())
}

fn handle_cleanup(paths: &AppPaths) -> anyhow::Result<()> {
    println!("{}", cleanup_dead_proxies(paths)?);
    Ok(())
}

fn handle_doctor(paths: &AppPaths, args: DoctorArgs) -> anyhow::Result<()> {
    let report = doctor_snapshot_v2(paths)?;
    let provider_probe_display = match &report.provider_probe {
        Ok(proxy) => format!("ok ({proxy})"),
        Err(error) => format!("error ({error})"),
    };

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).context("Не удалось сериализовать doctor")?
        );
        return Ok(());
    }

    println!("ProtoSwitch {}", APP_VERSION);
    println!("config.toml: {}", yes_no(paths.config_file.exists()));
    println!("state.json: {}", yes_no(paths.state_file.exists()));
    println!("watch.log: {}", yes_no(paths.log_file.exists()));
    println!(
        "tg:// handler: {}",
        if report.tg_protocol_handler.is_none() {
            "не найден"
        } else {
            "найден"
        }
    );
    println!(
        "Telegram Desktop: {}",
        report.telegram_executable.as_deref().unwrap_or("не найден")
    );
    println!("Telegram запущен: {}", yes_no(report.telegram_running));
    println!(
        "Автозапуск: {}",
        if report.autostart.installed {
            format!(
                "да ({})",
                report
                    .autostart
                    .method
                    .as_ref()
                    .map(autostart_method_label)
                    .unwrap_or("unknown")
            )
        } else {
            "нет".to_string()
        }
    );
    if let Some(target) = report.autostart.target {
        println!("Цель автозапуска: {target}");
    }
    println!("mtproto.ru: {provider_probe_display}");

    Ok(())
}

fn handle_autostart(paths: &AppPaths, command: AutostartCommand) -> anyhow::Result<()> {
    match command {
        AutostartCommand::Install => {
            println!("{}", set_autostart_enabled(paths, true)?);
        }
        AutostartCommand::Remove => {
            println!("{}", set_autostart_enabled(paths, false)?);
        }
    }

    Ok(())
}

fn watch_cycle(
    paths: &AppPaths,
    config: &AppConfig,
    provider: &MtProtoProvider,
    headless: bool,
) -> anyhow::Result<String> {
    let mut state = AppState::load(paths)?;
    let now = Utc::now();
    let telegram_running = telegram::is_running().unwrap_or(false);
    state.watcher.telegram_running = telegram_running;
    state.watcher.last_check_at = Some(now);
    state.watcher.next_check_at = Some(
        now + ChronoDuration::seconds(config.watcher.check_interval_secs.try_into().unwrap_or(30)),
    );
    if state.current_proxy_status.trim().is_empty() {
        state.set_current_proxy_status(if state.current_proxy.is_some() {
            "awaiting check"
        } else {
            "not selected"
        });
    }
    if state.source_status.trim().is_empty() {
        state.set_source_status("источник ещё не опрашивался");
    }
    if telegram_running {
        if let Some(record) = state.pending_proxy.clone() {
            match apply_proxy_record(
                paths,
                config,
                &mut state,
                record,
                "Использован сохранённый pending proxy".to_string(),
                "Pending proxy применён",
            ) {
                Ok(message) => return Ok(message),
                Err(error) => {
                    let message = error.to_string();
                    let _ = paths.append_log(format!("pending proxy apply rejected: {message}"));
                }
            }
        }
    }
    let current_is_healthy = state
        .current_proxy
        .as_ref()
        .map(|record| telegram::check_proxy(&record.proxy, config.watcher.connect_timeout_secs))
        .unwrap_or(false);
    if current_is_healthy {
        state.watcher.mode = WatcherMode::Watching;
        state.mark_healthy();
        state.set_source_status("источник не запрашивался");
        state.save(&paths.state_file)?;
        return Ok("Proxy остаётся рабочим.".to_string());
    }
    if state.current_proxy.is_some() {
        let failure_streak = state.mark_failure();
        state.set_current_proxy_status(format!(
            "не отвечает ({failure_streak}/{})",
            config.watcher.failure_threshold
        ));
    } else {
        state.watcher.failure_streak = config.watcher.failure_threshold;
        state.set_current_proxy_status("не выбран");
    }
    if state.watcher.failure_streak < config.watcher.failure_threshold {
        state.watcher.mode = WatcherMode::Watching;
        state.save(&paths.state_file)?;
        return Ok(format!(
            "Proxy недоступен, ждём порог: {} / {}",
            state.watcher.failure_streak, config.watcher.failure_threshold
        ));
    }
    if state.pending_proxy.is_some() && !telegram_running {
        state.watcher.mode = WatcherMode::WaitingForTelegram;
        state.set_current_proxy_status("текущий proxy не работает");
        state.set_source_status("есть pending proxy, ждём Telegram");
        state.save(&paths.state_file)?;
        return Ok("Есть pending proxy, ждём запуска Telegram.".to_string());
    }
    if telegram_running {
        return apply_candidate_with_retries(
            paths,
            config,
            provider,
            &mut state,
            "Watcher переключил proxy",
        );
    }
    let record =
        match fetch_validated_candidate(paths, config, provider, &state.recent_proxy_values()) {
            Ok(record) => record,
            Err(error) => {
                state.watcher.mode = WatcherMode::Error;
                state.set_source_status(error.to_string());
                if state.current_proxy.is_some() {
                    state.set_current_proxy_status(
                        "текущий proxy не работает, replacement не найден",
                    );
                } else {
                    state.set_current_proxy_status("рабочий proxy не найден");
                }
                state.save(&paths.state_file)?;
                return Err(error);
            }
        };
    state.last_fetch_at = Some(record.captured_at);
    state.push_recent(record.clone(), config.watcher.history_size);
    state.set_source_status(format!(
        "найден рабочий proxy: {}",
        record.proxy.short_label()
    ));
    state.pending_proxy = Some(record.clone());
    state.watcher.mode = WatcherMode::WaitingForTelegram;
    state.set_current_proxy_status(if state.current_proxy.is_some() {
        "текущий proxy не работает"
    } else {
        "текущий proxy не выбран"
    });
    state.set_source_status(format!(
        "найден replacement proxy, ждём Telegram: {}",
        record.proxy.short_label()
    ));
    state.save(&paths.state_file)?;
    paths.append_log(format!(
        "watch captured pending proxy while telegram offline {}",
        record.proxy.short_label()
    ))?;
    if headless {
        Ok(String::new())
    } else {
        Ok(format!(
            "Telegram не запущен. Pending proxy сохранён: {}",
            record.proxy.short_label()
        ))
    }
}

#[allow(dead_code)]
fn print_plain_status(
    paths: &AppPaths,
    config: &AppConfig,
    state: &AppState,
    autostart: &windows::AutostartStatus,
) {
    println!("ProtoSwitch {}", APP_VERSION);
    println!("Статус proxy: {}", current_proxy_status_text(state));
    println!("Статус источника: {}", source_status_text(state));
    println!("Источник: {}", config.provider.source_url);
    println!(
        "Текущий proxy: {}",
        state
            .current_proxy
            .as_ref()
            .map(|entry| entry.proxy.short_label())
            .unwrap_or_else(|| "не выбран".to_string())
    );
    println!(
        "Pending proxy: {}",
        state
            .pending_proxy
            .as_ref()
            .map(|entry| entry.proxy.short_label())
            .unwrap_or_else(|| "нет".to_string())
    );
    println!("Режим watcher: {}", watcher_mode(&state.watcher.mode));
    println!(
        "Telegram запущен: {}",
        yes_no(state.watcher.telegram_running)
    );
    println!(
        "Последний fetch: {}",
        state
            .last_fetch_at
            .as_ref()
            .map(format_local_time)
            .unwrap_or_else(|| "нет данных".to_string())
    );
    println!(
        "Последний apply: {}",
        state
            .last_apply_at
            .as_ref()
            .map(format_local_time)
            .unwrap_or_else(|| "нет данных".to_string())
    );
    println!(
        "Следующая проверка: {}",
        state
            .watcher
            .next_check_at
            .as_ref()
            .map(format_local_time)
            .unwrap_or_else(|| "нет данных".to_string())
    );
    println!(
        "Автозапуск: {}",
        if autostart.installed {
            format!(
                "да ({})",
                autostart
                    .method
                    .as_ref()
                    .map(autostart_method_label)
                    .unwrap_or("unknown")
            )
        } else if config.autostart.enabled {
            format!(
                "ожидается ({})",
                autostart_method_label(&config.autostart.method)
            )
        } else {
            "нет".to_string()
        }
    );
    println!(
        "Автоподчистка proxy: {}",
        if config.watcher.auto_cleanup_dead_proxies {
            "вкл"
        } else {
            "выкл"
        }
    );
    if let Some(target) = &autostart.target {
        println!("Цель автозапуска: {target}");
    }
    println!("config.toml: {}", paths.config_file.display());
    println!("state.json: {}", paths.state_file.display());
    println!("watch.log: {}", paths.log_file.display());
    if let Some(error) = &state.last_error {
        println!("Последняя ошибка: {error}");
    }
}

fn print_plain_status_v2(
    paths: &AppPaths,
    config: &AppConfig,
    state: &AppState,
    autostart: &windows::AutostartStatus,
) {
    println!("ProtoSwitch {}", APP_VERSION);
    println!("Статус proxy: {}", current_proxy_status_text(state));
    println!("Статус источника: {}", source_status_text(state));
    println!("Пул источников: {}", provider_pool_summary(config));
    println!("Включённые источники: {}", enabled_sources_summary(config));
    println!(
        "Текущий proxy: {}",
        state
            .current_proxy
            .as_ref()
            .map(|entry| entry.proxy.short_label())
            .unwrap_or_else(|| "не выбран".to_string())
    );
    println!(
        "Pending proxy: {}",
        state
            .pending_proxy
            .as_ref()
            .map(|entry| entry.proxy.short_label())
            .unwrap_or_else(|| "нет".to_string())
    );
    println!("Режим watcher: {}", watcher_mode(&state.watcher.mode));
    println!(
        "Telegram запущен: {}",
        yes_no(state.watcher.telegram_running)
    );
    if let Some(record) = &state.current_proxy {
        println!("Последний успешный источник: {}", record.source);
    }
    println!(
        "Последний fetch: {}",
        state
            .last_fetch_at
            .as_ref()
            .map(format_local_time)
            .unwrap_or_else(|| "нет данных".to_string())
    );
    println!(
        "Последний apply: {}",
        state
            .last_apply_at
            .as_ref()
            .map(format_local_time)
            .unwrap_or_else(|| "нет данных".to_string())
    );
    println!(
        "Следующая проверка: {}",
        state
            .watcher
            .next_check_at
            .as_ref()
            .map(format_local_time)
            .unwrap_or_else(|| "нет данных".to_string())
    );
    println!(
        "Автозапуск: {}",
        if autostart.installed {
            format!(
                "да ({})",
                autostart
                    .method
                    .as_ref()
                    .map(autostart_method_label)
                    .unwrap_or("unknown")
            )
        } else if config.autostart.enabled {
            format!(
                "ожидается ({})",
                autostart_method_label(&config.autostart.method)
            )
        } else {
            "нет".to_string()
        }
    );
    println!(
        "Автоподчистка proxy: {}",
        if config.watcher.auto_cleanup_dead_proxies {
            "вкл"
        } else {
            "выкл"
        }
    );
    println!(
        "SOCKS5 fallback: {}",
        if config.provider.enable_socks5_fallback {
            "вкл"
        } else {
            "выкл"
        }
    );
    if let Some(target) = &autostart.target {
        println!("Цель автозапуска: {target}");
    }
    println!("config.toml: {}", paths.config_file.display());
    println!("state.json: {}", paths.state_file.display());
    println!("watch.log: {}", paths.log_file.display());
    if let Some(error) = &state.last_error {
        println!("Последняя ошибка: {error}");
    }
}

pub(crate) fn current_proxy_status_text(state: &AppState) -> String {
    if !state.current_proxy_status.trim().is_empty() {
        return state.current_proxy_status.clone();
    }

    if state.pending_proxy.is_some() {
        return "есть pending proxy".to_string();
    }

    if state.current_proxy.is_some() {
        return "ожидает проверки".to_string();
    }

    "не выбран".to_string()
}

pub(crate) fn source_status_text(state: &AppState) -> String {
    if !state.source_status.trim().is_empty() {
        return state.source_status.clone();
    }

    "нет данных".to_string()
}

fn dead_managed_proxies(state: &AppState, timeout_secs: u64) -> Vec<MtProtoProxy> {
    let mut values = Vec::new();

    let mut push_if_dead = |proxy: &MtProtoProxy| {
        if values.contains(proxy) {
            return;
        }
        if !telegram::check_proxy(proxy, timeout_secs) {
            values.push(proxy.clone());
        }
    };

    if let Some(record) = &state.current_proxy {
        push_if_dead(&record.proxy);
    }
    if let Some(record) = &state.pending_proxy {
        push_if_dead(&record.proxy);
    }
    for record in &state.recent_proxies {
        push_if_dead(&record.proxy);
    }

    values
}

fn cleanup_dead_proxies_in_state(
    paths: &AppPaths,
    config: &AppConfig,
    state: &mut AppState,
) -> anyhow::Result<usize> {
    let mut dead = dead_managed_proxies(state, config.watcher.connect_timeout_secs);
    if config.watcher.auto_cleanup_dead_proxies && state.watcher.telegram_running {
        match telegram::detect_unavailable_proxies(
            &state.recent_proxy_values(),
            config.watcher.connect_timeout_secs,
        ) {
            Ok(unavailable) => {
                for proxy in unavailable {
                    if !dead.contains(&proxy) {
                        dead.push(proxy);
                    }
                }
            }
            Err(error) => {
                let _ = paths.append_log(format!("telegram status probe skipped: {error}"));
            }
        }
    }
    if dead.is_empty() {
        return Ok(0);
    }

    let removed = telegram::remove_proxies(&dead)?;
    if removed == 0 {
        return Ok(0);
    }

    state
        .recent_proxies
        .retain(|record| !dead.contains(&record.proxy));
    if state
        .current_proxy
        .as_ref()
        .map(|record| dead.contains(&record.proxy))
        .unwrap_or(false)
    {
        state.current_proxy = None;
        state.set_current_proxy_status("не выбран");
    }
    if state
        .pending_proxy
        .as_ref()
        .map(|record| dead.contains(&record.proxy))
        .unwrap_or(false)
    {
        state.pending_proxy = None;
    }

    state.set_source_status(format!("очистка Telegram: удалено {removed} proxy"));
    paths.append_log(format!("telegram cleanup removed {removed} dead proxies"))?;
    Ok(removed)
}

pub(crate) fn cleanup_dead_proxies(paths: &AppPaths) -> anyhow::Result<String> {
    let config = AppConfig::load(paths)?;
    let mut state = AppState::load(paths)?;
    let removed = cleanup_dead_proxies_in_state(paths, &config, &mut state)?;
    state.save(&paths.state_file)?;

    Ok(if removed == 0 {
        "Мёртвых proxy в управляемом списке не найдено.".to_string()
    } else {
        format!("Удалено мёртвых proxy из Telegram: {removed}")
    })
}

fn try_cleanup_dead_proxies(paths: &AppPaths, config: &AppConfig, state: &mut AppState) {
    if !config.watcher.auto_cleanup_dead_proxies {
        return;
    }
    if let Err(error) = cleanup_dead_proxies_in_state(paths, config, state) {
        let _ = paths.append_log(format!("telegram cleanup skipped: {error}"));
    }
}

pub(crate) fn load_status_snapshot(
    paths: &AppPaths,
) -> anyhow::Result<(AppConfig, AppState, windows::AutostartStatus)> {
    let config = AppConfig::load(paths)?;
    let mut state = AppState::load(paths)?;
    state.watcher.telegram_running = telegram::is_running().unwrap_or(false);
    let autostart = windows::query_autostart();
    Ok((config, state, autostart))
}

fn validate_candidate(record: ProxyRecord, timeout_secs: u64) -> anyhow::Result<ProxyRecord> {
    if telegram::check_proxy(&record.proxy, timeout_secs) {
        return Ok(record);
    }

    Err(anyhow::anyhow!(
        "Источник вернул proxy, но TCP-проверка не прошла: {}",
        record.proxy.short_label()
    ))
}

fn fetch_validated_candidate(
    paths: &AppPaths,
    config: &AppConfig,
    provider: &MtProtoProvider,
    recent: &[MtProtoProxy],
) -> anyhow::Result<ProxyRecord> {
    let mut rejected = recent.to_vec();
    let max_attempts = config.watcher.history_size.max(3).min(8);
    let mut last_error = None;

    for _ in 0..max_attempts {
        let record = provider.fetch_candidate(&rejected)?;
        rejected.push(record.proxy.clone());
        match validate_candidate(record, config.watcher.connect_timeout_secs) {
            Ok(record) => return Ok(record),
            Err(error) => {
                let message = error.to_string();
                let _ = paths.append_log(format!("candidate rejected: {message}"));
                last_error = Some(error);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Рабочий proxy не найден")))
}

fn store_apply_failure(
    paths: &AppPaths,
    config: &AppConfig,
    state: &mut AppState,
    record: &ProxyRecord,
    current_status: String,
    source_status: String,
    error_message: String,
    log_message: String,
) -> anyhow::Result<()> {
    let _ = telegram::remove_proxies(std::slice::from_ref(&record.proxy));
    state.pending_proxy = None;
    state.watcher.mode = WatcherMode::Error;
    state.watcher.failure_streak = config.watcher.failure_threshold;
    state.last_error = Some(error_message);
    state.set_current_proxy_status(current_status);
    state.set_source_status(source_status);
    state.push_recent(record.clone(), config.watcher.history_size);
    state.save(&paths.state_file)?;
    paths.append_log(log_message)?;
    Ok(())
}

fn apply_proxy_record(
    paths: &AppPaths,
    config: &AppConfig,
    state: &mut AppState,
    record: ProxyRecord,
    source_status: String,
    success_prefix: &str,
) -> anyhow::Result<String> {
    state.push_recent(record.clone(), config.watcher.history_size);
    if let Err(error) =
        telegram::open_proxy_link(&record.proxy, config.watcher.connect_timeout_secs)
    {
        let details = error.to_string();
        store_apply_failure(
            paths,
            config,
            state,
            &record,
            format!("Telegram не подтвердил добавление proxy: {details}"),
            format!(
                "Источник дал кандидата {}, но диалог Telegram завершился ошибкой: {details}",
                record.proxy.short_label()
            ),
            format!(
                "Telegram не подтвердил добавление proxy: {}",
                record.proxy.short_label()
            ),
            format!(
                "telegram dialog rejected proxy {} with error {details}",
                record.proxy.short_label()
            ),
        )?;
        return Err(anyhow::anyhow!(
            "Telegram не подтвердил добавление proxy: {}",
            record.proxy.short_label()
        ));
    }
    let telegram_status =
        telegram::settle_proxy_status(&record.proxy, config.watcher.connect_timeout_secs)?;
    match telegram_status {
        telegram::ManagedProxyStatus::Unavailable(details) => {
            store_apply_failure(
                paths,
                config,
                state,
                &record,
                format!("Telegram отклонил proxy: {details}"),
                format!(
                    "Источник дал кандидата {}, но Telegram показал статус: {details}",
                    record.proxy.short_label()
                ),
                format!(
                    "Telegram пометил proxy как недоступный: {}",
                    record.proxy.short_label()
                ),
                format!(
                    "telegram rejected proxy {} with status {details}",
                    record.proxy.short_label()
                ),
            )?;
            Err(anyhow::anyhow!(
                "Telegram пометил proxy как недоступный: {}",
                record.proxy.short_label()
            ))
        }
        telegram::ManagedProxyStatus::Available(details) => {
            state.current_proxy = Some(record.clone());
            state.pending_proxy = None;
            state.last_fetch_at.get_or_insert(record.captured_at);
            state.last_apply_at = Some(Utc::now());
            state.watcher.mode = WatcherMode::Switching;
            state.watcher.telegram_running = true;
            state.watcher.failure_streak = 0;
            state.last_error = None;
            state.set_current_proxy_status(format!("подключён, Telegram: {details}"));
            state.set_source_status(source_status);
            try_cleanup_dead_proxies(paths, config, state);
            state.save(&paths.state_file)?;
            paths.append_log(format!(
                "proxy applied {} with telegram status {details}",
                record.proxy.short_label()
            ))?;
            Ok(format!("{success_prefix}: {}", record.proxy.short_label()))
        }
        telegram::ManagedProxyStatus::Checking(details)
        | telegram::ManagedProxyStatus::Unknown(details) => {
            store_apply_failure(
                paths,
                config,
                state,
                &record,
                format!("Telegram не подтвердил подключение proxy: {details}"),
                format!(
                    "Источник дал кандидата {}, но Telegram не подтвердил подключение: {details}",
                    record.proxy.short_label()
                ),
                format!(
                    "Telegram не подтвердил подключение proxy: {}",
                    record.proxy.short_label()
                ),
                format!(
                    "telegram did not settle proxy {} and returned status {details}",
                    record.proxy.short_label()
                ),
            )?;
            Err(anyhow::anyhow!(
                "Telegram не подтвердил подключение proxy: {}",
                record.proxy.short_label()
            ))
        }
        telegram::ManagedProxyStatus::Missing => {
            store_apply_failure(
                paths,
                config,
                state,
                &record,
                "Telegram не сохранил proxy в списке".to_string(),
                format!(
                    "Источник дал кандидата {}, но Telegram не сохранил proxy",
                    record.proxy.short_label()
                ),
                format!("Telegram не сохранил proxy: {}", record.proxy.short_label()),
                format!(
                    "telegram did not persist proxy {} after apply",
                    record.proxy.short_label()
                ),
            )?;
            Err(anyhow::anyhow!(
                "Telegram не сохранил proxy: {}",
                record.proxy.short_label()
            ))
        }
    }
}

fn apply_candidate_with_retries(
    paths: &AppPaths,
    config: &AppConfig,
    provider: &MtProtoProvider,
    state: &mut AppState,
    success_prefix: &str,
) -> anyhow::Result<String> {
    let mut rejected = state.recent_proxy_values();
    let attempts = config.watcher.history_size.max(3).min(6);
    let mut last_error = None;

    for _ in 0..attempts {
        let record = fetch_validated_candidate(paths, config, provider, &rejected)?;
        rejected.push(record.proxy.clone());
        state.last_fetch_at = Some(record.captured_at);
        match apply_proxy_record(
            paths,
            config,
            state,
            record.clone(),
            format!("Найден рабочий proxy: {}", record.proxy.short_label()),
            success_prefix,
        ) {
            Ok(message) => return Ok(message),
            Err(error) => {
                let message = error.to_string();
                let _ = paths.append_log(format!("candidate apply rejected: {message}"));
                last_error = Some(error);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Рабочий proxy не найден")))
}

pub(crate) fn switch_to_candidate(paths: &AppPaths, dry_run: bool) -> anyhow::Result<String> {
    let config = AppConfig::load(paths)?;
    let provider = MtProtoProvider::new(config.provider.clone())?;
    let mut state = AppState::load(paths)?;
    let recent = state.recent_proxy_values();
    let record = match fetch_validated_candidate(paths, &config, &provider, &recent) {
        Ok(record) => record,
        Err(error) => {
            state.set_source_status(error.to_string());
            state.save(&paths.state_file)?;
            return Err(error);
        }
    };
    if dry_run {
        return Ok(format!("Новый proxy: {}", record.proxy.deep_link()));
    }
    apply_candidate_with_retries(paths, &config, &provider, &mut state, "Применён proxy")
}

pub(crate) fn apply_pending_proxy(paths: &AppPaths) -> anyhow::Result<String> {
    let mut state = AppState::load(paths)?;
    let Some(record) = state.pending_proxy.clone() else {
        return Err(anyhow::anyhow!("Нет pending proxy для применения"));
    };
    if !telegram::is_running().unwrap_or(false) {
        return Err(anyhow::anyhow!("Telegram Desktop сейчас не запущен"));
    }
    let config = AppConfig::load(paths)?;
    apply_proxy_record(
        paths,
        &config,
        &mut state,
        record,
        "Использован сохранённый pending proxy".to_string(),
        "Pending proxy применён",
    )
}

pub(crate) fn doctor_snapshot(paths: &AppPaths) -> anyhow::Result<DoctorSnapshot> {
    doctor_snapshot_v2(paths)
}

pub(crate) fn doctor_snapshot_v2(paths: &AppPaths) -> anyhow::Result<DoctorSnapshot> {
    let config = AppConfig::load(paths)?;
    let provider = MtProtoProvider::new(config.provider.clone())?;
    let installation = telegram::detect_installation()?;
    let autostart = windows::query_autostart();
    let provider_probe =
        fetch_validated_candidate(paths, &config, &provider, &[] as &[MtProtoProxy])
            .map(|record| format!("{} via {}", record.proxy.short_label(), record.source))
            .map_err(|error| error.to_string());

    Ok(DoctorSnapshot {
        app_version: APP_VERSION.to_string(),
        config_exists: paths.config_file.exists(),
        state_exists: paths.state_file.exists(),
        log_exists: paths.log_file.exists(),
        tg_protocol_handler: installation.protocol_handler,
        telegram_executable: installation
            .executable_path
            .map(|path| path.display().to_string()),
        telegram_running: telegram::is_running().unwrap_or(false),
        autostart,
        enabled_sources: config
            .provider
            .active_sources()
            .into_iter()
            .map(|source| source.name)
            .collect(),
        provider_probe,
    })
}

pub(crate) fn set_autostart_enabled(paths: &AppPaths, enabled: bool) -> anyhow::Result<String> {
    let mut config = AppConfig::load(paths)?;
    config.autostart.enabled = enabled;
    persist_config_with_restart(paths, config, false)
}

pub(crate) fn set_auto_cleanup_enabled(paths: &AppPaths, enabled: bool) -> anyhow::Result<String> {
    let mut config = AppConfig::load(paths)?;
    config.watcher.auto_cleanup_dead_proxies = enabled;
    persist_config(paths, config)
}

pub(crate) fn set_socks5_fallback_enabled(
    paths: &AppPaths,
    enabled: bool,
) -> anyhow::Result<String> {
    let mut config = AppConfig::load(paths)?;
    config.provider.enable_socks5_fallback = enabled;
    persist_config(paths, config)
}

pub(crate) fn persist_config(paths: &AppPaths, config: AppConfig) -> anyhow::Result<String> {
    persist_config_with_restart(paths, config, true)
}

fn persist_config_with_restart(
    paths: &AppPaths,
    mut config: AppConfig,
    restart_watcher: bool,
) -> anyhow::Result<String> {
    if config.autostart.enabled {
        let method = windows::install_autostart(
            &std::env::current_exe().context("Не удалось определить путь к protoswitch.exe")?,
        )?;
        config.autostart.method = method;
    } else {
        let _ = windows::remove_autostart();
        config.autostart.method = AutostartMethod::ScheduledTask;
    }

    config.save(&paths.config_file)?;
    if restart_watcher {
        let _ = restart_background_watcher(paths);
    }

    Ok(if config.autostart.enabled {
        format!(
            "Настройки сохранены. Автозапуск: вкл ({})",
            autostart_method_label(&config.autostart.method)
        )
    } else {
        "Настройки сохранены. Автозапуск: выкл".to_string()
    })
}

pub(crate) fn ensure_watcher_running(paths: &AppPaths) -> anyhow::Result<bool> {
    let config = AppConfig::load(paths)?;
    let state = AppState::load(paths)?;

    if watcher_is_recent(&config, &state) || watcher_process_exists() {
        return Ok(false);
    }

    spawn_background_watcher(paths)?;
    Ok(true)
}

pub(crate) fn stop_background_watcher(paths: &AppPaths) -> anyhow::Result<usize> {
    let mut system = System::new_all();
    system.refresh_processes(ProcessesToUpdate::All, true);
    let current_pid = std::process::id();
    let mut stopped = 0usize;

    for process in system.processes().values() {
        if process.pid().as_u32() == current_pid {
            continue;
        }

        let name = process.name().to_string_lossy().to_ascii_lowercase();
        if name != "protoswitch.exe" && name != "protoswitch" {
            continue;
        }

        let commandline = process
            .cmd()
            .iter()
            .map(|value| value.to_string_lossy().to_ascii_lowercase())
            .collect::<Vec<_>>();

        let is_headless_watcher = commandline.iter().any(|value| value == "watch")
            && commandline.iter().any(|value| value == "--headless");

        if is_headless_watcher && process.kill() {
            stopped += 1;
        }
    }

    let mut state = AppState::load(paths)?;
    state.watcher.mode = WatcherMode::Idle;
    state.watcher.last_check_at = None;
    state.watcher.next_check_at = None;
    state.watcher.telegram_running = telegram::is_running().unwrap_or(false);
    state.save(&paths.state_file)?;

    Ok(stopped)
}

pub(crate) fn restart_background_watcher(paths: &AppPaths) -> anyhow::Result<String> {
    let stopped = stop_background_watcher(paths)?;
    spawn_background_watcher(paths)?;
    Ok(if stopped == 0 {
        "Watcher запущен в новом сеансе.".to_string()
    } else {
        format!("Watcher перезапущен. Остановлено: {stopped}")
    })
}

fn spawn_background_watcher(paths: &AppPaths) -> anyhow::Result<()> {
    let executable =
        std::env::current_exe().context("Не удалось определить путь к protoswitch.exe")?;
    let mut command = Command::new(executable);
    command.args(["watch", "--headless"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }

    command
        .spawn()
        .context("Не удалось запустить watcher в фоне")?;
    paths.append_log("launch started watcher")?;
    Ok(())
}

pub(crate) fn watcher_is_recent(config: &AppConfig, state: &AppState) -> bool {
    let Some(last_check_at) = state.watcher.last_check_at else {
        return false;
    };

    let threshold = ChronoDuration::seconds(
        (config.watcher.check_interval_secs.saturating_mul(2) + 15)
            .try_into()
            .unwrap_or(75),
    );

    Utc::now() - last_check_at < threshold
}

pub(crate) fn watcher_process_exists() -> bool {
    let mut system = System::new_all();
    system.refresh_processes(ProcessesToUpdate::All, true);
    let current_pid = std::process::id();

    system.processes().values().any(|process| {
        if process.pid().as_u32() == current_pid {
            return false;
        }

        let name = process.name().to_string_lossy().to_ascii_lowercase();
        if name != "protoswitch.exe" && name != "protoswitch" {
            return false;
        }

        let commandline = process
            .cmd()
            .iter()
            .map(|value| value.to_string_lossy().to_ascii_lowercase())
            .collect::<Vec<_>>();

        commandline.iter().any(|value| value == "watch")
            && commandline.iter().any(|value| value == "--headless")
    })
}

pub(crate) fn open_in_shell(path: &Path) -> anyhow::Result<()> {
    Command::new("explorer")
        .arg(path)
        .spawn()
        .with_context(|| format!("Не удалось открыть {}", path.display()))?;
    Ok(())
}

pub(crate) fn open_in_notepad(path: &Path) -> anyhow::Result<()> {
    Command::new("notepad")
        .arg(path)
        .spawn()
        .with_context(|| format!("Не удалось открыть {}", path.display()))?;
    Ok(())
}

fn format_local_time(timestamp: &chrono::DateTime<Utc>) -> String {
    timestamp
        .with_timezone(&Local)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

fn watcher_mode(mode: &WatcherMode) -> &'static str {
    match mode {
        WatcherMode::Idle => "idle",
        WatcherMode::Watching => "watching",
        WatcherMode::WaitingForTelegram => "waiting_for_telegram",
        WatcherMode::Switching => "switching",
        WatcherMode::Error => "error",
    }
}

fn autostart_method_label(method: &AutostartMethod) -> &'static str {
    match method {
        AutostartMethod::ScheduledTask => "scheduled_task",
        AutostartMethod::StartupFolder => "startup_folder",
    }
}

pub(crate) fn provider_pool_summary(config: &AppConfig) -> String {
    let (mtproto, socks5) = config.provider.source_counts();
    if socks5 == 0 {
        format!("{mtproto} MTProto")
    } else {
        format!("{mtproto} MTProto + {socks5} SOCKS5")
    }
}

pub(crate) fn enabled_sources_summary(config: &AppConfig) -> String {
    let names = config
        .provider
        .active_sources()
        .into_iter()
        .map(|source| source.name)
        .collect::<Vec<_>>();
    if names.is_empty() {
        "нет активных источников".to_string()
    } else {
        names.join(", ")
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "да" } else { "нет" }
}

fn persist_watch_error(paths: &AppPaths, error: &str) -> anyhow::Result<()> {
    let mut state = AppState::load(paths)?;
    state.last_error = Some(error.to_string());
    state.watcher.mode = WatcherMode::Error;
    state.save(&paths.state_file)?;
    Ok(())
}
