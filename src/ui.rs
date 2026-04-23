use std::io::{self, IsTerminal};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Gauge, List, ListItem, Paragraph, Wrap};

use crate::APP_VERSION;
use crate::app;
use crate::model::{AppConfig, AppState, AutostartMethod, ProxyKind, WatcherMode};
use crate::paths::AppPaths;
use crate::platform;

pub fn stdout_is_terminal() -> bool {
    io::stdout().is_terminal()
}

pub fn run_setup(config: AppConfig) -> anyhow::Result<AppConfig> {
    let mut session = TerminalSession::new()?;
    let mut draft = SetupDraft::from(config);

    loop {
        session.terminal.draw(|frame| render_setup(frame, &draft))?;

        if let Event::Key(key) = event::read().context("Не удалось прочитать клавиатуру")?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            match key.code {
                KeyCode::Up => draft.focus = draft.focus.saturating_sub(1),
                KeyCode::Down => draft.focus = (draft.focus + 1).min(6),
                KeyCode::Left => draft.adjust(false),
                KeyCode::Right => draft.adjust(true),
                KeyCode::Enter => return Ok(draft.into_config()),
                KeyCode::Esc | KeyCode::Char('q') => return Ok(draft.original),
                _ => {}
            }
        }
    }
}

pub fn run_status(paths: &AppPaths) -> anyhow::Result<()> {
    let mut session = TerminalSession::new()?;
    let mut console = ConsoleState::default();
    let mut snapshot = UiSnapshot::load(paths)?;
    let mut last_refresh = Instant::now();

    loop {
        console.poll_background_tasks();
        if console.force_refresh || last_refresh.elapsed() >= Duration::from_millis(550) {
            snapshot = UiSnapshot::load(paths)?;
            console.sync_error(&snapshot.state.last_error);
            console.force_refresh = false;
            last_refresh = Instant::now();
        }

        let actions = console_actions(&snapshot);
        if console.section == ConsoleSection::Actions {
            console.focus = console.focus.min(actions.len().saturating_sub(1));
        } else {
            console.focus = 0;
        }

        session.terminal.draw(|frame| {
            render_console(frame, paths, &snapshot, &console, &actions);
        })?;

        if !event::poll(Duration::from_millis(80))? {
            continue;
        }

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match key.code {
            KeyCode::F(5) => {
                session.terminal.clear()?;
                console.force_refresh = true;
                console.clear_inspector();
                console.push_activity("Экран перечитан и перерисован.".to_string());
                continue;
            }
            KeyCode::PageUp => {
                console.scroll_activity_up();
                continue;
            }
            KeyCode::PageDown => {
                console.scroll_activity_down();
                continue;
            }
            KeyCode::Left => {
                console.section = console.section.prev();
                continue;
            }
            KeyCode::Right | KeyCode::Tab => {
                console.section = console.section.next();
                continue;
            }
            KeyCode::Char('1') => {
                console.section = ConsoleSection::Dashboard;
                continue;
            }
            KeyCode::Char('2') => {
                console.section = ConsoleSection::Actions;
                continue;
            }
            KeyCode::Char('3') => {
                console.section = ConsoleSection::Providers;
                continue;
            }
            KeyCode::Char('4') => {
                console.section = ConsoleSection::History;
                continue;
            }
            KeyCode::Char('r') => {
                session.terminal.clear()?;
                console.force_refresh = true;
                console.clear_inspector();
                console.set_result(
                    "Refresh",
                    vec!["Снимок перечитан и экран обновлён.".to_string()],
                );
                continue;
            }
            _ => {}
        }

        if console.section != ConsoleSection::Actions {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => return Ok(()),
                KeyCode::Char('c') => {
                    console.section = ConsoleSection::Actions;
                }
                _ => {}
            }
            continue;
        }

        let selected = actions[console.focus];
        let direct = match key.code {
            KeyCode::Up => {
                console.focus = console.focus.saturating_sub(1);
                None
            }
            KeyCode::Down => {
                console.focus = (console.focus + 1).min(actions.len().saturating_sub(1));
                None
            }
            KeyCode::Esc | KeyCode::Char('q') => Some(ConsoleAction::Exit),
            KeyCode::Enter => Some(selected),
            KeyCode::Char('s') => find_action(&actions, ConsoleAction::SwitchNow),
            KeyCode::Char('p') => find_action(&actions, ConsoleAction::ApplyPending),
            KeyCode::Char('w') => find_action(&actions, ConsoleAction::WatchControl),
            KeyCode::Char('x') => find_action(&actions, ConsoleAction::StopWatcher),
            KeyCode::Char('z') => find_action(&actions, ConsoleAction::StopAll),
            KeyCode::Char('a') => find_action(&actions, ConsoleAction::ToggleAutostart),
            KeyCode::Char('k') => find_action(&actions, ConsoleAction::ToggleAutoCleanup),
            KeyCode::Char('f') => find_action(&actions, ConsoleAction::ToggleSocks5Fallback),
            KeyCode::Char('e') => find_action(&actions, ConsoleAction::Settings),
            KeyCode::Char('d') => find_action(&actions, ConsoleAction::Doctor),
            KeyCode::Char('l') => find_action(&actions, ConsoleAction::OpenLog),
            KeyCode::Char('o') => find_action(&actions, ConsoleAction::OpenDataDir),
            KeyCode::Char('r') => Some(ConsoleAction::Refresh),
            _ => None,
        };

        let Some(action) = direct else {
            continue;
        };

        match action {
            ConsoleAction::Exit => return Ok(()),
            ConsoleAction::SwitchNow => match app::switch_to_candidate(paths, false) {
                Ok(message) => {
                    console.force_refresh = true;
                    console.set_result("Switch", vec![message]);
                }
                Err(error) => console.push_activity(format!("switch: {error}")),
            },
            ConsoleAction::ApplyPending => match app::apply_pending_proxy(paths) {
                Ok(message) => {
                    console.force_refresh = true;
                    console.set_result("Pending", vec![message]);
                }
                Err(error) => console.push_activity(format!("pending: {error}")),
            },
            ConsoleAction::WatchControl => {
                let result = if snapshot.watcher_online {
                    app::restart_background_watcher(paths)
                } else {
                    app::ensure_watcher_running(paths).map(|started| {
                        if started {
                            "Watcher запущен.".to_string()
                        } else {
                            "Watcher уже активен.".to_string()
                        }
                    })
                };
                match result {
                    Ok(message) => {
                        console.force_refresh = true;
                        console.set_result("Watcher", vec![message]);
                    }
                    Err(error) => console.push_activity(format!("watcher: {error}")),
                }
            }
            ConsoleAction::StopWatcher => match app::stop_background_watcher(paths) {
                Ok(stopped) => {
                    console.force_refresh = true;
                    console.set_result(
                        "Watcher",
                        vec![if stopped == 0 {
                            "Фоновый watcher не найден.".to_string()
                        } else {
                            format!("Остановлено headless watcher-процессов: {stopped}")
                        }],
                    )
                }
                Err(error) => console.push_activity(format!("watcher stop: {error}")),
            },
            ConsoleAction::StopAll => {
                let _ = app::stop_all_protoswitch_processes(paths);
                return Ok(());
            }
            ConsoleAction::ToggleAutostart => {
                let enable = !(snapshot.autostart.installed || snapshot.config.autostart.enabled);
                match app::set_autostart_enabled(paths, enable) {
                    Ok(message) => {
                        console.force_refresh = true;
                        console.set_result("Autostart", vec![message]);
                    }
                    Err(error) => console.push_activity(format!("autostart: {error}")),
                }
            }
            ConsoleAction::ToggleAutoCleanup => {
                let enable = !snapshot.config.watcher.auto_cleanup_dead_proxies;
                match app::set_auto_cleanup_enabled(paths, enable) {
                    Ok(message) => {
                        console.force_refresh = true;
                        console.set_result("Auto-clean", vec![message]);
                    }
                    Err(error) => console.push_activity(format!("autoclean: {error}")),
                }
            }
            ConsoleAction::ToggleSocks5Fallback => {
                let enable = !snapshot.config.provider.enable_socks5_fallback;
                match app::set_socks5_fallback_enabled(paths, enable) {
                    Ok(message) => {
                        console.force_refresh = true;
                        console.set_result("Providers", vec![message]);
                    }
                    Err(error) => console.push_activity(format!("providers: {error}")),
                }
            }
            ConsoleAction::Settings => {
                let current = snapshot.config.clone();
                let original_marker = toml::to_string(&current)?;
                drop(session);
                let edited = run_setup(current)?;
                let edited_marker = toml::to_string(&edited)?;
                session = TerminalSession::new()?;

                if original_marker == edited_marker {
                    console.set_result(
                        "Settings",
                        vec!["Изменений нет. Конфиг оставлен без правок.".to_string()],
                    );
                } else {
                    match app::persist_config(paths, edited) {
                        Ok(message) => {
                            console.force_refresh = true;
                            console.set_result("Settings", vec![message]);
                        }
                        Err(error) => console.push_activity(format!("settings: {error}")),
                    }
                }
            }
            ConsoleAction::Doctor => console.start_doctor(paths),
            ConsoleAction::OpenLog => match app::open_in_notepad(&paths.log_file) {
                Ok(_) => {
                    console.set_result("Log", vec![format!("Открыт {}", paths.log_file.display())])
                }
                Err(error) => console.push_activity(format!("log: {error}")),
            },
            ConsoleAction::OpenDataDir => match app::open_in_shell(&paths.local_dir) {
                Ok(_) => console.set_result(
                    "Data",
                    vec![format!("Открыта {}", paths.local_dir.display())],
                ),
                Err(error) => console.push_activity(format!("data: {error}")),
            },
            ConsoleAction::Refresh => {
                session.terminal.clear()?;
                console.force_refresh = true;
                console.clear_inspector();
                console.set_result(
                    "Refresh",
                    vec!["Снимок обновлён и экран перерисован.".to_string()],
                );
            }
        }
    }
}

fn render_setup(frame: &mut ratatui::Frame<'_>, draft: &SetupDraft) {
    let area = frame.area();
    frame.render_widget(Clear, area);

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(16),
            Constraint::Length(3),
        ])
        .split(area);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(52), Constraint::Percentage(48)])
        .split(vertical[1]);

    let title = Paragraph::new(Line::from(vec![
        Span::styled("ProtoSwitch", title_style()),
        Span::raw(" "),
        Span::styled(APP_VERSION, muted_style()),
        Span::raw("  "),
        badge("настройка", Color::Rgb(118, 201, 160)),
    ]))
    .block(panel("Первый запуск"));
    frame.render_widget(title, vertical[0]);

    let fields = draft
        .fields()
        .into_iter()
        .enumerate()
        .map(|(index, field)| {
            let selected = index == draft.focus;
            let marker = if selected { "› " } else { "  " };
            ListItem::new(Line::from(vec![
                Span::styled(
                    marker,
                    if selected {
                        selected_style()
                    } else {
                        muted_style()
                    },
                ),
                Span::styled(
                    format!("{:<24}", field.label),
                    if selected {
                        selected_style()
                    } else {
                        text_style()
                    },
                ),
                Span::styled(field.value, value_style(selected, false)),
            ]))
        })
        .collect::<Vec<_>>();
    frame.render_widget(List::new(fields).block(panel("Параметры")), body[0]);

    let active = draft.current_field();
    let info_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Length(8)])
        .split(body[1]);

    let details = vec![
        Line::from(vec![
            Span::styled(active.label, title_style()),
            Span::raw("  "),
            Span::styled(active.value.clone(), value_style(true, false)),
        ]),
        Line::from(""),
        Line::from(active.description),
        Line::from(""),
        kv_line("Пул", draft.provider_pool(), info_rows[0].width),
        kv_line("Источники", draft.enabled_sources(), info_rows[0].width),
        kv_line(
            "SOCKS5 fallback",
            if draft.socks5_fallback {
                "включён".to_string()
            } else {
                "выключен".to_string()
            },
            info_rows[0].width,
        ),
        kv_line(
            "Автоподчистка",
            if draft.auto_cleanup_dead_proxies {
                "включена".to_string()
            } else {
                "выключена".to_string()
            },
            info_rows[0].width,
        ),
    ];
    frame.render_widget(
        Paragraph::new(details)
            .block(toned_panel("Контекст", &active.value))
            .wrap(Wrap { trim: true }),
        info_rows[0],
    );

    let summary = vec![
        kv_line(
            "Проверка",
            format!("{} сек", draft.check_interval_secs),
            info_rows[1].width,
        ),
        kv_line(
            "TCP timeout",
            format!("{} сек", draft.connect_timeout_secs),
            info_rows[1].width,
        ),
        kv_line(
            "Порог сбоев",
            draft.failure_threshold.to_string(),
            info_rows[1].width,
        ),
        kv_line(
            "История proxy",
            draft.history_size.to_string(),
            info_rows[1].width,
        ),
        kv_line(
            "Автозапуск",
            if draft.autostart_enabled {
                "включён".to_string()
            } else {
                "выключен".to_string()
            },
            info_rows[1].width,
        ),
    ];
    frame.render_widget(
        Paragraph::new(summary)
            .block(panel("Резюме"))
            .wrap(Wrap { trim: true }),
        info_rows[1],
    );

    let footer = Paragraph::new("Up/Down поле  Left/Right изменить  Enter сохранить  Esc выйти")
        .style(muted_style())
        .block(panel("Управление"));
    frame.render_widget(footer, vertical[2]);
}

fn render_console(
    frame: &mut ratatui::Frame<'_>,
    paths: &AppPaths,
    snapshot: &UiSnapshot,
    console: &ConsoleState,
    actions: &[ConsoleAction],
) {
    let area = frame.area();
    frame.render_widget(Clear, area);

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Length(3),
            Constraint::Min(14),
            Constraint::Length(3),
        ])
        .split(area);

    render_session_header(frame, vertical[0], snapshot);

    frame.render_widget(
        Paragraph::new(section_tabs(console.section))
            .block(panel("Разделы"))
            .wrap(Wrap { trim: true }),
        vertical[1],
    );

    match console.section {
        ConsoleSection::Dashboard => {
            render_dashboard_responsive(frame, vertical[2], snapshot, console)
        }
        ConsoleSection::Actions => {
            render_actions_responsive(frame, vertical[2], paths, snapshot, console, actions)
        }
        ConsoleSection::Providers => {
            render_providers_responsive(frame, vertical[2], snapshot, console)
        }
        ConsoleSection::History => render_history_responsive(frame, vertical[2], snapshot, console),
    }

    let footer = Paragraph::new(footer_hint(console.section))
        .style(muted_style())
        .block(panel("Клавиши"));
    frame.render_widget(footer, vertical[3]);
}

#[allow(dead_code)]
fn render_dashboard(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    snapshot: &UiSnapshot,
    console: &ConsoleState,
) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(9),
            Constraint::Length(8),
            Constraint::Min(7),
        ])
        .split(area);

    let hero = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(56), Constraint::Percentage(44)])
        .split(rows[0]);

    let middle = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
        .split(rows[1]);

    let current_proxy_style = snapshot
        .state
        .current_proxy
        .as_ref()
        .map(|record| protocol_style(record.proxy.kind))
        .unwrap_or_else(muted_style);
    let pending_style = snapshot
        .state
        .pending_proxy
        .as_ref()
        .map(|record| protocol_style(record.proxy.kind))
        .unwrap_or_else(muted_style);
    let route_width = hero[0].width;
    let runtime_width = hero[1].width;

    let route_lines = vec![
        Line::from(vec![
            Span::styled("Current  ", muted_style()),
            Span::styled(
                compact_to_width(
                    &snapshot
                        .state
                        .current_proxy
                        .as_ref()
                        .map(|record| record.proxy.short_label())
                        .unwrap_or_else(|| "не выбран".to_string()),
                    route_width,
                    14,
                ),
                current_proxy_style,
            ),
        ]),
        Line::from(vec![
            Span::styled("Status   ", muted_style()),
            Span::styled(
                compact_to_width(
                    &app::current_proxy_status_text(&snapshot.state),
                    route_width,
                    14,
                ),
                semantic_style(&app::current_proxy_status_text(&snapshot.state)),
            ),
        ]),
        Line::from(vec![
            Span::styled("Source   ", muted_style()),
            Span::styled(
                compact_to_width(
                    &snapshot
                        .state
                        .current_proxy
                        .as_ref()
                        .map(|record| record.source.clone())
                        .unwrap_or_else(|| "ещё не закреплён".to_string()),
                    route_width,
                    14,
                ),
                text_style(),
            ),
        ]),
        Line::from(vec![
            Span::styled("Pending  ", muted_style()),
            Span::styled(
                compact_to_width(
                    &snapshot
                        .state
                        .pending_proxy
                        .as_ref()
                        .map(|record| record.proxy.short_label())
                        .unwrap_or_else(|| "пусто".to_string()),
                    route_width,
                    14,
                ),
                pending_style,
            ),
        ]),
        kv_line(
            "Last apply",
            format_time(snapshot.state.last_apply_at.as_ref()),
            route_width,
        ),
        kv_line(
            "Last fetch",
            format_time(snapshot.state.last_fetch_at.as_ref()),
            route_width,
        ),
    ];
    frame.render_widget(
        Paragraph::new(route_lines)
            .block(toned_panel(
                "Proxy",
                &app::current_proxy_status_text(&snapshot.state),
            ))
            .wrap(Wrap { trim: true }),
        hero[0],
    );

    let runtime_lines = vec![
        kv_line(
            "Watcher",
            format!(
                "{} / fail {}/{}",
                mode_label(&snapshot.state.watcher.mode),
                snapshot.state.watcher.failure_streak,
                snapshot.config.watcher.failure_threshold
            ),
            runtime_width,
        ),
        kv_line(
            "Telegram",
            if snapshot.state.watcher.telegram_running {
                "запущен".to_string()
            } else {
                "не запущен".to_string()
            },
            runtime_width,
        ),
        kv_line(
            "Source state",
            app::source_status_text(&snapshot.state),
            runtime_width,
        ),
        kv_line(
            "Next check",
            format_time(snapshot.state.watcher.next_check_at.as_ref()),
            runtime_width,
        ),
        kv_line(
            "Autostart",
            if snapshot.autostart.installed || snapshot.config.autostart.enabled {
                snapshot
                    .autostart
                    .method
                    .as_ref()
                    .map(autostart_method_label)
                    .unwrap_or("pending")
                    .to_string()
            } else {
                "выключен".to_string()
            },
            runtime_width,
        ),
        kv_line(
            "Auto-clean",
            if snapshot.config.watcher.auto_cleanup_dead_proxies {
                "включён".to_string()
            } else {
                "выключен".to_string()
            },
            runtime_width,
        ),
    ];
    frame.render_widget(
        Paragraph::new(runtime_lines)
            .block(toned_panel(
                "Среда",
                &app::source_status_text(&snapshot.state),
            ))
            .wrap(Wrap { trim: true }),
        hero[1],
    );

    let failure_ratio = if snapshot.config.watcher.failure_threshold == 0 {
        0.0
    } else {
        let remaining = snapshot
            .config
            .watcher
            .failure_threshold
            .saturating_sub(snapshot.state.watcher.failure_streak);
        remaining as f64 / snapshot.config.watcher.failure_threshold as f64
    };
    render_gauge_card(
        frame,
        middle[0],
        "Здоровье watcher",
        failure_ratio,
        &format!(
            "{}/{} failures",
            snapshot.state.watcher.failure_streak, snapshot.config.watcher.failure_threshold
        ),
        &format!("mode: {}", mode_label(&snapshot.state.watcher.mode)),
    );

    let active_sources = snapshot.config.provider.active_sources();
    let (mtproto_count, socks5_count) = snapshot.config.provider.source_counts();
    let total_sources = active_sources.len().max(1) as f64;
    render_gauge_card(
        frame,
        middle[1],
        "Состав источников",
        mtproto_count as f64 / total_sources,
        &app::provider_pool_summary(&snapshot.config),
        &format!("{socks5_count} SOCKS5 source(s) active"),
    );

    let ready_count = [
        snapshot.watcher_online,
        snapshot.state.watcher.telegram_running,
        !active_sources.is_empty(),
    ]
    .into_iter()
    .filter(|value| *value)
    .count();
    render_gauge_card(
        frame,
        middle[2],
        "Готовность",
        ready_count as f64 / 3.0,
        &format!("{ready_count}/3 ready"),
        &compact_to_width(
            &app::enabled_sources_summary(&snapshot.config),
            middle[2].width,
            10,
        ),
    );

    frame.render_widget(
        Paragraph::new(console.activity_lines(rows[2].width))
            .block(panel("Сигналы"))
            .wrap(Wrap { trim: true }),
        rows[2],
    );
}

#[allow(dead_code)]
fn render_actions(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    paths: &AppPaths,
    snapshot: &UiSnapshot,
    console: &ConsoleState,
    actions: &[ConsoleAction],
) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(34), Constraint::Percentage(66)])
        .split(area);

    let action_items = actions
        .iter()
        .enumerate()
        .map(|(index, action)| {
            let selected = index == console.focus;
            let style = if selected {
                selected_style()
            } else {
                text_style()
            };
            ListItem::new(Line::from(vec![
                Span::styled(if selected { "> " } else { "  " }, style),
                Span::styled(action.label(snapshot), style),
                Span::raw("  "),
                Span::styled(action.shortcut(), muted_style()),
            ]))
        })
        .collect::<Vec<_>>();
    frame.render_widget(List::new(action_items).block(panel("Команды")), columns[0]);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(11), Constraint::Min(8)])
        .split(columns[1]);

    let detail = if console.inspector_lines.is_empty() {
        action_description_lines(actions[console.focus], snapshot, paths)
    } else {
        console.inspector_lines.clone()
    };
    frame.render_widget(
        Paragraph::new(detail)
            .block(panel(&console.inspector_title))
            .wrap(Wrap { trim: true }),
        right[0],
    );

    frame.render_widget(
        Paragraph::new(console.activity_lines(right[1].width))
            .block(panel("Сигналы"))
            .wrap(Wrap { trim: true }),
        right[1],
    );
}

#[allow(dead_code)]
fn render_providers(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    snapshot: &UiSnapshot,
    console: &ConsoleState,
) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Min(10)])
        .split(area);

    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
        .split(rows[0]);

    render_status_card(
        frame,
        top[0],
        "Пул",
        app::provider_pool_summary(&snapshot.config),
        app::enabled_sources_summary(&snapshot.config),
    );
    render_status_card(
        frame,
        top[1],
        "Состояние источника",
        compact_to_width(&app::source_status_text(&snapshot.state), top[1].width, 6),
        snapshot
            .state
            .current_proxy
            .as_ref()
            .map(|record| format!("last good feed: {}", record.source))
            .unwrap_or_else(|| "ещё нет подтверждённого источника".to_string()),
    );
    render_status_card(
        frame,
        top[2],
        "Fallback",
        if snapshot.config.provider.enable_socks5_fallback {
            "SOCKS5 fallback включён".to_string()
        } else {
            "только MTProto".to_string()
        },
        "F переключает добавочные SOCKS5-источники".to_string(),
    );

    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(rows[1]);

    frame.render_widget(
        List::new(provider_source_items(snapshot, bottom[0].width))
            .block(panel("Встроенные ленты")),
        bottom[0],
    );

    let provider_lines = vec![
        kv_line(
            "Fetch tries",
            snapshot.config.provider.fetch_attempts.to_string(),
            bottom[1].width,
        ),
        kv_line(
            "Retry delay",
            format!("{} ms", snapshot.config.provider.fetch_retry_delay_ms),
            bottom[1].width,
        ),
        kv_line(
            "Active feeds",
            snapshot.config.provider.active_sources().len().to_string(),
            bottom[1].width,
        ),
        kv_line(
            "Current source",
            snapshot
                .state
                .current_proxy
                .as_ref()
                .map(|record| record.source.clone())
                .unwrap_or_else(|| "ещё не закреплён".to_string()),
            bottom[1].width,
        ),
        kv_line(
            "Last error",
            snapshot
                .state
                .last_error
                .as_ref()
                .map(|value| value.to_string())
                .unwrap_or_else(|| "нет".to_string()),
            bottom[1].width,
        ),
        Line::from(""),
        Line::from(Span::styled("Signals", title_style())),
    ]
    .into_iter()
    .chain(console.activity_lines(bottom[1].width))
    .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(provider_lines)
            .block(panel("Политика источников"))
            .wrap(Wrap { trim: true }),
        bottom[1],
    );
}

#[allow(dead_code)]
fn render_history(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    snapshot: &UiSnapshot,
    console: &ConsoleState,
) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(52), Constraint::Percentage(48)])
        .split(area);

    frame.render_widget(
        List::new(history_items(&snapshot.state, columns[0].width)).block(panel("Последние proxy")),
        columns[0],
    );

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(10), Constraint::Min(8)])
        .split(columns[1]);

    let detail = vec![
        kv_line(
            "Last fetch",
            format_time(snapshot.state.last_fetch_at.as_ref()),
            right[0].width,
        ),
        kv_line(
            "Last apply",
            format_time(snapshot.state.last_apply_at.as_ref()),
            right[0].width,
        ),
        kv_line(
            "Current source",
            snapshot
                .state
                .current_proxy
                .as_ref()
                .map(|record| record.source.clone())
                .unwrap_or_else(|| "нет".to_string()),
            right[0].width,
        ),
        kv_line(
            "Config",
            snapshot.paths.config_file.display().to_string(),
            right[0].width,
        ),
        kv_line(
            "State",
            snapshot.paths.state_file.display().to_string(),
            right[0].width,
        ),
        kv_line(
            "Log",
            snapshot.paths.log_file.display().to_string(),
            right[0].width,
        ),
    ];
    frame.render_widget(
        Paragraph::new(detail)
            .block(panel("Хронология"))
            .wrap(Wrap { trim: true }),
        right[0],
    );
    frame.render_widget(
        Paragraph::new(console.activity_lines(right[1].width))
            .block(panel("Сигналы"))
            .wrap(Wrap { trim: true }),
        right[1],
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WidthMode {
    Narrow,
    Regular,
    Wide,
}

fn width_mode(width: u16) -> WidthMode {
    if width < 108 {
        WidthMode::Narrow
    } else if width < 148 {
        WidthMode::Regular
    } else {
        WidthMode::Wide
    }
}

fn render_session_header(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    snapshot: &UiSnapshot,
) {
    let current_status = app::current_proxy_status_text(&snapshot.state);
    let summary = app::overall_summary_text(&snapshot.state);
    let background = app::background_summary_text(&snapshot.state);
    let panel = panel("Сеанс");
    let inner = panel.inner(area);
    frame.render_widget(panel, area);
    frame.render_widget(
        Paragraph::new(vec![
            session_badges_line(snapshot, &current_status, inner.width),
            Line::from(vec![
                Span::styled("Итог       ", muted_style()),
                Span::styled(
                    compact_to_width(&summary, inner.width, 12),
                    semantic_style(&summary),
                ),
            ]),
            Line::from(vec![
                Span::styled("Фон        ", muted_style()),
                Span::styled(compact_to_width(&background, inner.width, 12), text_style()),
            ]),
        ])
        .wrap(Wrap { trim: true }),
        inner,
    );
}

fn session_badges_line(snapshot: &UiSnapshot, current_status: &str, width: u16) -> Line<'static> {
    let mut spans = vec![
        Span::styled("ProtoSwitch", title_style()),
        Span::raw(" "),
        Span::styled(APP_VERSION, muted_style()),
    ];
    let mode = width_mode(width);
    let mut push_badge = |text: String, background: Color| {
        spans.push(Span::raw("  "));
        spans.push(badge(text, background));
    };

    push_badge(
        if snapshot.watcher_online {
            "watcher активен".to_string()
        } else {
            "watcher ждёт".to_string()
        },
        if snapshot.watcher_online {
            Color::Rgb(118, 201, 160)
        } else {
            Color::Rgb(247, 190, 103)
        },
    );

    if !matches!(mode, WidthMode::Narrow) {
        push_badge(
            if snapshot.state.watcher.telegram_running {
                "Telegram открыт".to_string()
            } else {
                "Telegram закрыт".to_string()
            },
            if snapshot.state.watcher.telegram_running {
                Color::Rgb(109, 175, 255)
            } else {
                Color::Rgb(229, 118, 118)
            },
        );
    }

    if matches!(mode, WidthMode::Wide) {
        push_badge(
            if snapshot.config.provider.enable_socks5_fallback {
                "SOCKS5 готов".to_string()
            } else {
                "только MTProto".to_string()
            },
            if snapshot.config.provider.enable_socks5_fallback {
                Color::Rgb(118, 201, 160)
            } else {
                Color::Rgb(247, 190, 103)
            },
        );
    }

    push_badge(
        compact_to_width(current_status, width, 24),
        tone_color(current_status),
    );

    Line::from(spans)
}

fn render_dashboard_responsive(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    snapshot: &UiSnapshot,
    console: &ConsoleState,
) {
    let overall_summary = app::overall_summary_text(&snapshot.state);
    let background_summary = app::background_summary_text(&snapshot.state);
    let next_step = app::next_step_text(&snapshot.state);
    let proxy_status = app::current_proxy_status_text(&snapshot.state);
    let source_status = app::source_status_text(&snapshot.state);
    let backend_status = app::backend_status_text(&snapshot.state, None);
    let backend_route = app::backend_route_text(&snapshot.state, None);
    let active_sources = snapshot.config.provider.active_sources();
    let ready_count = [
        snapshot.watcher_online,
        snapshot.state.watcher.telegram_running,
        !active_sources.is_empty(),
    ]
    .into_iter()
    .filter(|value| *value)
    .count();

    let overview_lines = |width: u16| {
        let mut lines = Vec::new();
        lines.extend(kv_lines("Сейчас", overall_summary.clone(), width));
        lines.extend(kv_lines("Дальше", next_step.clone(), width));
        lines
    };

    let proxy_lines = |width: u16| {
        vec![
            kv_line(
                "Текущий",
                snapshot
                    .state
                    .current_proxy
                    .as_ref()
                    .map(|record| record.proxy.short_label())
                    .unwrap_or_else(|| "не выбран".to_string()),
                width,
            ),
            kv_line("Статус", proxy_status.clone(), width),
            kv_line(
                "Источник",
                snapshot
                    .state
                    .current_proxy
                    .as_ref()
                    .map(|record| record.source.clone())
                    .unwrap_or_else(|| "ещё не закреплён".to_string()),
                width,
            ),
            kv_line(
                "Pending",
                snapshot
                    .state
                    .pending_proxy
                    .as_ref()
                    .map(|record| record.proxy.short_label())
                    .unwrap_or_else(|| "пусто".to_string()),
                width,
            ),
            kv_line(
                "Apply",
                format_time(snapshot.state.last_apply_at.as_ref()),
                width,
            ),
        ]
    };

    let telegram_lines = |width: u16| {
        let mut lines = Vec::new();
        lines.extend(kv_lines(
            "Watcher",
            format!(
                "{} / сбои {}/{}",
                mode_label_ru(&snapshot.state.watcher.mode),
                snapshot.state.watcher.failure_streak,
                snapshot.config.watcher.failure_threshold
            ),
            width,
        ));
        lines.extend(kv_lines(
            "Telegram",
            if snapshot.state.watcher.telegram_running {
                "запущен".to_string()
            } else {
                "не запущен".to_string()
            },
            width,
        ));
        lines.extend(kv_lines(
            "Backend",
            backend_status.clone(),
            width,
        ));
        lines.extend(kv_lines(
            "Путь",
            backend_route.clone(),
            width,
        ));
        lines.extend(kv_lines(
            "Рестарт",
            if snapshot.state.backend_restart_required {
                "нужен".to_string()
            } else {
                "не нужен".to_string()
            },
            width,
        ));
        lines
    };

    let source_lines = |width: u16| {
        let mut lines = Vec::new();
        lines.extend(kv_lines("Статус", source_status.clone(), width));
        lines.extend(kv_lines(
            "Пул",
            app::provider_pool_summary(&snapshot.config),
            width,
        ));
        lines.extend(kv_lines(
            "Ленты",
            app::enabled_sources_summary(&snapshot.config),
            width,
        ));
        lines.extend(kv_lines(
            "Готовн.",
            format!("{ready_count}/3"),
            width,
        ));
        lines.extend(kv_lines(
            "Проверка",
            format_time(snapshot.state.watcher.next_check_at.as_ref()),
            width,
        ));
        lines
    };

    let comfort_lines = |width: u16| {
        let mut lines = Vec::new();
        lines.extend(kv_lines("Фон", background_summary.clone(), width));
        lines.extend(kv_lines(
            "Автозап.",
            if snapshot.autostart.installed || snapshot.config.autostart.enabled {
                snapshot
                    .autostart
                    .method
                    .as_ref()
                    .map(autostart_method_label)
                    .unwrap_or("pending")
                    .to_string()
            } else {
                "выключен".to_string()
            },
            width,
        ));
        lines.extend(kv_lines(
            "Чистка",
            if snapshot.config.watcher.auto_cleanup_dead_proxies {
                "включена".to_string()
            } else {
                "выключена".to_string()
            },
            width,
        ));
        lines.extend(kv_lines(
            "SOCKS5",
            if snapshot.config.provider.enable_socks5_fallback {
                "fallback включён".to_string()
            } else {
                "fallback выключен".to_string()
            },
            width,
        ));
        lines.extend(kv_lines(
            "ОС",
            platform::current_os_label().to_string(),
            width,
        ));
        lines
    };

    match width_mode(area.width) {
        WidthMode::Narrow => {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(6),
                    Constraint::Length(7),
                    Constraint::Length(7),
                    Constraint::Length(7),
                    Constraint::Min(6),
                ])
                .split(area);
            render_lines_panel(frame, rows[0], "Сейчас", &overall_summary, overview_lines(rows[0].width));
            render_lines_panel(frame, rows[1], "Proxy", &proxy_status, proxy_lines(rows[1].width));
            render_lines_panel(
                frame,
                rows[2],
                "Telegram",
                &backend_status,
                telegram_lines(rows[2].width),
            );
            render_lines_panel(
                frame,
                rows[3],
                "Источники",
                &source_status,
                source_lines(rows[3].width),
            );
            render_activity_panel(frame, rows[4], console);
        }
        WidthMode::Regular => {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(6),
                    Constraint::Length(7),
                    Constraint::Length(8),
                    Constraint::Min(6),
                ])
                .split(area);
            let middle = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(rows[1]);
            let bottom = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(rows[2]);
            render_lines_panel(frame, rows[0], "Сейчас", &overall_summary, overview_lines(rows[0].width));
            render_lines_panel(frame, middle[0], "Proxy", &proxy_status, proxy_lines(middle[0].width));
            render_lines_panel(
                frame,
                middle[1],
                "Telegram",
                &backend_status,
                telegram_lines(middle[1].width),
            );
            render_lines_panel(
                frame,
                bottom[0],
                "Источники",
                &source_status,
                source_lines(bottom[0].width),
            );
            render_lines_panel(
                frame,
                bottom[1],
                "Тихий режим",
                &background_summary,
                comfort_lines(bottom[1].width),
            );
            render_activity_panel(frame, rows[3], console);
        }
        WidthMode::Wide => {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(6),
                    Constraint::Length(8),
                    Constraint::Length(8),
                    Constraint::Min(6),
                ])
                .split(area);
            let middle = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(34),
                    Constraint::Percentage(33),
                    Constraint::Percentage(33),
                ])
                .split(rows[1]);
            let bottom = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(rows[2]);
            render_lines_panel(frame, rows[0], "Сейчас", &overall_summary, overview_lines(rows[0].width));
            render_lines_panel(frame, middle[0], "Proxy", &proxy_status, proxy_lines(middle[0].width));
            render_lines_panel(
                frame,
                middle[1],
                "Telegram",
                &backend_status,
                telegram_lines(middle[1].width),
            );
            render_lines_panel(
                frame,
                middle[2],
                "Источники",
                &source_status,
                source_lines(middle[2].width),
            );
            render_lines_panel(
                frame,
                bottom[0],
                "Тихий режим",
                &background_summary,
                comfort_lines(bottom[0].width),
            );
            render_activity_panel(frame, bottom[1], console);
        }
    }
}

fn render_actions_responsive(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    paths: &AppPaths,
    snapshot: &UiSnapshot,
    console: &ConsoleState,
    actions: &[ConsoleAction],
) {
    let action_items = actions
        .iter()
        .enumerate()
        .map(|(index, action)| {
            let selected = index == console.focus;
            let style = if selected {
                selected_style()
            } else {
                text_style()
            };
            ListItem::new(Line::from(vec![
                Span::styled(if selected { "> " } else { "  " }, style),
                Span::styled(action.label(snapshot), style),
                Span::raw("  "),
                Span::styled(action.shortcut(), muted_style()),
            ]))
        })
        .collect::<Vec<_>>();
    let detail = if console.inspector_lines.is_empty() {
        action_description_lines(actions[console.focus], snapshot, paths)
    } else {
        console.inspector_lines.clone()
    };

    match width_mode(area.width) {
        WidthMode::Narrow => {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(12),
                    Constraint::Length(10),
                    Constraint::Min(6),
                ])
                .split(area);
            frame.render_widget(List::new(action_items).block(panel("Команды")), rows[0]);
            frame.render_widget(
                Paragraph::new(detail)
                    .block(panel(&console.inspector_title))
                    .wrap(Wrap { trim: true }),
                rows[1],
            );
            render_activity_panel(frame, rows[2], console);
        }
        _ => {
            let columns = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(34), Constraint::Percentage(66)])
                .split(area);
            frame.render_widget(List::new(action_items).block(panel("Команды")), columns[0]);
            let right = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(11), Constraint::Min(8)])
                .split(columns[1]);
            frame.render_widget(
                Paragraph::new(detail)
                    .block(panel(&console.inspector_title))
                    .wrap(Wrap { trim: true }),
                right[0],
            );
            render_activity_panel(frame, right[1], console);
        }
    }
}

fn render_providers_responsive(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    snapshot: &UiSnapshot,
    console: &ConsoleState,
) {
    let state_headline = app::source_status_text(&snapshot.state);
    let fallback_status = if snapshot.config.provider.enable_socks5_fallback {
        "SOCKS5 fallback включён".to_string()
    } else {
        "только MTProto".to_string()
    };

    match width_mode(area.width) {
        WidthMode::Narrow => {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(6),
                    Constraint::Length(6),
                    Constraint::Length(6),
                    Constraint::Min(8),
                    Constraint::Min(6),
                ])
                .split(area);
            render_status_card(
                frame,
                rows[0],
                "Pool",
                app::provider_pool_summary(&snapshot.config),
                app::enabled_sources_summary(&snapshot.config),
            );
            render_status_card(
                frame,
                rows[1],
                "Состояние источника",
                state_headline,
                snapshot
                    .state
                    .current_proxy
                    .as_ref()
                    .map(|record| format!("last good feed: {}", record.source))
                    .unwrap_or_else(|| "ещё нет подтверждённого источника".to_string()),
            );
            render_status_card(
                frame,
                rows[2],
                "Резерв",
                fallback_status,
                "F переключает запасные SOCKS5-ленты".to_string(),
            );
            frame.render_widget(
                List::new(provider_source_items(snapshot, rows[3].width)).block(panel("Ленты")),
                rows[3],
            );
            render_activity_panel(frame, rows[4], console);
        }
        _ => {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(8), Constraint::Min(10)])
                .split(area);
            let top = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(34),
                    Constraint::Percentage(33),
                    Constraint::Percentage(33),
                ])
                .split(rows[0]);
            render_status_card(
                frame,
                top[0],
                "Pool",
                app::provider_pool_summary(&snapshot.config),
                app::enabled_sources_summary(&snapshot.config),
            );
            render_status_card(
                frame,
                top[1],
                "Состояние источника",
                app::source_status_text(&snapshot.state),
                snapshot
                    .state
                    .current_proxy
                    .as_ref()
                    .map(|record| format!("last good feed: {}", record.source))
                    .unwrap_or_else(|| "ещё нет подтверждённого источника".to_string()),
            );
            render_status_card(
                frame,
                top[2],
                "Резерв",
                if snapshot.config.provider.enable_socks5_fallback {
                    "SOCKS5 fallback включён".to_string()
                } else {
                    "только MTProto".to_string()
                },
                "F переключает запасные SOCKS5-ленты".to_string(),
            );
            let bottom = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
                .split(rows[1]);
            frame.render_widget(
                List::new(provider_source_items(snapshot, bottom[0].width)).block(panel("Ленты")),
                bottom[0],
            );
            frame.render_widget(
                Paragraph::new({
                    let mut lines = Vec::new();
                    lines.extend(kv_lines(
                        "Fetch tries",
                        snapshot.config.provider.fetch_attempts.to_string(),
                        bottom[1].width,
                    ));
                    lines.extend(kv_lines(
                        "Retry delay",
                        format!("{} ms", snapshot.config.provider.fetch_retry_delay_ms),
                        bottom[1].width,
                    ));
                    lines.extend(kv_lines(
                        "Active feeds",
                        snapshot.config.provider.active_sources().len().to_string(),
                        bottom[1].width,
                    ));
                    lines.extend(kv_lines(
                        "Current source",
                        snapshot
                            .state
                            .current_proxy
                            .as_ref()
                            .map(|record| record.source.clone())
                            .unwrap_or_else(|| "ещё не закреплён".to_string()),
                        bottom[1].width,
                    ));
                    lines.extend(kv_lines(
                        "Last error",
                        snapshot
                            .state
                            .last_error
                            .clone()
                            .unwrap_or_else(|| "нет".to_string()),
                        bottom[1].width,
                    ));
                    lines.push(Line::from(""));
                    lines
                })
                .block(panel("Политика"))
                .wrap(Wrap { trim: true }),
                bottom[1],
            );
        }
    }
}

fn render_history_responsive(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    snapshot: &UiSnapshot,
    console: &ConsoleState,
) {
    match width_mode(area.width) {
        WidthMode::Narrow => {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(8),
                    Constraint::Length(9),
                    Constraint::Min(6),
                ])
                .split(area);
            frame.render_widget(
                List::new(history_items(&snapshot.state, rows[0].width))
                    .block(panel("Последние proxy")),
                rows[0],
            );
            frame.render_widget(
                Paragraph::new({
                    let mut lines = Vec::new();
                    lines.extend(kv_lines(
                        "Last fetch",
                        format_time(snapshot.state.last_fetch_at.as_ref()),
                        rows[1].width,
                    ));
                    lines.extend(kv_lines(
                        "Last apply",
                        format_time(snapshot.state.last_apply_at.as_ref()),
                        rows[1].width,
                    ));
                    lines.extend(kv_lines(
                        "Current source",
                        snapshot
                            .state
                            .current_proxy
                            .as_ref()
                            .map(|record| record.source.clone())
                            .unwrap_or_else(|| "нет".to_string()),
                        rows[1].width,
                    ));
                    lines.extend(kv_lines(
                        "Config",
                        snapshot.paths.config_file.display().to_string(),
                        rows[1].width,
                    ));
                    lines.extend(kv_lines(
                        "State",
                        snapshot.paths.state_file.display().to_string(),
                        rows[1].width,
                    ));
                    lines.extend(kv_lines(
                        "Log",
                        snapshot.paths.log_file.display().to_string(),
                        rows[1].width,
                    ));
                    lines
                })
                .block(panel("Хронология"))
                .wrap(Wrap { trim: true }),
                rows[1],
            );
            render_activity_panel(frame, rows[2], console);
        }
        _ => {
            let columns = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(52), Constraint::Percentage(48)])
                .split(area);
            frame.render_widget(
                List::new(history_items(&snapshot.state, columns[0].width))
                    .block(panel("Последние proxy")),
                columns[0],
            );
            let right = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(10), Constraint::Min(8)])
                .split(columns[1]);
            frame.render_widget(
                Paragraph::new({
                    let mut lines = Vec::new();
                    lines.extend(kv_lines(
                        "Last fetch",
                        format_time(snapshot.state.last_fetch_at.as_ref()),
                        right[0].width,
                    ));
                    lines.extend(kv_lines(
                        "Last apply",
                        format_time(snapshot.state.last_apply_at.as_ref()),
                        right[0].width,
                    ));
                    lines.extend(kv_lines(
                        "Current source",
                        snapshot
                            .state
                            .current_proxy
                            .as_ref()
                            .map(|record| record.source.clone())
                            .unwrap_or_else(|| "нет".to_string()),
                        right[0].width,
                    ));
                    lines.extend(kv_lines(
                        "Config",
                        snapshot.paths.config_file.display().to_string(),
                        right[0].width,
                    ));
                    lines.extend(kv_lines(
                        "State",
                        snapshot.paths.state_file.display().to_string(),
                        right[0].width,
                    ));
                    lines.extend(kv_lines(
                        "Log",
                        snapshot.paths.log_file.display().to_string(),
                        right[0].width,
                    ));
                    lines
                })
                .block(panel("Хронология"))
                .wrap(Wrap { trim: true }),
                right[0],
            );
            render_activity_panel(frame, right[1], console);
        }
    }
}

fn render_activity_panel(frame: &mut ratatui::Frame<'_>, area: Rect, console: &ConsoleState) {
    frame.render_widget(
        Paragraph::new(console.activity_lines_for_area(area.width, area.height.saturating_sub(2)))
            .block(panel("Сигналы"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_lines_panel(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    title: &str,
    signal: &str,
    lines: Vec<Line<'static>>,
) {
    frame.render_widget(
        Paragraph::new(lines)
            .block(toned_panel(title, signal))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_status_card(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    title: &str,
    headline: String,
    status: String,
) {
    let panel = toned_panel(title, &headline);
    let inner = panel.inner(area);
    frame.render_widget(panel, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(headline, semantic_style(&status))),
            Line::from(""),
            Line::from(Span::styled(status, muted_style())),
        ])
        .wrap(Wrap { trim: true }),
        inner,
    );
}

fn render_gauge_card(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    title: &str,
    ratio: f64,
    label: &str,
    status: &str,
) {
    let panel = toned_panel(title, status);
    let inner = panel.inner(area);
    let label_width = inner.width.saturating_sub(18).max(10) as usize;
    let status_width = inner.width.saturating_sub(24).max(8) as usize;
    frame.render_widget(panel, area);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(1),
        ])
        .split(inner);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(compact(label, label_width), title_style()),
            Span::raw("  "),
            Span::styled(compact(status, status_width), semantic_style(status)),
        ]))
        .wrap(Wrap { trim: true }),
        rows[0],
    );
    frame.render_widget(
        Gauge::default()
            .ratio(ratio.clamp(0.0, 1.0))
            .label(format!(
                "{:>3}%",
                (ratio.clamp(0.0, 1.0) * 100.0).round() as u32
            ))
            .gauge_style(
                Style::default()
                    .fg(tone_color(status))
                    .bg(Color::Rgb(27, 35, 44))
                    .add_modifier(Modifier::BOLD),
            ),
        rows[1],
    );
}

fn doctor_lines(report: &app::DoctorSnapshot) -> Vec<Line<'static>> {
    let probe = match &report.provider_probe {
        Ok(proxy) => format!("ok: {proxy}"),
        Err(error) => format!("error: {error}"),
    };

    vec![
        Line::from(format!("Версия: {}", report.app_version)),
        Line::from(format!("Платформа: {}", report.platform)),
        Line::from(format!("config.toml: {}", yes_no(report.config_exists))),
        Line::from(format!("state.json: {}", yes_no(report.state_exists))),
        Line::from(format!("watch.log: {}", yes_no(report.log_exists))),
        Line::from(format!(
            "tg:// handler: {}",
            if report.tg_protocol_handler.is_some() {
                "найден"
            } else {
                "не найден"
            }
        )),
        Line::from(format!(
            "Telegram Desktop: {}",
            report.telegram_executable.as_deref().unwrap_or("не найден")
        )),
        Line::from(format!(
            "Telegram запущен: {}",
            yes_no(report.telegram_running)
        )),
        Line::from(format!("Статус proxy: {}", report.current_proxy_status)),
        Line::from(format!("Статус источника: {}", report.source_status)),
        Line::from(format!("Статус backend: {}", report.backend_status)),
        Line::from(format!("Путь применения: {}", report.backend_route)),
        Line::from(format!(
            "Нужен перезапуск Telegram: {}",
            yes_no(report.backend_restart_required)
        )),
        Line::from(format!(
            "Автозапуск: {}",
            if report.autostart.installed {
                report
                    .autostart
                    .method
                    .as_ref()
                    .map(autostart_method_label)
                    .unwrap_or("unknown")
                    .to_string()
            } else {
                "нет".to_string()
            }
        )),
        Line::from(format!(
            "Источники: {}",
            if report.enabled_sources.is_empty() {
                "нет".to_string()
            } else {
                report.enabled_sources.join(", ")
            }
        )),
        Line::from(format!("Пробный fetch: {}", probe)),
    ]
}

fn section_tabs(section: ConsoleSection) -> Line<'static> {
    let sections = [
        (ConsoleSection::Dashboard, "Обзор"),
        (ConsoleSection::Actions, "Команды"),
        (ConsoleSection::Providers, "Источники"),
        (ConsoleSection::History, "История"),
    ];

    let mut spans = Vec::new();
    for (index, (candidate, label)) in sections.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("   "));
        }
        let style = if candidate == section {
            Style::default()
                .fg(Color::Rgb(11, 16, 22))
                .bg(Color::Rgb(109, 175, 255))
                .add_modifier(Modifier::BOLD)
        } else {
            muted_style()
        };
        spans.push(Span::styled(format!(" {} ", label), style));
    }
    Line::from(spans)
}

fn footer_hint(section: ConsoleSection) -> &'static str {
    match section {
        ConsoleSection::Dashboard => {
            "1-4 разделы  Tab/Left/Right  C команды  PgUp/PgDn сигналы  R/F5 обновить  Q выход"
        }
        ConsoleSection::Actions => {
            "Up/Down выбор  Enter запуск  S switch  W watcher  Z стоп  PgUp/PgDn сигналы  R/F5 обновить  Q выход"
        }
        ConsoleSection::Providers | ConsoleSection::History => {
            "1-4 разделы  Tab/Left/Right  C команды  PgUp/PgDn сигналы  R/F5 обновить  Q выход"
        }
    }
}

fn action_description_lines(
    action: ConsoleAction,
    snapshot: &UiSnapshot,
    paths: &AppPaths,
) -> Vec<Line<'static>> {
    action
        .description(snapshot, paths)
        .into_iter()
        .map(Line::from)
        .collect()
}

fn history_items(state: &AppState, width: u16) -> Vec<ListItem<'static>> {
    if state.recent_proxies.is_empty() {
        return vec![ListItem::new(Line::from(vec![
            Span::styled("• ", muted_style()),
            Span::styled("история пока пуста", muted_style()),
        ]))];
    }

    state
        .recent_proxies
        .iter()
        .take(10)
        .map(|record| {
            ListItem::new(vec![
                Line::from(vec![
                    Span::styled(
                        format!("{} ", record.proxy.protocol_label()),
                        protocol_style(record.proxy.kind),
                    ),
                    Span::styled(
                        compact_to_width(&record.proxy.short_label(), width, 10),
                        text_style(),
                    ),
                ]),
                Line::from(vec![
                    Span::styled(compact_to_width(&record.source, width, 18), muted_style()),
                    Span::raw("  "),
                    Span::styled(format_record_time(record.captured_at), muted_style()),
                ]),
            ])
        })
        .collect()
}

fn provider_source_items(snapshot: &UiSnapshot, width: u16) -> Vec<ListItem<'static>> {
    snapshot
        .config
        .provider
        .sources
        .iter()
        .map(|source| {
            let status = if source.enabled {
                if source.kind.is_socks5() && !snapshot.config.provider.enable_socks5_fallback {
                    "ждёт fallback"
                } else {
                    "активен"
                }
            } else {
                "выключен"
            };
            let protocol = if source.kind.is_socks5() {
                Span::styled(
                    format!("{} ", source.kind.label()),
                    Style::default()
                        .fg(Color::Rgb(247, 190, 103))
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(
                    format!("{} ", source.kind.label()),
                    Style::default()
                        .fg(Color::Rgb(109, 175, 255))
                        .add_modifier(Modifier::BOLD),
                )
            };
            ListItem::new(vec![
                Line::from(vec![
                    protocol,
                    Span::styled(compact_to_width(&source.name, width, 18), text_style()),
                    Span::raw("  "),
                    Span::styled(status, semantic_style(status)),
                ]),
                Line::from(Span::styled(
                    compact_to_width(&source.url, width, 6),
                    muted_style(),
                )),
            ])
        })
        .collect()
}

fn console_actions(snapshot: &UiSnapshot) -> Vec<ConsoleAction> {
    let mut actions = vec![
        ConsoleAction::Refresh,
        ConsoleAction::SwitchNow,
        ConsoleAction::Settings,
        ConsoleAction::WatchControl,
        ConsoleAction::ToggleAutostart,
        ConsoleAction::ToggleAutoCleanup,
        ConsoleAction::Doctor,
        ConsoleAction::OpenLog,
        ConsoleAction::OpenDataDir,
        ConsoleAction::ToggleSocks5Fallback,
        ConsoleAction::StopWatcher,
        ConsoleAction::StopAll,
        ConsoleAction::Exit,
    ];

    if snapshot.state.pending_proxy.is_some() && snapshot.state.watcher.telegram_running {
        actions.insert(2, ConsoleAction::ApplyPending);
    }

    if !(snapshot.autostart.installed
        || snapshot.config.autostart.enabled
        || snapshot.watcher_online)
    {
        actions.retain(|action| *action != ConsoleAction::StopWatcher);
    }

    actions
}

fn find_action(actions: &[ConsoleAction], needle: ConsoleAction) -> Option<ConsoleAction> {
    actions.iter().copied().find(|action| *action == needle)
}

fn kv_line(label: &str, value: String, _width: u16) -> Line<'static> {
    let label_width = label.chars().count().clamp(6, 10);
    Line::from(vec![
        Span::styled(format!("{label:<label_width$}"), muted_style()),
        Span::styled(value, text_style()),
    ])
}

fn kv_lines(label: &str, value: String, width: u16) -> Vec<Line<'static>> {
    let label_width = label.chars().count().clamp(6, 10);
    let value_width = width.saturating_sub(label_width as u16 + 3).max(12) as usize;
    let wrapped = wrap_value_lines(&value, value_width, 2);
    let indent = " ".repeat(label_width);

    wrapped
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            Line::from(vec![
                Span::styled(
                    if index == 0 {
                        format!("{label:<label_width$}")
                    } else {
                        indent.clone()
                    },
                    muted_style(),
                ),
                Span::styled(chunk, text_style()),
            ])
        })
        .collect()
}

fn compact_to_width(value: &str, width: u16, reserve: u16) -> String {
    compact(value, width.saturating_sub(reserve).max(10) as usize)
}

fn compact(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }

    let head = max.saturating_sub(5) / 2;
    let tail = max.saturating_sub(5) - head;
    let prefix = value.chars().take(head).collect::<String>();
    let suffix = value
        .chars()
        .rev()
        .take(tail)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{prefix} ... {suffix}")
}

fn wrap_value_lines(value: &str, width: usize, max_lines: usize) -> Vec<String> {
    if max_lines == 0 || width == 0 {
        return Vec::new();
    }
    if value.chars().count() <= width {
        return vec![value.to_string()];
    }

    let mut lines = Vec::new();
    let mut remaining = value.trim();
    while !remaining.is_empty() && lines.len() + 1 < max_lines {
        let mut current = String::new();
        let mut consumed = 0usize;
        for (index, token) in remaining.split_whitespace().enumerate() {
            let token_len = token.chars().count();
            let next_len = if index == 0 {
                token_len
            } else {
                current.chars().count() + 1 + token_len
            };
            if next_len > width {
                break;
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(token);
            consumed += token.len();
            if index + 1 < remaining.split_whitespace().count() {
                consumed += 1;
            }
        }

        if current.is_empty() {
            current = remaining.chars().take(width).collect::<String>();
            consumed = current.len();
        }

        lines.push(current.trim().to_string());
        remaining = remaining.get(consumed..).unwrap_or_default().trim_start();
    }

    if !remaining.is_empty() {
        lines.push(compact(remaining, width));
    }

    lines
}

fn yes_no(value: bool) -> &'static str {
    if value { "да" } else { "нет" }
}

fn mode_label(mode: &WatcherMode) -> &'static str {
    match mode {
        WatcherMode::Idle => "idle",
        WatcherMode::Watching => "watching",
        WatcherMode::WaitingForTelegram => "waiting",
        WatcherMode::Switching => "switching",
        WatcherMode::Error => "error",
    }
}

fn mode_label_ru(mode: &WatcherMode) -> &'static str {
    match mode {
        WatcherMode::Idle => "спокоен",
        WatcherMode::Watching => "наблюдает",
        WatcherMode::WaitingForTelegram => "ждёт Telegram",
        WatcherMode::Switching => "переключает",
        WatcherMode::Error => "ошибка",
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

fn format_time(value: Option<&chrono::DateTime<chrono::Utc>>) -> String {
    value
        .map(|entry| {
            entry
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "нет данных".to_string())
}

fn format_record_time(value: chrono::DateTime<chrono::Utc>) -> String {
    value
        .with_timezone(&chrono::Local)
        .format("%H:%M:%S")
        .to_string()
}

fn tone_color(value: &str) -> Color {
    let lower = value.to_lowercase();
    if lower.contains("работ")
        || lower.contains("актив")
        || lower.contains("подключ")
        || lower.contains("доступ")
        || lower.contains("online")
        || lower.contains("ok")
        || lower.contains("active")
        || lower.contains("enabled")
        || lower.contains("ready")
        || lower.contains("включ")
    {
        return Color::Rgb(118, 201, 160);
    }
    if lower.contains("pending")
        || lower.contains("wait")
        || lower.contains("checking")
        || lower.contains("manual")
        || lower.contains("idle")
        || lower.contains("источник пуст")
        || lower.contains("ожид")
        || lower.contains("перезапуск")
        || lower.contains("paused")
        || lower.contains("fallback")
        || lower.contains("switch")
    {
        return Color::Rgb(247, 190, 103);
    }
    if lower.contains("нет")
        || lower.contains("не ")
        || lower.contains("error")
        || lower.contains("off")
        || lower.contains("откл")
        || lower.contains("reject")
        || lower.contains("unavailable")
        || lower.contains("dead")
        || lower.contains("сбой")
    {
        return Color::Rgb(229, 118, 118);
    }
    Color::Rgb(109, 175, 255)
}

fn semantic_style(value: &str) -> Style {
    Style::default()
        .fg(tone_color(value))
        .add_modifier(Modifier::BOLD)
}

fn protocol_style(kind: ProxyKind) -> Style {
    match kind {
        ProxyKind::MtProto => Style::default()
            .fg(Color::Rgb(109, 175, 255))
            .add_modifier(Modifier::BOLD),
        ProxyKind::Socks5 => Style::default()
            .fg(Color::Rgb(247, 190, 103))
            .add_modifier(Modifier::BOLD),
    }
}

fn panel(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(52, 66, 81)))
        .title(title.to_string())
}

fn toned_panel(title: &str, signal: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(tone_color(signal)))
        .title(title.to_string())
}

fn title_style() -> Style {
    Style::default()
        .fg(Color::Rgb(239, 244, 250))
        .add_modifier(Modifier::BOLD)
}

fn text_style() -> Style {
    Style::default().fg(Color::Rgb(214, 224, 235))
}

fn muted_style() -> Style {
    Style::default().fg(Color::Rgb(133, 149, 165))
}

fn selected_style() -> Style {
    Style::default()
        .fg(Color::Rgb(109, 175, 255))
        .add_modifier(Modifier::BOLD)
}

fn value_style(selected: bool, alert: bool) -> Style {
    if alert {
        return semantic_style("error");
    }
    if selected {
        selected_style()
    } else {
        Style::default().fg(Color::Rgb(180, 214, 248))
    }
}

fn badge(text: impl Into<String>, background: Color) -> Span<'static> {
    Span::styled(
        text.into(),
        Style::default()
            .fg(Color::Rgb(11, 16, 22))
            .bg(background)
            .add_modifier(Modifier::BOLD),
    )
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TerminalSession {
    fn new() -> anyhow::Result<Self> {
        enable_raw_mode().context("Не удалось включить raw mode")?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen).context("Не удалось открыть alternate screen")?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend).context("Не удалось создать TUI-терминал")?;
        terminal
            .clear()
            .context("Не удалось очистить экран терминала")?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

#[derive(Clone)]
struct SetupDraft {
    original: AppConfig,
    focus: usize,
    check_interval_secs: u64,
    connect_timeout_secs: u64,
    failure_threshold: u32,
    history_size: usize,
    autostart_enabled: bool,
    auto_cleanup_dead_proxies: bool,
    socks5_fallback: bool,
}

impl SetupDraft {
    fn from(config: AppConfig) -> Self {
        Self {
            original: config.clone(),
            focus: 0,
            check_interval_secs: config.watcher.check_interval_secs,
            connect_timeout_secs: config.watcher.connect_timeout_secs,
            failure_threshold: config.watcher.failure_threshold,
            history_size: config.watcher.history_size,
            autostart_enabled: config.autostart.enabled,
            auto_cleanup_dead_proxies: config.watcher.auto_cleanup_dead_proxies,
            socks5_fallback: config.provider.enable_socks5_fallback,
        }
    }

    fn adjust(&mut self, increase: bool) {
        match self.focus {
            0 => adjust_u64(&mut self.check_interval_secs, increase, 5, 300, 5),
            1 => adjust_u64(&mut self.connect_timeout_secs, increase, 1, 30, 1),
            2 => adjust_u32(&mut self.failure_threshold, increase, 1, 10, 1),
            3 => adjust_usize(&mut self.history_size, increase, 1, 20, 1),
            4 => self.autostart_enabled = !self.autostart_enabled,
            5 => self.auto_cleanup_dead_proxies = !self.auto_cleanup_dead_proxies,
            6 => self.socks5_fallback = !self.socks5_fallback,
            _ => {}
        }
    }

    fn into_config(self) -> AppConfig {
        let mut config = self.original;
        config.watcher.check_interval_secs = self.check_interval_secs;
        config.watcher.connect_timeout_secs = self.connect_timeout_secs;
        config.watcher.failure_threshold = self.failure_threshold;
        config.watcher.history_size = self.history_size;
        config.watcher.auto_cleanup_dead_proxies = self.auto_cleanup_dead_proxies;
        config.autostart.enabled = self.autostart_enabled;
        config.provider.enable_socks5_fallback = self.socks5_fallback;
        config
    }

    fn provider_pool(&self) -> String {
        let mut config = self.original.clone();
        config.provider.enable_socks5_fallback = self.socks5_fallback;
        app::provider_pool_summary(&config)
    }

    fn enabled_sources(&self) -> String {
        let mut config = self.original.clone();
        config.provider.enable_socks5_fallback = self.socks5_fallback;
        app::enabled_sources_summary(&config)
    }

    fn fields(&self) -> [SetupField; 7] {
        [
            SetupField {
                label: "Интервал проверки",
                value: format!("{} сек", self.check_interval_secs),
                description: "Как часто watcher проверяет текущий proxy и ищет замену при деградации.",
            },
            SetupField {
                label: "TCP timeout",
                value: format!("{} сек", self.connect_timeout_secs),
                description: "Сколько ждать ответа от сервера в локальном TCP health-check.",
            },
            SetupField {
                label: "Порог сбоев",
                value: self.failure_threshold.to_string(),
                description: "После какого количества подряд неудачных проверок начинать ротацию proxy.",
            },
            SetupField {
                label: "История proxy",
                value: self.history_size.to_string(),
                description: "Сколько последних proxy хранить, чтобы не вернуться мгновенно на тот же адрес.",
            },
            SetupField {
                label: "Автозапуск watcher",
                value: if self.autostart_enabled {
                    "включён".to_string()
                } else {
                    "выключен".to_string()
                },
                description: "Запускать headless watcher при логине Windows.",
            },
            SetupField {
                label: "Автоподчистка",
                value: if self.auto_cleanup_dead_proxies {
                    "включена".to_string()
                } else {
                    "выключена".to_string()
                },
                description: "Автоматически удалять из Telegram proxy, которые ProtoSwitch считает мёртвыми.",
            },
            SetupField {
                label: "SOCKS5 fallback",
                value: if self.socks5_fallback {
                    "включён".to_string()
                } else {
                    "выключен".to_string()
                },
                description: "Разрешить fallback на бесплатные SOCKS5-источники, когда MTProto-ленты пусты или деградировали.",
            },
        ]
    }

    fn current_field(&self) -> SetupField {
        self.fields()[self.focus].clone()
    }
}

#[derive(Clone)]
struct SetupField {
    label: &'static str,
    value: String,
    description: &'static str,
}

#[derive(Clone)]
struct UiSnapshot {
    config: AppConfig,
    state: AppState,
    autostart: platform::AutostartStatus,
    watcher_online: bool,
    paths: AppPaths,
}

impl UiSnapshot {
    fn load(paths: &AppPaths) -> anyhow::Result<Self> {
        let (config, state, autostart) = app::load_status_snapshot(paths)?;
        let watcher_online =
            app::watcher_process_exists() || app::watcher_is_recent(&config, &state);
        Ok(Self {
            config,
            state,
            autostart,
            watcher_online,
            paths: paths.clone(),
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConsoleSection {
    Dashboard,
    Actions,
    Providers,
    History,
}

impl ConsoleSection {
    fn next(self) -> Self {
        match self {
            ConsoleSection::Dashboard => ConsoleSection::Actions,
            ConsoleSection::Actions => ConsoleSection::Providers,
            ConsoleSection::Providers => ConsoleSection::History,
            ConsoleSection::History => ConsoleSection::Dashboard,
        }
    }

    fn prev(self) -> Self {
        match self {
            ConsoleSection::Dashboard => ConsoleSection::History,
            ConsoleSection::Actions => ConsoleSection::Dashboard,
            ConsoleSection::Providers => ConsoleSection::Actions,
            ConsoleSection::History => ConsoleSection::Providers,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConsoleAction {
    SwitchNow,
    ApplyPending,
    WatchControl,
    StopWatcher,
    StopAll,
    ToggleAutostart,
    ToggleAutoCleanup,
    ToggleSocks5Fallback,
    Settings,
    Doctor,
    OpenLog,
    OpenDataDir,
    Refresh,
    Exit,
}

impl ConsoleAction {
    fn label(&self, snapshot: &UiSnapshot) -> &'static str {
        match self {
            ConsoleAction::SwitchNow => "найти новый proxy сейчас",
            ConsoleAction::ApplyPending => "применить уже найденный proxy",
            ConsoleAction::WatchControl => {
                if snapshot.watcher_online {
                    "авторежим: перезапустить watcher"
                } else {
                    "авторежим: запустить watcher"
                }
            }
            ConsoleAction::StopWatcher => "остановить только watcher",
            ConsoleAction::StopAll => "полностью остановить ProtoSwitch",
            ConsoleAction::ToggleAutostart => {
                if snapshot.autostart.installed || snapshot.config.autostart.enabled {
                    "автозапуск: выключить"
                } else {
                    "автозапуск: включить"
                }
            }
            ConsoleAction::ToggleAutoCleanup => {
                if snapshot.config.watcher.auto_cleanup_dead_proxies {
                    "автоподчистка: выключить"
                } else {
                    "автоподчистка: включить"
                }
            }
            ConsoleAction::ToggleSocks5Fallback => {
                if snapshot.config.provider.enable_socks5_fallback {
                    "источники: только MTProto"
                } else {
                    "источники: добавить SOCKS5"
                }
            }
            ConsoleAction::Settings => "быстрые настройки",
            ConsoleAction::Doctor => "диагностика",
            ConsoleAction::OpenLog => "открыть лог",
            ConsoleAction::OpenDataDir => "открыть папку данных",
            ConsoleAction::Refresh => "обновить состояние",
            ConsoleAction::Exit => "выход",
        }
    }

    fn shortcut(&self) -> &'static str {
        match self {
            ConsoleAction::SwitchNow => "[S]",
            ConsoleAction::ApplyPending => "[P]",
            ConsoleAction::WatchControl => "[W]",
            ConsoleAction::StopWatcher => "[X]",
            ConsoleAction::StopAll => "[Z]",
            ConsoleAction::ToggleAutostart => "[A]",
            ConsoleAction::ToggleAutoCleanup => "[K]",
            ConsoleAction::ToggleSocks5Fallback => "[F]",
            ConsoleAction::Settings => "[E]",
            ConsoleAction::Doctor => "[D]",
            ConsoleAction::OpenLog => "[L]",
            ConsoleAction::OpenDataDir => "[O]",
            ConsoleAction::Refresh => "[R]",
            ConsoleAction::Exit => "[Q]",
        }
    }

    fn description(&self, snapshot: &UiSnapshot, paths: &AppPaths) -> Vec<String> {
        match self {
            ConsoleAction::SwitchNow => vec![
                "Сразу запросить нового кандидата из встроенного пула источников и сохранить его в Telegram.".to_string(),
                "Если Telegram уже открыт, ProtoSwitch старается не мешать работе и использует тихий managed-flow.".to_string(),
            ],
            ConsoleAction::ApplyPending => vec![
                "Применить уже сохранённый pending proxy без нового fetch.".to_string(),
                "Подходит, когда replacement найден заранее, а Telegram запущен только сейчас.".to_string(),
            ],
            ConsoleAction::WatchControl => vec![if snapshot.watcher_online {
                "Перезапустить headless watcher с актуальным конфигом и свежим состоянием.".to_string()
            } else {
                "Поднять headless watcher в фоне без перезапуска интерфейса.".to_string()
            }],
            ConsoleAction::StopWatcher => {
                vec!["Остановить только фоновые headless watcher-процессы ProtoSwitch.".to_string()]
            }
            ConsoleAction::StopAll => vec![
                "Остановить все остальные процессы ProtoSwitch и закрыть текущую консоль.".to_string(),
                "Подходит, если нужно полностью остановить приложение перед переустановкой или ручной проверкой.".to_string(),
            ],
            ConsoleAction::ToggleAutostart => vec![if snapshot.autostart.installed
                || snapshot.config.autostart.enabled
            {
                "Отключить запуск ProtoSwitch при логине Windows.".to_string()
            } else {
                "Включить запуск ProtoSwitch при логине Windows.".to_string()
            }],
            ConsoleAction::ToggleAutoCleanup => vec![if snapshot.config.watcher.auto_cleanup_dead_proxies {
                "Отключить автоматическое удаление мёртвых proxy из управляемого списка Telegram.".to_string()
            } else {
                "Включить автоматическое удаление мёртвых proxy после apply и при повторной проверке.".to_string()
            }],
            ConsoleAction::ToggleSocks5Fallback => vec![if snapshot.config.provider.enable_socks5_fallback {
                "Оставить только MTProto-источники и временно выключить SOCKS5 fallback.".to_string()
            } else {
                "Разрешить бесплатные SOCKS5-источники как запасной путь, когда MTProto-ленты пусты или деградируют.".to_string()
            }],
            ConsoleAction::Settings => vec![
                "Изменить интервалы watcher, timeout, порог сбоев, историю, автозапуск, автоподчистку и SOCKS5 fallback.".to_string(),
            ],
            ConsoleAction::Doctor => vec![
                "Проверить tg:// handler, Telegram Desktop, provider pool, state/config/logs и автозапуск.".to_string(),
                "Диагностика идёт в фоне, поэтому интерфейс не зависает во время проверки.".to_string(),
            ],
            ConsoleAction::OpenLog => vec![format!("Открыть watch.log: {}", paths.log_file.display())],
            ConsoleAction::OpenDataDir => {
                vec![format!("Открыть рабочую папку: {}", paths.local_dir.display())]
            }
            ConsoleAction::Refresh => vec![
                "Перечитать status snapshot без побочных действий.".to_string(),
                "Полезно, если хочется быстро перепроверить экран после фонового изменения состояния.".to_string(),
            ],
            ConsoleAction::Exit => vec![
                format!(
                    "Закрыть интерфейс. Watcher {}.",
                    if snapshot.watcher_online {
                        "продолжит работу в фоне"
                    } else {
                        "останется выключенным"
                    }
                ),
                format!(
                    "Текущий статус proxy: {}",
                    app::current_proxy_status_text(&snapshot.state)
                ),
            ],
        }
    }
}

struct ConsoleState {
    section: ConsoleSection,
    focus: usize,
    activity: Vec<String>,
    activity_scroll: usize,
    inspector_title: String,
    inspector_lines: Vec<Line<'static>>,
    last_seen_error: Option<String>,
    force_refresh: bool,
    doctor_job: Option<Receiver<Result<app::DoctorSnapshot, String>>>,
}

impl ConsoleState {
    fn push_activity(&mut self, message: String) {
        self.activity_scroll = 0;
        self.activity.insert(0, compact(&message, 96));
        while self.activity.len() > 24 {
            self.activity.pop();
        }
    }

    fn set_result(&mut self, title: &str, lines: Vec<String>) {
        self.inspector_title = title.to_string();
        self.inspector_lines = lines.iter().cloned().map(Line::from).collect();
        if let Some(first) = lines.first() {
            self.push_activity(first.clone());
        }
    }

    fn set_inspector(&mut self, title: &str, lines: Vec<Line<'static>>) {
        self.inspector_title = title.to_string();
        self.inspector_lines = lines;
        self.push_activity(format!("{title}: снимок обновлён"));
    }

    fn clear_inspector(&mut self) {
        self.inspector_title = "Контекст".to_string();
        self.inspector_lines.clear();
    }

    fn start_doctor(&mut self, paths: &AppPaths) {
        if self.doctor_job.is_some() {
            self.push_activity("doctor: диагностика уже выполняется".to_string());
            return;
        }

        let (sender, receiver) = mpsc::channel();
        let job_paths = paths.clone();
        self.doctor_job = Some(receiver);
        self.inspector_title = "Диагностика".to_string();
        self.inspector_lines = vec![
            Line::from("Диагностика выполняется в фоне."),
            Line::from("Можно переключать view и обновлять экран."),
        ];
        self.push_activity("doctor: диагностика запущена".to_string());
        thread::spawn(move || {
            let _ =
                sender.send(app::doctor_snapshot(&job_paths).map_err(|error| error.to_string()));
        });
    }

    fn poll_background_tasks(&mut self) {
        let Some(receiver) = self.doctor_job.as_ref() else {
            return;
        };

        match receiver.try_recv() {
            Ok(Ok(report)) => {
                self.doctor_job = None;
                self.set_inspector("Диагностика", doctor_lines(&report));
            }
            Ok(Err(error)) => {
                self.doctor_job = None;
                self.push_activity(format!("doctor: {error}"));
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.doctor_job = None;
                self.push_activity("doctor: фоновая задача прервалась".to_string());
            }
        }
    }

    fn sync_error(&mut self, error: &Option<String>) {
        if self.last_seen_error != *error {
            if let Some(value) = error {
                self.push_activity(format!("error: {value}"));
            }
            self.last_seen_error = error.clone();
        }
    }

    fn activity_lines(&self, width: u16) -> Vec<Line<'static>> {
        if self.activity.is_empty() {
            return vec![
                Line::from("Интерфейс готов."),
                Line::from("Здесь появляются результаты команд и сигналы watcher."),
            ];
        }

        self.activity
            .iter()
            .map(|entry| Line::from(compact_to_width(entry, width, 4)))
            .collect()
    }

    fn activity_lines_for_area(&self, width: u16, height: u16) -> Vec<Line<'static>> {
        if self.activity.is_empty() {
            return self.activity_lines(width);
        }

        let visible = height.max(2) as usize;
        let max_scroll = self.activity.len().saturating_sub(visible);
        let start = self.activity_scroll.min(max_scroll);

        self.activity
            .iter()
            .skip(start)
            .take(visible)
            .map(|entry| Line::from(compact_to_width(entry, width, 4)))
            .collect()
    }

    fn scroll_activity_up(&mut self) {
        self.activity_scroll = self.activity_scroll.saturating_sub(1);
    }

    fn scroll_activity_down(&mut self) {
        if self.activity.is_empty() {
            return;
        }
        self.activity_scroll = self.activity_scroll.saturating_add(1);
    }
}

impl Default for ConsoleState {
    fn default() -> Self {
        Self {
            section: ConsoleSection::Dashboard,
            focus: 0,
            activity: Vec::new(),
            activity_scroll: 0,
            inspector_title: "Контекст".to_string(),
            inspector_lines: Vec::new(),
            last_seen_error: None,
            force_refresh: false,
            doctor_job: None,
        }
    }
}

fn adjust_u64(value: &mut u64, increase: bool, min: u64, max: u64, step: u64) {
    if increase {
        *value = (*value + step).min(max);
    } else {
        *value = value.saturating_sub(step).max(min);
    }
}

fn adjust_u32(value: &mut u32, increase: bool, min: u32, max: u32, step: u32) {
    if increase {
        *value = (*value + step).min(max);
    } else {
        *value = value.saturating_sub(step).max(min);
    }
}

fn adjust_usize(value: &mut usize, increase: bool, min: usize, max: usize, step: usize) {
    if increase {
        *value = (*value + step).min(max);
    } else {
        *value = value.saturating_sub(step).max(min);
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use tempfile::tempdir;

    use super::*;
    use crate::model::{ProxyRecord, TelegramBackendMode, TelegramProxy, WatcherSnapshot};

    #[test]
    fn renders_narrow_dashboard_with_backend_panel() {
        let output = render_console_text(90, 28, ConsoleSection::Dashboard);
        assert!(output.contains("Сейчас"));
        assert!(output.contains("Telegram"));
        assert!(output.contains("Источники"));
    }

    #[test]
    fn renders_regular_providers_with_feed_list() {
        let output = render_console_text(120, 34, ConsoleSection::Providers);
        assert!(output.contains("Резерв"));
        assert!(output.contains("Ленты"));
        assert!(output.contains("Политика"));
    }

    #[test]
    fn renders_wide_history_with_timeline_panel() {
        let output = render_console_text(160, 40, ConsoleSection::History);
        assert!(output.contains("Хронология"));
        assert!(output.contains("Последние proxy"));
        assert!(output.contains("Сигналы"));
    }

    #[test]
    fn activity_window_supports_scrolling() {
        let mut console = ConsoleState::default();
        for index in 0..12 {
            console.push_activity(format!("signal-{index}"));
        }

        let first = console
            .activity_lines_for_area(60, 4)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        console.scroll_activity_down();
        console.scroll_activity_down();
        let second = console
            .activity_lines_for_area(60, 4)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert_ne!(first, second);
        assert!(first.contains("signal-11"));
    }

    #[test]
    fn wraps_long_status_values_without_dropping_context() {
        let lines = wrap_value_lines(
            "источник пуст: нет свободных proxy, ждём следующую проверку и сохраняем managed replacement",
            28,
            2,
        );

        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("источник пуст"));
        assert!(!lines[1].is_empty());
        assert!(lines.join(" ").contains("proxy"));
    }

    fn render_console_text(width: u16, height: u16, section: ConsoleSection) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        let snapshot = sample_snapshot();
        let mut console = ConsoleState {
            section,
            ..ConsoleState::default()
        };
        console.push_activity("signal-one".to_string());
        console.push_activity("signal-two".to_string());
        console.push_activity(
            "very long signal about pending managed restart and provider fallback".to_string(),
        );
        let actions = console_actions(&snapshot);

        terminal
            .draw(|frame| render_console(frame, &snapshot.paths, &snapshot, &console, &actions))
            .unwrap();

        buffer_to_string(terminal.backend().buffer())
    }

    fn buffer_to_string(buffer: &ratatui::buffer::Buffer) -> String {
        let area = buffer.area;
        let mut lines = Vec::new();
        for y in 0..area.height {
            let mut line = String::new();
            for x in 0..area.width {
                line.push_str(buffer[(x, y)].symbol());
            }
            lines.push(line);
        }
        lines.join("\n")
    }

    fn sample_snapshot() -> UiSnapshot {
        let root = tempdir().unwrap();
        let paths = AppPaths::from_base_dirs(root.path().join("config"), root.path().join("data"));
        let mut config = AppConfig::default();
        config.telegram.backend_mode = TelegramBackendMode::Managed;
        let current = ProxyRecord::new(
            TelegramProxy::mtproto(
                "ovh.pl.1.mtproto.ru",
                443,
                "ee211122223333444455556666777788",
            ),
            "mtproto.ru",
        );
        let pending = ProxyRecord::new(
            TelegramProxy::socks5("185.3.200.185", 2053, None, None),
            "proxifly",
        );
        let now = Utc::now();
        let state = AppState {
            current_proxy: Some(current),
            pending_proxy: Some(pending),
            current_proxy_status: "proxy сохранён, ждёт перезапуска Telegram".to_string(),
            source_status: "источник временно пуст, используем сохранённый managed proxy"
                .to_string(),
            backend_status: "managed enabled / selected proxy".to_string(),
            backend_route:
                "pending until restart / C:\\Users\\tester\\AppData\\Roaming\\Telegram Desktop\\tdata\\settingss"
                    .to_string(),
            backend_restart_required: true,
            last_fetch_at: Some(now),
            last_apply_at: Some(now),
            watcher: WatcherSnapshot {
                mode: WatcherMode::WaitingForTelegram,
                failure_streak: 2,
                telegram_running: true,
                last_check_at: Some(now),
                next_check_at: Some(now),
            },
            ..AppState::default()
        };

        UiSnapshot {
            config,
            state,
            autostart: platform::AutostartStatus {
                installed: true,
                method: Some(AutostartMethod::StartupFolder),
                target: Some("ProtoSwitch".to_string()),
            },
            watcher_online: true,
            paths,
        }
    }

    #[test]
    fn renders_session_header_with_human_summary() {
        let rendered = render_console_text(120, 34, ConsoleSection::Dashboard);

        assert!(rendered.contains("Итог"));
        assert!(rendered.contains("Фон"));
        assert!(rendered.contains("сохранён тихо") || rendered.contains("перезапуска Telegram"));
    }

    #[test]
    fn renders_regular_dashboard_with_comfort_panel() {
        let rendered = render_console_text(120, 34, ConsoleSection::Dashboard);

        assert!(rendered.contains("Тихий режим"));
        assert!(rendered.contains("Дальше"));
        assert!(rendered.contains("Автозап."));
    }

    #[test]
    fn prioritizes_daily_console_actions() {
        let actions = console_actions(&sample_snapshot());

        assert_eq!(actions[0], ConsoleAction::Refresh);
        assert_eq!(actions[1], ConsoleAction::SwitchNow);
        assert_eq!(actions[2], ConsoleAction::ApplyPending);
        assert!(
            actions
                .iter()
                .position(|action| *action == ConsoleAction::Doctor)
                .unwrap()
                < actions
                    .iter()
                    .position(|action| *action == ConsoleAction::StopAll)
                    .unwrap()
        );
    }
}
