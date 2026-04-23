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
use crate::platform;
use crate::provider::MtProtoProvider;
use crate::telegram;
use crate::ui;
use crate::{APP_NAME, APP_VERSION};

#[cfg(test)]
mod e2e;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DoctorSnapshot {
    pub app_version: String,
    pub platform: String,
    pub config_exists: bool,
    pub state_exists: bool,
    pub log_exists: bool,
    pub tg_protocol_handler: Option<String>,
    pub telegram_executable: Option<String>,
    pub telegram_data_dir: Option<String>,
    pub telegram_running: bool,
    pub current_proxy_status: String,
    pub source_status: String,
    pub backend_mode: String,
    pub backend_status: String,
    pub backend_route: String,
    pub backend_restart_required: bool,
    pub managed_proxy_mode: Option<String>,
    pub managed_selected_proxy: Option<String>,
    pub managed_proxy_count: Option<usize>,
    pub autostart: platform::AutostartStatus,
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
        Some(Commands::Repair) => handle_repair(&paths),
        Some(Commands::Shutdown) => handle_shutdown(&paths),
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
        "Telegram data dir: {}",
        telegram_info
            .data_dir
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "не найден".to_string())
    );
    println!(
        "Telegram backend: {}",
        telegram_backend_mode_label(&config.telegram.backend_mode)
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
            serde_json::to_string_pretty(&status_snapshot_json_value(&config, &state, &autostart))
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

pub(crate) fn status_snapshot_json_value(
    config: &AppConfig,
    state: &AppState,
    autostart: &platform::AutostartStatus,
) -> serde_json::Value {
    serde_json::json!({
        "config": config,
        "state": state,
        "autostart": autostart,
    })
}

fn handle_switch(paths: &AppPaths, args: SwitchArgs) -> anyhow::Result<()> {
    println!("{}", switch_to_candidate(paths, args.dry_run)?);
    Ok(())
}

fn handle_cleanup(paths: &AppPaths) -> anyhow::Result<()> {
    println!("{}", cleanup_dead_proxies(paths)?);
    Ok(())
}

fn handle_repair(paths: &AppPaths) -> anyhow::Result<()> {
    println!("{}", repair_installation(paths)?);
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
    println!("Платформа: {}", report.platform);
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
    println!(
        "Telegram data dir: {}",
        report.telegram_data_dir.as_deref().unwrap_or("не найден")
    );
    println!("Telegram запущен: {}", yes_no(report.telegram_running));
    println!("Статус proxy: {}", report.current_proxy_status);
    println!("Статус источника: {}", report.source_status);
    println!("Telegram backend: {}", report.backend_mode);
    println!("Статус backend: {}", report.backend_status);
    println!("Путь применения: {}", report.backend_route);
    println!(
        "Нужен перезапуск Telegram: {}",
        yes_no(report.backend_restart_required)
    );
    println!(
        "Режим managed proxy: {}",
        report.managed_proxy_mode.as_deref().unwrap_or("нет данных")
    );
    println!(
        "Выбранный managed proxy: {}",
        report
            .managed_selected_proxy
            .as_deref()
            .unwrap_or("нет данных")
    );
    println!(
        "Managed proxy в списке: {}",
        report
            .managed_proxy_count
            .map(|value| value.to_string())
            .unwrap_or_else(|| "нет данных".to_string())
    );
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

fn handle_shutdown(paths: &AppPaths) -> anyhow::Result<()> {
    let stopped = stop_all_protoswitch_processes(paths)?;
    println!(
        "{}",
        if stopped == 0 {
            "Других процессов ProtoSwitch не найдено.".to_string()
        } else {
            format!("Остановлено процессов ProtoSwitch: {stopped}")
        }
    );
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
    let previous_telegram_running = state.watcher.telegram_running;
    let telegram_running = telegram::is_running().unwrap_or(false);
    state.watcher.telegram_running = telegram_running;
    state.watcher.last_check_at = Some(now);
    state.watcher.next_check_at = Some(
        now + ChronoDuration::seconds(config.watcher.check_interval_secs.try_into().unwrap_or(30)),
    );
    if state.current_proxy_status.trim().is_empty() {
        state.set_current_proxy_status(if state.current_proxy.is_some() {
            "ожидает проверки"
        } else {
            "не выбран"
        });
    }
    if state.source_status.trim().is_empty() {
        state.set_source_status("источник ещё не опрашивался");
    }
    if state.backend_restart_required {
        if !previous_telegram_running && telegram_running {
            state.backend_restart_required = false;
            state.set_backend_status("managed override принят после перезапуска");
        } else {
            state.watcher.mode = WatcherMode::WaitingForTelegram;
            state.watcher.failure_streak = 0;
            state.set_current_proxy_status(if telegram_running {
                "proxy сохранён, ждёт перезапуска Telegram"
            } else {
                "proxy сохранён, ждёт запуска Telegram"
            });
            if state.source_status.trim().is_empty() {
                state.set_source_status("replacement proxy уже сохранён в settingss");
            }
            state.save(&paths.state_file)?;
            return Ok(if headless {
                String::new()
            } else if telegram_running {
                "Proxy сохранён в Telegram settings. Нужен перезапуск Telegram.".to_string()
            } else {
                "Proxy сохранён в Telegram settings и применится при следующем запуске Telegram."
                    .to_string()
            });
        }
    }
    if state.current_proxy.is_none() {
        state.watcher.mode = WatcherMode::Idle;
        state.watcher.failure_streak = 0;
        state.set_current_proxy_status("ещё нет proxy под управлением");
        state.set_source_status(if state.pending_proxy.is_some() {
            "replacement proxy уже сохранён, ждём ручной apply"
        } else {
            "авторотация ждёт первого ручного switch"
        });
        state.save(&paths.state_file)?;
        return Ok(if headless {
            String::new()
        } else if state.pending_proxy.is_some() {
            "Watcher не трогает внешний proxy. Есть pending proxy для ручного применения."
                .to_string()
        } else {
            "Watcher ждёт первого ручного switch и не трогает внешний proxy.".to_string()
        });
    }
    let managed_proxy_healthy = state
        .current_proxy
        .as_ref()
        .map(|record| telegram::check_proxy(&record.proxy, config.watcher.connect_timeout_secs))
        .unwrap_or(false);
    if managed_proxy_healthy {
        if state.pending_proxy.is_some() {
            state.pending_proxy = None;
            let _ =
                paths.append_log("cleared stale pending proxy because current proxy is healthy");
        }
        state.watcher.mode = WatcherMode::Watching;
        state.mark_healthy();
        state.set_source_status("источник не запрашивался: текущий proxy активен");
        state.save(&paths.state_file)?;
        return Ok("Proxy остаётся рабочим.".to_string());
    }
    if telegram_running && let Some(record) = state.pending_proxy.clone() {
        match apply_proxy_record(
            paths,
            config,
            &mut state,
            record,
            "Использован сохранённый pending proxy".to_string(),
            "Отложенный proxy применён",
            false,
        ) {
            Ok(message) => return Ok(message),
            Err(error) => {
                let message = error.to_string();
                let _ = paths.append_log(format!("pending proxy apply rejected: {message}"));
            }
        }
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
        state.set_source_status("replacement proxy уже сохранён, ждём Telegram");
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
            false,
        );
    }
    let record =
        match fetch_validated_candidate(paths, config, provider, &state.recent_proxy_values()) {
            Ok(record) => record,
            Err(error) => {
                state.watcher.mode = WatcherMode::Error;
                state.set_source_status(normalize_source_status(&error.to_string()));
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
    state.pending_proxy = Some(record.clone());
    state.watcher.mode = WatcherMode::WaitingForTelegram;
    state.set_current_proxy_status(if state.current_proxy.is_some() {
        "текущий proxy не работает"
    } else {
        "текущий proxy не выбран"
    });
    state.set_source_status(format!(
        "replacement proxy сохранён, ждём Telegram: {}",
        record.proxy.short_label()
    ));
    state.save(&paths.state_file)?;
    paths.append_log(format!(
        "watcher сохранил отложенный proxy при выключенном Telegram: {}",
        record.proxy.short_label()
    ))?;
    if headless {
        Ok(String::new())
    } else {
        Ok(format!(
            "Telegram не запущен. Отложенный proxy сохранён: {}",
            record.proxy.short_label()
        ))
    }
}

#[allow(dead_code)]
fn print_plain_status(
    paths: &AppPaths,
    config: &AppConfig,
    state: &AppState,
    autostart: &platform::AutostartStatus,
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
        "Отложенный proxy: {}",
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
    autostart: &platform::AutostartStatus,
) {
    let managed = telegram::managed_settings_status(&config.telegram).ok();
    println!("ProtoSwitch {}", APP_VERSION);
    println!("Платформа: {}", platform::current_os_label());
    println!("Итог: {}", overall_summary_text(state));
    println!("Фон: {}", background_summary_text(state));
    println!("Следующий шаг: {}", next_step_text(state));
    println!("Статус proxy: {}", current_proxy_status_text(state));
    println!("Статус источника: {}", source_status_text(state));
    println!(
        "Статус backend: {}",
        backend_status_text(state, managed.as_ref())
    );
    println!(
        "Путь применения: {}",
        backend_route_text(state, managed.as_ref())
    );
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
        "Отложенный proxy: {}",
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
        "Режим backend Telegram: {}",
        telegram_backend_mode_label(&config.telegram.backend_mode)
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

    if state.backend_restart_required {
        return "сохранён, ждёт применения".to_string();
    }

    if state.pending_proxy.is_some() {
        return "есть резерв".to_string();
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

pub(crate) fn overall_summary_text(state: &AppState) -> String {
    let proxy_status = current_proxy_status_text(state);
    let source_status = source_status_text(state);

    if state.backend_restart_required {
        return if state.watcher.telegram_running {
            "Новый proxy записан в Telegram. ProtoSwitch продолжит проверку состояния.".to_string()
        } else {
            "Proxy записан в Telegram settings и готов к следующей проверке.".to_string()
        };
    }

    if matches!(state.watcher.mode, WatcherMode::Error) {
        return "Watcher упёрся в ошибку и ждёт следующего цикла или ручной проверки.".to_string();
    }

    if status_contains_any(
        &proxy_status,
        &["актив", "работ", "доступ", "подключ", "online", "ok"],
    ) {
        return "Текущий proxy выглядит рабочим. ProtoSwitch просто следит за состоянием."
            .to_string();
    }

    if state.pending_proxy.is_some() {
        return if state.watcher.telegram_running {
            "Есть подготовленный replacement proxy. Его можно применить вручную без нового fetch."
                .to_string()
        } else {
            "Найден replacement proxy. ProtoSwitch дождётся Telegram и продолжит работу."
                .to_string()
        };
    }

    if status_contains_any(&source_status, &["источник пуст", "нет свободных"])
    {
        return "Сейчас свежего proxy нет. ProtoSwitch продолжит поиск в фоне.".to_string();
    }

    if state.current_proxy.is_none() && matches!(state.watcher.mode, WatcherMode::Idle) {
        return "Watcher остановлен. ProtoSwitch сейчас только показывает состояние.".to_string();
    }

    if state.current_proxy.is_none() {
        return "Рабочий proxy ещё не закреплён. ProtoSwitch подбирает подходящий вариант."
            .to_string();
    }

    "ProtoSwitch держит состояние под контролем и ждёт следующей проверки.".to_string()
}

pub(crate) fn background_summary_text(state: &AppState) -> String {
    if state.backend_restart_required {
        return if state.watcher.telegram_running {
            "Фоновый режим активен. Telegram не открывается поверх других окон.".to_string()
        } else {
            "Replacement сохранён и будет проверен после запуска Telegram.".to_string()
        };
    }

    if matches!(state.watcher.mode, WatcherMode::Idle) {
        return "Watcher остановлен, ProtoSwitch не вмешивается в Telegram.".to_string();
    }

    if state.watcher.telegram_running {
        "Watcher работает в фоне: без popup и без захвата фокуса.".to_string()
    } else {
        "ProtoSwitch спокойно ждёт Telegram и может готовить replacement заранее.".to_string()
    }
}

pub(crate) fn next_step_text(state: &AppState) -> String {
    if state.backend_restart_required && state.watcher.telegram_running {
        return "Оставьте ProtoSwitch включённым: он продолжит контроль proxy.".to_string();
    }

    if state.current_proxy.is_none() && state.pending_proxy.is_none() {
        return if matches!(state.watcher.mode, WatcherMode::Idle) {
            "Запустите watcher, если хотите автоматическую смену proxy.".to_string()
        } else {
            "Можно дождаться следующего цикла watcher или запросить proxy вручную.".to_string()
        };
    }

    if state.pending_proxy.is_some() && state.watcher.telegram_running {
        return "При желании можно применить pending proxy вручную прямо сейчас.".to_string();
    }

    "Никаких действий не требуется.".to_string()
}

fn status_contains_any(value: &str, needles: &[&str]) -> bool {
    let lower = value.to_lowercase();
    needles.iter().any(|needle| lower.contains(needle))
}

fn normalize_source_status(raw: &str) -> String {
    let lower = raw.to_lowercase();
    if lower.contains("свободных серверов нет") {
        return "источник пуст: нет свободных proxy".to_string();
    }
    if lower.contains("tcp-проверка не прошла") {
        return "кандидат отклонён локальной проверкой".to_string();
    }
    if lower.contains("только недавние proxy") {
        return "источник не дал новый proxy".to_string();
    }
    if lower.contains("не вернул proxy")
        || lower.contains("не вернули кандидатов")
        || lower.contains("пустой ответ")
        || lower.contains("в списке нет")
        || lower.contains("не удалось выбрать кандидата")
        || lower.contains("не успел выдать proxy")
    {
        return "источник пуст: кандидаты не найдены".to_string();
    }
    if lower.contains("не удалось открыть") || lower.contains("вернул ошибку")
    {
        return format!("источник недоступен: {raw}");
    }
    raw.to_string()
}

pub(crate) fn backend_status_text(
    state: &AppState,
    managed: Option<&telegram::ManagedSettingsStatus>,
) -> String {
    if state.backend_restart_required {
        let waiting = if state.watcher.telegram_running {
            "ждёт перезапуска Telegram"
        } else {
            "ждёт следующего запуска Telegram"
        };
        if !state.backend_status.trim().is_empty() {
            return format!("{} / {}", state.backend_status, waiting);
        }
        return waiting.to_string();
    }

    if !state.backend_status.trim().is_empty() {
        return state.backend_status.clone();
    }

    managed
        .map(|status| {
            let rotation = if status.rotation_enabled {
                " / rotation"
            } else {
                ""
            };
            format!(
                "managed backend / {}{} / {}",
                status.mode_label, rotation, status.selected_label
            )
        })
        .unwrap_or_else(|| "нет данных".to_string())
}

pub(crate) fn backend_route_text(
    state: &AppState,
    managed: Option<&telegram::ManagedSettingsStatus>,
) -> String {
    if state.backend_restart_required && !state.backend_route.trim().is_empty() {
        return format!("ожидает перезапуска / {}", state.backend_route);
    }

    if !state.backend_route.trim().is_empty() {
        return state.backend_route.clone();
    }

    managed
        .map(|status| status.data_dir.join("settingss").display().to_string())
        .unwrap_or_else(|| "нет данных".to_string())
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
    let dead = dead_managed_proxies(state, config.watcher.connect_timeout_secs);
    if dead.is_empty() {
        return Ok(0);
    }

    let removed = telegram::cleanup_managed_proxies(&config.telegram, &dead)?;
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
        state.backend_restart_required = false;
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
    state.set_backend_status(format!("автоподчистка удалила {removed} proxy"));
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
) -> anyhow::Result<(AppConfig, AppState, platform::AutostartStatus)> {
    let config = AppConfig::load(paths)?;
    let mut state = AppState::load(paths)?;
    state.watcher.telegram_running = telegram::is_running().unwrap_or(false);
    if let Ok(managed) = telegram::managed_settings_status(&config.telegram) {
        if state.backend_status.trim().is_empty() {
            state.set_backend_status(format!(
                "managed backend / {} / {}",
                managed.mode_label, managed.selected_label
            ));
        }
        if state.backend_route.trim().is_empty() {
            state.set_backend_route(managed.data_dir.join("settingss").display().to_string());
        }
    }
    let autostart = platform::query_autostart();
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
    let max_attempts = config.watcher.history_size.clamp(3, 8);
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

struct ApplyFailureState {
    current_status: String,
    source_status: String,
    error_message: String,
    log_message: String,
}

fn store_apply_failure(
    paths: &AppPaths,
    config: &AppConfig,
    state: &mut AppState,
    record: &ProxyRecord,
    failure: ApplyFailureState,
) -> anyhow::Result<()> {
    let _ =
        telegram::cleanup_managed_proxies(&config.telegram, std::slice::from_ref(&record.proxy));
    state.pending_proxy = None;
    state.backend_restart_required = false;
    state.watcher.mode = WatcherMode::Error;
    state.watcher.failure_streak = config.watcher.failure_threshold;
    state.last_error = Some(failure.error_message);
    state.set_current_proxy_status(failure.current_status);
    state.set_source_status(failure.source_status);
    state.set_backend_status("managed backend завершился ошибкой");
    state.push_recent(record.clone(), config.watcher.history_size);
    state.save(&paths.state_file)?;
    paths.append_log(failure.log_message)?;
    Ok(())
}

fn apply_proxy_record(
    paths: &AppPaths,
    config: &AppConfig,
    state: &mut AppState,
    record: ProxyRecord,
    source_status: String,
    success_prefix: &str,
    allow_fallback: bool,
) -> anyhow::Result<String> {
    state.push_recent(record.clone(), config.watcher.history_size);
    let managed_owned = state.recent_proxy_values();

    let use_managed_backend = !allow_fallback
        || !matches!(
            config.telegram.backend_mode,
            crate::model::TelegramBackendMode::Manual
        );

    if use_managed_backend {
        let managed = match telegram::apply_managed_proxy(
            &config.telegram,
            &record.proxy,
            &managed_owned,
            config.watcher.auto_cleanup_dead_proxies,
            allow_fallback,
            config.watcher.connect_timeout_secs,
        ) {
            Ok(result) => result,
            Err(error) => {
                let details = error.to_string();
                store_apply_failure(
                    paths,
                    config,
                    state,
                    &record,
                    ApplyFailureState {
                        current_status: format!(
                            "Managed backend не смог записать proxy: {details}"
                        ),
                        source_status: format!(
                            "Источник дал кандидата {}, но managed backend завершился ошибкой: {details}",
                            record.proxy.short_label()
                        ),
                        error_message: format!(
                            "Managed backend не смог записать proxy: {}",
                            record.proxy.short_label()
                        ),
                        log_message: format!(
                            "managed backend отклонил proxy {}: {details}",
                            record.proxy.short_label()
                        ),
                    },
                )?;
                return Err(anyhow::anyhow!(
                    "Managed backend не смог записать proxy: {} ({details})",
                    record.proxy.short_label()
                ));
            }
        };

        if managed.used_fallback {
            state.backend_restart_required = false;
            state.set_backend_status("ручной fallback через Telegram UI");
            state.set_backend_route(format!("ручной fallback -> {}", record.proxy.deep_link()));
        } else if let Some(details) = managed.fallback_error.as_ref() {
            state.backend_restart_required = state.watcher.telegram_running;
            state.set_backend_status("ручной fallback недоступен");
            state.set_backend_route(format!("settingss -> {}", managed.settings_path.display()));
            let _ = paths.append_log(format!(
                "manual fallback unavailable for {}: {details}",
                record.proxy.short_label()
            ));
        } else {
            state.set_backend_status(format!(
                "managed backend / {} / {}",
                managed.settings_status.mode_label, managed.settings_status.selected_label
            ));
            state.set_backend_route(format!("settingss -> {}", managed.settings_path.display()));
        }

        if !managed.immediate {
            state.current_proxy = Some(record.clone());
            state.pending_proxy = None;
            state.last_fetch_at.get_or_insert(record.captured_at);
            state.last_apply_at = Some(Utc::now());
            state.backend_restart_required = false;
            state.watcher.mode = WatcherMode::Watching;
            state.watcher.failure_streak = 0;
            state.last_error = None;
            state.set_current_proxy_status(if managed.fallback_error.is_some() {
                "proxy сохранён в settingss, ручной fallback недоступен"
            } else if state.watcher.telegram_running {
                "proxy записан в Telegram settings"
            } else {
                "proxy сохранён в Telegram settings"
            });
            state.set_source_status(source_status);
            state.save(&paths.state_file)?;
            paths.append_log(format!(
                "proxy сохранён в settingss: {} -> {}",
                record.proxy.short_label(),
                managed.settings_path.display()
            ))?;
            return Ok(if state.watcher.telegram_running {
                format!(
                    "{success_prefix}: {} ({})",
                    record.proxy.short_label(),
                    if managed.fallback_error.is_some() {
                        "сохранён в settingss, ручной fallback недоступен"
                    } else {
                        "записан в Telegram settings"
                    }
                )
            } else {
                format!(
                    "{success_prefix}: {} (записан в Telegram settings)",
                    record.proxy.short_label()
                )
            });
        }
    } else if let Err(error) =
        telegram::open_proxy_link(&record.proxy, config.watcher.connect_timeout_secs)
    {
        let details = error.to_string();
        store_apply_failure(
            paths,
            config,
            state,
            &record,
            ApplyFailureState {
                current_status: format!("Telegram не подтвердил добавление proxy: {details}"),
                source_status: format!(
                    "Источник дал кандидата {}, но диалог Telegram завершился ошибкой: {details}",
                    record.proxy.short_label()
                ),
                error_message: format!(
                    "Telegram не подтвердил добавление proxy: {}",
                    record.proxy.short_label()
                ),
                log_message: format!(
                    "telegram dialog rejected proxy {} with error {details}",
                    record.proxy.short_label()
                ),
            },
        )?;
        return Err(anyhow::anyhow!(
            "Telegram не подтвердил добавление proxy: {}",
            record.proxy.short_label()
        ));
    } else {
        state.backend_restart_required = false;
        state.set_backend_status("ручной tg:// apply");
        state.set_backend_route(format!("ручной fallback -> {}", record.proxy.deep_link()));
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
                ApplyFailureState {
                    current_status: format!("Telegram отклонил proxy: {details}"),
                    source_status: format!(
                        "Источник дал кандидата {}, но Telegram показал статус: {details}",
                        record.proxy.short_label()
                    ),
                    error_message: format!(
                        "Telegram пометил proxy как недоступный: {}",
                        record.proxy.short_label()
                    ),
                    log_message: format!(
                        "telegram rejected proxy {} with status {details}",
                        record.proxy.short_label()
                    ),
                },
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
            if state.backend_status.trim().is_empty() {
                state.set_backend_status("live apply через Telegram UI");
            }
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
                ApplyFailureState {
                    current_status: format!("Telegram не подтвердил подключение proxy: {details}"),
                    source_status: format!(
                        "Источник дал кандидата {}, но Telegram не подтвердил подключение: {details}",
                        record.proxy.short_label()
                    ),
                    error_message: format!(
                        "Telegram не подтвердил подключение proxy: {}",
                        record.proxy.short_label()
                    ),
                    log_message: format!(
                        "telegram did not settle proxy {} and returned status {details}",
                        record.proxy.short_label()
                    ),
                },
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
                ApplyFailureState {
                    current_status: "Telegram не сохранил proxy в списке".to_string(),
                    source_status: format!(
                        "Источник дал кандидата {}, но Telegram не сохранил proxy",
                        record.proxy.short_label()
                    ),
                    error_message: format!(
                        "Telegram не сохранил proxy: {}",
                        record.proxy.short_label()
                    ),
                    log_message: format!(
                        "telegram did not persist proxy {} after apply",
                        record.proxy.short_label()
                    ),
                },
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
    allow_fallback: bool,
) -> anyhow::Result<String> {
    let mut rejected = state.recent_proxy_values();
    let attempts = config.watcher.history_size.clamp(3, 6);
    let mut last_error = None;

    for _ in 0..attempts {
        let record = match fetch_validated_candidate(paths, config, provider, &rejected) {
            Ok(record) => record,
            Err(error) => {
                return Err(last_error.unwrap_or(error));
            }
        };
        rejected.push(record.proxy.clone());
        state.last_fetch_at = Some(record.captured_at);
        match apply_proxy_record(
            paths,
            config,
            state,
            record.clone(),
            format!("Найден рабочий proxy: {}", record.proxy.short_label()),
            success_prefix,
            allow_fallback,
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
    apply_candidate_with_retries(
        paths,
        &config,
        &provider,
        &mut state,
        "Применён proxy",
        true,
    )
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
        "Отложенный proxy применён",
        true,
    )
}

pub(crate) fn doctor_snapshot(paths: &AppPaths) -> anyhow::Result<DoctorSnapshot> {
    doctor_snapshot_v2(paths)
}

pub(crate) fn doctor_snapshot_v2(paths: &AppPaths) -> anyhow::Result<DoctorSnapshot> {
    let config = AppConfig::load(paths)?;
    let state = AppState::load(paths)?;
    let provider = MtProtoProvider::new(config.provider.clone())?;
    let installation = telegram::detect_installation()?;
    let managed = telegram::managed_settings_status(&config.telegram).ok();
    let autostart = platform::query_autostart();
    let provider_probe =
        fetch_validated_candidate(paths, &config, &provider, &[] as &[MtProtoProxy])
            .map(|record| format!("{} via {}", record.proxy.short_label(), record.source))
            .map_err(|error| error.to_string());

    Ok(DoctorSnapshot {
        app_version: APP_VERSION.to_string(),
        platform: platform::current_os_label().to_string(),
        config_exists: paths.config_file.exists(),
        state_exists: paths.state_file.exists(),
        log_exists: paths.log_file.exists(),
        tg_protocol_handler: installation.protocol_handler,
        telegram_executable: installation
            .executable_path
            .map(|path| path.display().to_string()),
        telegram_data_dir: installation.data_dir.map(|path| path.display().to_string()),
        telegram_running: telegram::is_running().unwrap_or(false),
        current_proxy_status: current_proxy_status_text(&state),
        source_status: source_status_text(&state),
        backend_mode: telegram_backend_mode_label(&config.telegram.backend_mode).to_string(),
        backend_status: backend_status_text(&state, managed.as_ref()),
        backend_route: backend_route_text(&state, managed.as_ref()),
        backend_restart_required: state.backend_restart_required,
        managed_proxy_mode: managed.as_ref().map(|status| {
            if status.rotation_enabled {
                format!("{} / rotation", status.mode_label)
            } else {
                status.mode_label.clone()
            }
        }),
        managed_selected_proxy: managed.as_ref().map(|status| status.selected_label.clone()),
        managed_proxy_count: managed.as_ref().map(|status| status.proxy_count),
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
    let watcher_was_online = if restart_watcher {
        match (AppConfig::load(paths), AppState::load(paths)) {
            (Ok(current_config), Ok(current_state)) => {
                watcher_is_recent(&current_config, &current_state) || watcher_process_exists()
            }
            _ => watcher_process_exists(),
        }
    } else {
        false
    };

    if config.autostart.enabled {
        let method = platform::install_autostart(
            &std::env::current_exe().context("Не удалось определить путь к protoswitch.exe")?,
        )?;
        config.autostart.method = method;
    } else {
        let _ = platform::remove_autostart();
        config.autostart.method = default_autostart_method();
    }

    config.save(&paths.config_file)?;
    if watcher_was_online {
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

#[derive(Clone, Copy)]
enum StopProcessScope {
    WatchersOnly,
    AllProtoSwitch,
}

fn stop_protoswitch_processes(paths: &AppPaths, scope: StopProcessScope) -> anyhow::Result<usize> {
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

        let should_stop = match scope {
            StopProcessScope::WatchersOnly => is_headless_watcher,
            StopProcessScope::AllProtoSwitch => true,
        };

        if should_stop && process.kill() {
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

pub(crate) fn stop_background_watcher(paths: &AppPaths) -> anyhow::Result<usize> {
    stop_protoswitch_processes(paths, StopProcessScope::WatchersOnly)
}

pub(crate) fn stop_all_protoswitch_processes(paths: &AppPaths) -> anyhow::Result<usize> {
    stop_protoswitch_processes(paths, StopProcessScope::AllProtoSwitch)
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

pub(crate) fn repair_installation(paths: &AppPaths) -> anyhow::Result<String> {
    paths.ensure_dirs()?;
    let stopped = stop_background_watcher(paths).unwrap_or(0);
    if !paths.state_file.exists() {
        AppState::default().save(&paths.state_file)?;
    }

    let config = AppConfig::load(paths)?;
    let saved = persist_config_with_restart(paths, config, false)?;
    let doctor = doctor_snapshot_v2(paths)?;
    let provider = match doctor.provider_probe {
        Ok(value) => format!("источник доступен ({value})"),
        Err(error) => format!("источник требует внимания ({error})"),
    };

    Ok(format!(
        "{saved}. Остановлено watcher-процессов: {stopped}. tg:// handler: {}. {}.",
        if doctor.tg_protocol_handler.is_some() {
            "найден"
        } else {
            "не найден"
        },
        provider
    ))
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
        AutostartMethod::XdgDesktop => "xdg_desktop",
        AutostartMethod::LaunchAgent => "launch_agent",
    }
}

fn default_autostart_method() -> AutostartMethod {
    #[cfg(windows)]
    {
        return AutostartMethod::ScheduledTask;
    }

    #[cfg(target_os = "linux")]
    {
        return AutostartMethod::XdgDesktop;
    }

    #[cfg(target_os = "macos")]
    {
        return AutostartMethod::LaunchAgent;
    }

    #[allow(unreachable_code)]
    AutostartMethod::ScheduledTask
}

fn telegram_backend_mode_label(mode: &crate::model::TelegramBackendMode) -> &'static str {
    match mode {
        crate::model::TelegramBackendMode::Managed => "managed",
        crate::model::TelegramBackendMode::Hybrid => "hybrid",
        crate::model::TelegramBackendMode::Manual => "manual",
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

#[cfg(test)]
mod tests {
    use super::{background_summary_text, next_step_text, overall_summary_text};
    use crate::model::{AppState, ProxyRecord, TelegramProxy, WatcherMode, WatcherSnapshot};

    #[test]
    fn summarizes_managed_background_flow() {
        let state = AppState {
            current_proxy: Some(ProxyRecord::new(
                TelegramProxy::mtproto(
                    "ovh.pl.1.mtproto.ru",
                    443,
                    "ee211122223333444455556666777788",
                ),
                "mtproto.ru",
            )),
            backend_restart_required: true,
            watcher: WatcherSnapshot {
                telegram_running: true,
                ..WatcherSnapshot::default()
            },
            ..AppState::default()
        };

        assert!(overall_summary_text(&state).contains("записан в Telegram"));
        assert!(background_summary_text(&state).contains("Фоновый режим"));
        assert_eq!(
            next_step_text(&state),
            "Оставьте ProtoSwitch включённым: он продолжит контроль proxy."
        );
    }

    #[test]
    fn summarizes_idle_empty_state() {
        let state = AppState {
            watcher: WatcherSnapshot {
                mode: WatcherMode::Idle,
                ..WatcherSnapshot::default()
            },
            ..AppState::default()
        };

        assert!(overall_summary_text(&state).contains("Watcher остановлен"));
        assert!(background_summary_text(&state).contains("не вмешивается"));
        assert!(next_step_text(&state).contains("Запустите watcher"));
    }
}
