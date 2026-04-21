use std::thread;
use std::time::Duration;

use anyhow::Context;
use chrono::{Duration as ChronoDuration, Local, Utc};
use clap::Parser;

use crate::cli::{
    AutostartCommand, Cli, Commands, DoctorArgs, InitArgs, StatusArgs, SwitchArgs, WatchArgs,
};
use crate::model::{AppConfig, AppState, InitOverrides, WatcherMode};
use crate::paths::AppPaths;
use crate::provider::MtProtoProvider;
use crate::telegram;
use crate::ui;
use crate::windows;
use crate::{APP_NAME, APP_VERSION};

pub fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let paths = AppPaths::resolve()?;
    paths.ensure_dirs()?;

    match cli.command {
        Commands::Init(args) => handle_init(&paths, args),
        Commands::Watch(args) => handle_watch(&paths, args),
        Commands::Status(args) => handle_status(&paths, args),
        Commands::Switch(args) => handle_switch(&paths, args),
        Commands::Doctor(args) => handle_doctor(&paths, args),
        Commands::Autostart { command } => handle_autostart(&paths, command),
    }
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

    config.save(&paths.config_file)?;

    if !paths.state_file.exists() {
        AppState::default().save(&paths.state_file)?;
    }

    if config.autostart.enabled {
        windows::install_autostart(
            &std::env::current_exe().context("Не удалось определить путь к protoswitch.exe")?,
        )?;
    }

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
    let config = AppConfig::load(paths)?;
    let state = AppState::load(paths)?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "config": config,
                "state": state,
            }))
            .context("Не удалось сериализовать status")?
        );
        return Ok(());
    }

    if !args.plain && ui::stdout_is_terminal() {
        return ui::run_status(paths, &config, &state);
    }

    print_plain_status(paths, &config, &state);
    Ok(())
}

fn handle_switch(paths: &AppPaths, args: SwitchArgs) -> anyhow::Result<()> {
    let config = AppConfig::load(paths)?;
    let provider = MtProtoProvider::new(config.provider.clone())?;
    let mut state = AppState::load(paths)?;
    let recent = state.recent_proxy_values();
    let record = provider.fetch_candidate(&recent)?;

    if args.dry_run {
        println!("Новый proxy: {}", record.proxy.deep_link());
        return Ok(());
    }

    telegram::open_proxy_link(&record.proxy)?;
    state.current_proxy = Some(record.clone());
    state.pending_proxy = None;
    state.last_fetch_at = Some(record.captured_at);
    state.last_apply_at = Some(Utc::now());
    state.watcher.mode = WatcherMode::Switching;
    state.watcher.telegram_running = telegram::is_running().unwrap_or(false);
    state.mark_healthy();
    state.push_recent(record.clone(), config.watcher.history_size);
    state.save(&paths.state_file)?;
    paths.append_log(format!(
        "manual switch applied {}",
        record.proxy.short_label()
    ))?;

    println!("Применён proxy: {}", record.proxy.short_label());
    Ok(())
}

fn handle_doctor(paths: &AppPaths, args: DoctorArgs) -> anyhow::Result<()> {
    let config = AppConfig::load(paths)?;
    let provider = MtProtoProvider::new(config.provider.clone())?;
    let installation = telegram::detect_installation()?;
    let provider_probe = provider
        .fetch_candidate(&[])
        .map(|record| record.proxy.short_label())
        .map_err(|error| error.to_string());
    let provider_probe_display = match &provider_probe {
        Ok(proxy) => format!("ok ({proxy})"),
        Err(error) => format!("error ({error})"),
    };
    let report = serde_json::json!({
        "app_version": APP_VERSION,
        "config_exists": paths.config_file.exists(),
        "state_exists": paths.state_file.exists(),
        "log_exists": paths.log_file.exists(),
        "tg_protocol_handler": installation.protocol_handler,
        "telegram_executable": installation.executable_path.map(|path| path.display().to_string()),
        "telegram_running": telegram::is_running().unwrap_or(false),
        "autostart_installed": windows::query_autostart(),
        "provider_probe": provider_probe,
    });

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
        if report["tg_protocol_handler"].is_null() {
            "не найден"
        } else {
            "найден"
        }
    );
    println!(
        "Telegram Desktop: {}",
        report["telegram_executable"]
            .as_str()
            .unwrap_or("не найден")
    );
    println!(
        "Telegram запущен: {}",
        yes_no(report["telegram_running"].as_bool().unwrap_or(false))
    );
    println!(
        "Автозапуск: {}",
        yes_no(report["autostart_installed"].as_bool().unwrap_or(false))
    );
    println!("mtproto.ru: {provider_probe_display}");

    Ok(())
}

fn handle_autostart(paths: &AppPaths, command: AutostartCommand) -> anyhow::Result<()> {
    let mut config = AppConfig::load(paths)?;

    match command {
        AutostartCommand::Install => {
            windows::install_autostart(
                &std::env::current_exe().context("Не удалось определить путь к protoswitch.exe")?,
            )?;
            config.autostart.enabled = true;
            config.save(&paths.config_file)?;
            println!("Автозапуск включён.");
        }
        AutostartCommand::Remove => {
            windows::remove_autostart()?;
            config.autostart.enabled = false;
            config.save(&paths.config_file)?;
            println!("Автозапуск выключен.");
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

    if telegram_running {
        if let Some(record) = state.pending_proxy.clone() {
            telegram::open_proxy_link(&record.proxy)?;
            state.current_proxy = Some(record.clone());
            state.pending_proxy = None;
            state.last_apply_at = Some(Utc::now());
            state.watcher.mode = WatcherMode::Switching;
            state.mark_healthy();
            state.save(&paths.state_file)?;
            paths.append_log(format!(
                "pending proxy applied {}",
                record.proxy.short_label()
            ))?;
            return Ok(format!(
                "Pending proxy применён: {}",
                record.proxy.short_label()
            ));
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
        state.save(&paths.state_file)?;
        return Ok("Proxy остаётся рабочим.".to_string());
    }

    if state.current_proxy.is_some() {
        state.mark_failure();
    } else {
        state.watcher.failure_streak = config.watcher.failure_threshold;
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
        state.save(&paths.state_file)?;
        return Ok("Есть pending proxy, ждём запуска Telegram.".to_string());
    }

    let record = provider.fetch_candidate(&state.recent_proxy_values())?;
    state.last_fetch_at = Some(record.captured_at);
    state.push_recent(record.clone(), config.watcher.history_size);

    if telegram_running {
        telegram::open_proxy_link(&record.proxy)?;
        state.current_proxy = Some(record.clone());
        state.pending_proxy = None;
        state.last_apply_at = Some(Utc::now());
        state.watcher.mode = WatcherMode::Switching;
        state.mark_healthy();
        state.save(&paths.state_file)?;
        paths.append_log(format!("watch applied {}", record.proxy.short_label()))?;
        return Ok(format!(
            "Watcher переключил proxy: {}",
            record.proxy.short_label()
        ));
    }

    state.pending_proxy = Some(record.clone());
    state.watcher.mode = WatcherMode::WaitingForTelegram;
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

fn print_plain_status(paths: &AppPaths, config: &AppConfig, state: &AppState) {
    println!("ProtoSwitch {}", APP_VERSION);
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
    println!("config.toml: {}", paths.config_file.display());
    println!("state.json: {}", paths.state_file.display());
    println!("watch.log: {}", paths.log_file.display());
    if let Some(error) = &state.last_error {
        println!("Последняя ошибка: {error}");
    }
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
