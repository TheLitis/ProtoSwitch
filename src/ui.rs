use std::io::{self, IsTerminal};
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
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Wrap};

use crate::APP_VERSION;
use crate::app;
use crate::model::{AppConfig, AppState, AutostartMethod, WatcherMode};
use crate::paths::AppPaths;
use crate::windows;

pub fn stdout_is_terminal() -> bool {
    io::stdout().is_terminal()
}

pub fn run_setup(config: AppConfig) -> anyhow::Result<AppConfig> {
    let mut session = TerminalSession::new()?;
    let mut draft = SetupDraft::from(config);

    loop {
        session.terminal.draw(|frame| render_setup(frame, &draft))?;

        if let Event::Key(key) = event::read().context("Не удалось прочитать клавиатуру")? {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            match key.code {
                KeyCode::Up => draft.focus = draft.focus.saturating_sub(1),
                KeyCode::Down => draft.focus = (draft.focus + 1).min(5),
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
        if console.force_refresh || last_refresh.elapsed() >= Duration::from_millis(650) {
            snapshot = UiSnapshot::load(paths)?;
            console.sync_error(&snapshot.state.last_error);
            console.force_refresh = false;
            last_refresh = Instant::now();
        }

        let actions = console_actions(&snapshot);
        if console.section == ConsoleSection::Control {
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
            KeyCode::Left => {
                console.section = console.section.prev();
                continue;
            }
            KeyCode::Right | KeyCode::Tab => {
                console.section = console.section.next();
                continue;
            }
            _ => {}
        }

        if console.section != ConsoleSection::Control {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => return Ok(()),
                KeyCode::Char('r') => {
                    console.force_refresh = true;
                    console.set_result("Refresh", vec!["Данные перечитаны из config/state.".to_string()]);
                }
                KeyCode::Char('c') => {
                    console.section = ConsoleSection::Control;
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
            KeyCode::Char('a') => find_action(&actions, ConsoleAction::ToggleAutostart),
            KeyCode::Char('k') => find_action(&actions, ConsoleAction::ToggleAutoCleanup),
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
            ConsoleAction::ToggleAutostart => {
                let enable =
                    !(snapshot.autostart.installed || snapshot.config.autostart.enabled);
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
                        console.set_result("Autoclean", vec![message]);
                    }
                    Err(error) => console.push_activity(format!("autoclean: {error}")),
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
            ConsoleAction::Doctor => match app::doctor_snapshot(paths) {
                Ok(report) => console.set_inspector("Doctor", doctor_lines(&report)),
                Err(error) => console.push_activity(format!("doctor: {error}")),
            },
            ConsoleAction::OpenLog => match app::open_in_notepad(&paths.log_file) {
                Ok(_) => console.set_result("Log", vec![format!("Открыт {}", paths.log_file.display())]),
                Err(error) => console.push_activity(format!("log: {error}")),
            },
            ConsoleAction::OpenDataDir => match app::open_in_shell(&paths.local_dir) {
                Ok(_) => {
                    console.set_result("Data", vec![format!("Открыта {}", paths.local_dir.display())])
                }
                Err(error) => console.push_activity(format!("data: {error}")),
            },
            ConsoleAction::Refresh => {
                console.force_refresh = true;
                console.set_result("Refresh", vec!["Снимок обновлён.".to_string()]);
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
        .constraints([Constraint::Percentage(54), Constraint::Percentage(46)])
        .split(vertical[1]);

    let title = Paragraph::new(Line::from(vec![
        Span::styled("ProtoSwitch", title_style()),
        Span::raw(" "),
        Span::styled(APP_VERSION, muted_style()),
        Span::raw("  "),
        badge("setup", Color::Rgb(118, 201, 160)),
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
            let value = Span::styled(field.value, value_style(selected, false));
            ListItem::new(Line::from(vec![
                Span::styled(marker, if selected { selected_style() } else { muted_style() }),
                Span::styled(format!("{:<24}", field.label), if selected { selected_style() } else { text_style() }),
                value,
            ]))
        })
        .collect::<Vec<_>>();
    frame.render_widget(List::new(fields).block(panel("Параметры")), body[0]);

    let active = draft.current_field();
    let info = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(active.label, title_style()),
            Span::raw("  "),
            Span::styled(active.value, value_style(true, false)),
        ]),
        Line::from(""),
        Line::from(active.description),
        Line::from(""),
        Line::from(format!("Проверка: {} сек", draft.check_interval_secs)),
        Line::from(format!("TCP timeout: {} сек", draft.connect_timeout_secs)),
        Line::from(format!("Порог сбоев: {}", draft.failure_threshold)),
        Line::from(format!("История proxy: {}", draft.history_size)),
        Line::from(format!(
            "Автозапуск: {}",
            if draft.autostart_enabled { "вкл" } else { "выкл" }
        )),
        Line::from(format!(
            "Автоподчистка: {}",
            if draft.auto_cleanup_dead_proxies {
                "вкл"
            } else {
                "выкл"
            }
        )),
    ])
    .block(panel("Контекст"))
    .wrap(Wrap { trim: true });
    frame.render_widget(info, body[1]);

    let footer = Paragraph::new("↑/↓ поле  ←/→ изменить  Enter сохранить  Esc выйти")
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
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(16),
            Constraint::Length(3),
        ])
        .split(area);

    let header = Paragraph::new(Line::from(vec![
        Span::styled("ProtoSwitch", title_style()),
        Span::raw(" "),
        Span::styled(APP_VERSION, muted_style()),
        Span::raw("  "),
        badge(
            if snapshot.watcher_online { "watcher on" } else { "watcher idle" },
            if snapshot.watcher_online {
                Color::Rgb(118, 201, 160)
            } else {
                Color::Rgb(247, 190, 103)
            },
        ),
        Span::raw(" "),
        badge(
            if snapshot.state.watcher.telegram_running {
                "telegram on"
            } else {
                "telegram off"
            },
            if snapshot.state.watcher.telegram_running {
                Color::Rgb(109, 175, 255)
            } else {
                Color::Rgb(229, 118, 118)
            },
        ),
        Span::raw(" "),
        badge(
            if snapshot.config.watcher.auto_cleanup_dead_proxies {
                "autoclean"
            } else {
                "manual clean"
            },
            if snapshot.config.watcher.auto_cleanup_dead_proxies {
                Color::Rgb(118, 201, 160)
            } else {
                Color::Rgb(247, 190, 103)
            },
        ),
    ]))
    .block(panel("Session"));
    frame.render_widget(header, vertical[0]);

    frame.render_widget(
        Paragraph::new(section_tabs(console.section))
            .block(panel("Views"))
            .wrap(Wrap { trim: true }),
        vertical[1],
    );

    match console.section {
        ConsoleSection::Overview => render_overview(frame, vertical[2], snapshot, console),
        ConsoleSection::Control => render_control(frame, vertical[2], paths, snapshot, console, actions),
        ConsoleSection::History => render_history(frame, vertical[2], snapshot, console),
    }

    let footer = Paragraph::new(
        "←/→ разделы  ↑/↓ выбор  Enter действие  S switch  P pending  W watcher  A autostart  K autoclean  R refresh  Q выйти",
    )
    .style(muted_style())
    .block(panel("Keys"));
    frame.render_widget(footer, vertical[3]);
}

fn render_overview(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    snapshot: &UiSnapshot,
    console: &ConsoleState,
) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Length(8), Constraint::Min(7)])
        .split(area);

    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(34), Constraint::Percentage(33), Constraint::Percentage(33)])
        .split(rows[0]);

    let middle = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(34), Constraint::Percentage(33), Constraint::Percentage(33)])
        .split(rows[1]);

    render_status_card(
        frame,
        top[0],
        "Proxy",
        snapshot
            .state
            .current_proxy
            .as_ref()
            .map(|record| record.proxy.short_label())
            .unwrap_or_else(|| "не выбран".to_string()),
        app::current_proxy_status_text(&snapshot.state),
    );

    render_status_card(
        frame,
        top[1],
        "Source",
        compact(&snapshot.config.provider.source_url, 28),
        app::source_status_text(&snapshot.state),
    );

    render_status_card(
        frame,
        top[2],
        "Pending",
        snapshot
            .state
            .pending_proxy
            .as_ref()
            .map(|record| record.proxy.short_label())
            .unwrap_or_else(|| "нет".to_string()),
        if snapshot.state.pending_proxy.is_some() {
            "ожидает применения".to_string()
        } else {
            "очередь пуста".to_string()
        },
    );

    render_status_card(
        frame,
        middle[0],
        "Watcher",
        format!(
            "{} / fail {}/{}",
            mode_label(&snapshot.state.watcher.mode),
            snapshot.state.watcher.failure_streak,
            snapshot.config.watcher.failure_threshold
        ),
        if snapshot.watcher_online {
            "headless процесс активен".to_string()
        } else {
            "headless процесс не виден".to_string()
        },
    );

    render_status_card(
        frame,
        middle[1],
        "Telegram",
        if snapshot.state.watcher.telegram_running {
            "запущен".to_string()
        } else {
            "не запущен".to_string()
        },
        snapshot
            .state
            .watcher
            .next_check_at
            .as_ref()
            .map(|value| format!("следующая проверка {}", format_time(Some(value))))
            .unwrap_or_else(|| "нет данных о следующей проверке".to_string()),
    );

    render_status_card(
        frame,
        middle[2],
        "Runtime",
        if snapshot.autostart.installed || snapshot.config.autostart.enabled {
            format!(
                "autostart {}",
                snapshot
                    .autostart
                    .method
                    .as_ref()
                    .map(autostart_method_label)
                    .unwrap_or("pending")
            )
        } else {
            "ручной запуск".to_string()
        },
        format!(
            "auto-clean {}",
            if snapshot.config.watcher.auto_cleanup_dead_proxies {
                "on"
            } else {
                "off"
            }
        ),
    );

    frame.render_widget(
        Paragraph::new(console.activity_lines())
            .block(panel("Signals"))
            .wrap(Wrap { trim: true }),
        rows[2],
    );
}

fn render_control(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    paths: &AppPaths,
    snapshot: &UiSnapshot,
    console: &ConsoleState,
    actions: &[ConsoleAction],
) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(36), Constraint::Percentage(64)])
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
                Span::styled(if selected { "› " } else { "  " }, style),
                Span::styled(action.label(snapshot), style),
                Span::raw("  "),
                Span::styled(action.shortcut(), muted_style()),
            ]))
        })
        .collect::<Vec<_>>();
    frame.render_widget(List::new(action_items).block(panel("Control")), columns[0]);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(10), Constraint::Min(8)])
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
        Paragraph::new(console.activity_lines())
            .block(panel("Activity"))
            .wrap(Wrap { trim: true }),
        right[1],
    );
}

fn render_history(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    snapshot: &UiSnapshot,
    console: &ConsoleState,
) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(48), Constraint::Percentage(52)])
        .split(area);

    frame.render_widget(
        List::new(history_items(&snapshot.state)).block(panel("Recent proxies")),
        columns[0],
    );

    let detail = vec![
        kv_line("Последний fetch", format_time(snapshot.state.last_fetch_at.as_ref())),
        kv_line("Последний apply", format_time(snapshot.state.last_apply_at.as_ref())),
        kv_line("Config", snapshot.paths.config_file.display().to_string()),
        kv_line("State", snapshot.paths.state_file.display().to_string()),
        kv_line("Log", snapshot.paths.log_file.display().to_string()),
    ];

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(9), Constraint::Min(8)])
        .split(columns[1]);

    frame.render_widget(
        Paragraph::new(detail).block(panel("Files")).wrap(Wrap { trim: true }),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(console.activity_lines())
            .block(panel("Timeline"))
            .wrap(Wrap { trim: true }),
        rows[1],
    );
}

fn render_status_card(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    title: &str,
    headline: String,
    status: String,
) {
    let status_style = semantic_style(&status);
    let card = Paragraph::new(vec![
        Line::from(Span::styled(compact(&headline, 34), title_style())),
        Line::from(""),
        Line::from(Span::styled(compact(&status, 40), status_style)),
    ])
    .wrap(Wrap { trim: true })
    .block(panel(title));
    frame.render_widget(card, area);
}

fn doctor_lines(report: &app::DoctorSnapshot) -> Vec<Line<'static>> {
    let probe = match &report.provider_probe {
        Ok(proxy) => format!("ok: {proxy}"),
        Err(error) => format!("error: {error}"),
    };

    vec![
        Line::from(format!("Версия: {}", report.app_version)),
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
        Line::from(format!("Telegram запущен: {}", yes_no(report.telegram_running))),
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
        Line::from(format!("mtproto.ru: {}", probe)),
    ]
}

fn section_tabs(section: ConsoleSection) -> Line<'static> {
    let sections = [
        (ConsoleSection::Overview, "Overview"),
        (ConsoleSection::Control, "Control"),
        (ConsoleSection::History, "History"),
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

fn history_items(state: &AppState) -> Vec<ListItem<'static>> {
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
            ListItem::new(Line::from(vec![
                Span::styled("• ", muted_style()),
                Span::styled(record.proxy.short_label(), text_style()),
            ]))
        })
        .collect()
}

fn console_actions(snapshot: &UiSnapshot) -> Vec<ConsoleAction> {
    let mut actions = vec![
        ConsoleAction::SwitchNow,
        ConsoleAction::WatchControl,
        ConsoleAction::StopWatcher,
        ConsoleAction::ToggleAutostart,
        ConsoleAction::ToggleAutoCleanup,
        ConsoleAction::Settings,
        ConsoleAction::Doctor,
        ConsoleAction::OpenLog,
        ConsoleAction::OpenDataDir,
        ConsoleAction::Refresh,
        ConsoleAction::Exit,
    ];

    if snapshot.state.pending_proxy.is_some() && snapshot.state.watcher.telegram_running {
        actions.insert(1, ConsoleAction::ApplyPending);
    }

    if !(snapshot.autostart.installed || snapshot.config.autostart.enabled)
        && !snapshot.watcher_online
    {
        actions.retain(|action| *action != ConsoleAction::StopWatcher);
    }

    actions
}

fn find_action(actions: &[ConsoleAction], needle: ConsoleAction) -> Option<ConsoleAction> {
    actions.iter().copied().find(|action| *action == needle)
}

fn kv_line(label: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<18}"), muted_style()),
        Span::styled(compact(&value, 52), text_style()),
    ])
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

fn autostart_method_label(method: &AutostartMethod) -> &'static str {
    match method {
        AutostartMethod::ScheduledTask => "scheduled_task",
        AutostartMethod::StartupFolder => "startup_folder",
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

fn semantic_style(value: &str) -> Style {
    let lower = value.to_lowercase();
    if lower.contains("работает")
        || lower.contains("подключ")
        || lower.contains("доступ")
        || lower.contains("online")
        || lower.contains("ok")
        || lower.contains("вкл")
        || lower.contains("active")
    {
        return Style::default()
            .fg(Color::Rgb(118, 201, 160))
            .add_modifier(Modifier::BOLD);
    }
    if lower.contains("pending")
        || lower.contains("ожида")
        || lower.contains("checking")
        || lower.contains("switch")
        || lower.contains("жд")
        || lower.contains("manual")
    {
        return Style::default()
            .fg(Color::Rgb(247, 190, 103))
            .add_modifier(Modifier::BOLD);
    }
    if lower.contains("нет")
        || lower.contains("error")
        || lower.contains("не ")
        || lower.contains("idle")
        || lower.contains("off")
        || lower.contains("reject")
        || lower.contains("недоступ")
        || lower.contains("отклонил")
    {
        return Style::default()
            .fg(Color::Rgb(229, 118, 118))
            .add_modifier(Modifier::BOLD);
    }

    Style::default()
        .fg(Color::Rgb(109, 175, 255))
        .add_modifier(Modifier::BOLD)
}

fn panel(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(52, 66, 81)))
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

fn badge(text: &str, background: Color) -> Span<'static> {
    Span::styled(
        text.to_string(),
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
        terminal.clear().context("Не удалось очистить экран терминала")?;
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
        config
    }

    fn fields(&self) -> [SetupField; 6] {
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
                    "вкл".to_string()
                } else {
                    "выкл".to_string()
                },
                description: "Запускать headless watcher при логине Windows.",
            },
            SetupField {
                label: "Автоподчистка",
                value: if self.auto_cleanup_dead_proxies {
                    "вкл".to_string()
                } else {
                    "выкл".to_string()
                },
                description: "Автоматически удалять из Telegram proxy, которые ProtoSwitch считает мёртвыми.",
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
    autostart: windows::AutostartStatus,
    watcher_online: bool,
    paths: AppPaths,
}

impl UiSnapshot {
    fn load(paths: &AppPaths) -> anyhow::Result<Self> {
        let (config, state, autostart) = app::load_status_snapshot(paths)?;
        let watcher_online = app::watcher_process_exists() || app::watcher_is_recent(&config, &state);
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
    Overview,
    Control,
    History,
}

impl ConsoleSection {
    fn next(self) -> Self {
        match self {
            ConsoleSection::Overview => ConsoleSection::Control,
            ConsoleSection::Control => ConsoleSection::History,
            ConsoleSection::History => ConsoleSection::Overview,
        }
    }

    fn prev(self) -> Self {
        match self {
            ConsoleSection::Overview => ConsoleSection::History,
            ConsoleSection::Control => ConsoleSection::Overview,
            ConsoleSection::History => ConsoleSection::Control,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConsoleAction {
    SwitchNow,
    ApplyPending,
    WatchControl,
    StopWatcher,
    ToggleAutostart,
    ToggleAutoCleanup,
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
            ConsoleAction::SwitchNow => "switch now",
            ConsoleAction::ApplyPending => "apply pending",
            ConsoleAction::WatchControl => {
                if snapshot.watcher_online {
                    "restart watcher"
                } else {
                    "start watcher"
                }
            }
            ConsoleAction::StopWatcher => "stop watcher",
            ConsoleAction::ToggleAutostart => {
                if snapshot.autostart.installed || snapshot.config.autostart.enabled {
                    "disable autostart"
                } else {
                    "enable autostart"
                }
            }
            ConsoleAction::ToggleAutoCleanup => {
                if snapshot.config.watcher.auto_cleanup_dead_proxies {
                    "disable auto-clean"
                } else {
                    "enable auto-clean"
                }
            }
            ConsoleAction::Settings => "settings",
            ConsoleAction::Doctor => "doctor",
            ConsoleAction::OpenLog => "open log",
            ConsoleAction::OpenDataDir => "open data folder",
            ConsoleAction::Refresh => "refresh snapshot",
            ConsoleAction::Exit => "quit",
        }
    }

    fn shortcut(&self) -> &'static str {
        match self {
            ConsoleAction::SwitchNow => "[S]",
            ConsoleAction::ApplyPending => "[P]",
            ConsoleAction::WatchControl => "[W]",
            ConsoleAction::StopWatcher => "[X]",
            ConsoleAction::ToggleAutostart => "[A]",
            ConsoleAction::ToggleAutoCleanup => "[K]",
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
                "Сразу запросить нового кандидата у mtproto.ru и применить его в Telegram.".to_string(),
                "Теперь после auto-confirm ProtoSwitch дополнительно спрашивает у Telegram видимый статус proxy.".to_string(),
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
                "Включить автоматическое удаление мёртвых proxy после apply и при явной проверке.".to_string()
            }],
            ConsoleAction::Settings => vec![
                "Изменить интервалы watcher, timeout, порог сбоев, историю, автозапуск и автоподчистку.".to_string(),
            ],
            ConsoleAction::Doctor => vec![
                "Проверить tg:// handler, Telegram Desktop, mtproto.ru, state/config/logs и автозапуск.".to_string(),
            ],
            ConsoleAction::OpenLog => vec![format!("Открыть watch.log: {}", paths.log_file.display())],
            ConsoleAction::OpenDataDir => {
                vec![format!("Открыть рабочую папку: {}", paths.local_dir.display())]
            }
            ConsoleAction::Refresh => vec!["Перечитать status snapshot без побочных действий.".to_string()],
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
    inspector_title: String,
    inspector_lines: Vec<Line<'static>>,
    last_seen_error: Option<String>,
    force_refresh: bool,
}

impl ConsoleState {
    fn push_activity(&mut self, message: String) {
        self.activity.insert(0, message);
        while self.activity.len() > 8 {
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

    fn sync_error(&mut self, error: &Option<String>) {
        if self.last_seen_error != *error {
            if let Some(value) = error {
                self.push_activity(format!("error: {value}"));
            }
            self.last_seen_error = error.clone();
        }
    }

    fn activity_lines(&self) -> Vec<Line<'static>> {
        if self.activity.is_empty() {
            return vec![
                Line::from("Интерфейс готов."),
                Line::from("Здесь появляются результаты команд и сигналы watcher."),
            ];
        }

        self.activity.iter().cloned().map(Line::from).collect()
    }
}

impl Default for ConsoleState {
    fn default() -> Self {
        Self {
            section: ConsoleSection::Overview,
            focus: 0,
            activity: Vec::new(),
            inspector_title: "Context".to_string(),
            inspector_lines: Vec::new(),
            last_seen_error: None,
            force_refresh: false,
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
