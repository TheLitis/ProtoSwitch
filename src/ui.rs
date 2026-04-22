use std::io::{self, IsTerminal};
use std::time::Duration;

use anyhow::Context;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};

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
        session.terminal.draw(|frame| {
            let area = frame.area();
            frame.render_widget(Clear, area);

            let outer = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Length(5),
                    Constraint::Min(14),
                    Constraint::Length(3),
                ])
                .split(area);

            let body = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(56), Constraint::Percentage(44)])
                .split(outer[2]);

            let title = Paragraph::new(Line::from(vec![
                Span::styled("ProtoSwitch", title_style()),
                Span::raw(" "),
                Span::styled(APP_VERSION, muted_style()),
                Span::raw("  "),
                badge("Настройка", warn_color(), surface_color()),
            ]))
            .block(panel("Первый запуск", true));
            frame.render_widget(title, outer[0]);

            let hero = Paragraph::new(vec![
                Line::from("Запуск настроен как отдельный операторский экран: сначала выставьте режим watcher, затем сохраните конфиг."),
                Line::from("Изменения применяются стрелками. Enter сохраняет профиль, Esc возвращает исходные параметры."),
            ])
            .block(panel("Сценарий", false))
            .wrap(Wrap { trim: true });
            frame.render_widget(hero, outer[1]);

            let items = draft.fields().into_iter().enumerate().map(|(index, field)| {
                let style = if index == draft.focus {
                    Style::default()
                        .fg(accent_color())
                        .bg(selection_bg())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(text_color())
                };

                let row = Line::from(vec![
                    Span::styled(format!("{:<24}", field.label), style),
                    Span::styled(field.value, style),
                ]);
                ListItem::new(row)
            });

            let form = List::new(items).block(panel("Параметры", true));
            frame.render_widget(form, body[0]);

            let sidebar = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(9), Constraint::Min(8)])
                .split(body[1]);

            let summary = Paragraph::new(vec![
                Line::from(vec![
                    Span::raw("Проверка: "),
                    Span::styled(
                        format!("{} сек", draft.check_interval_secs),
                        value_style(),
                    ),
                ]),
                Line::from(vec![
                    Span::raw("TCP timeout: "),
                    Span::styled(
                        format!("{} сек", draft.connect_timeout_secs),
                        value_style(),
                    ),
                ]),
                Line::from(vec![
                    Span::raw("Порог сбоев: "),
                    Span::styled(draft.failure_threshold.to_string(), value_style()),
                ]),
                Line::from(vec![
                    Span::raw("История proxy: "),
                    Span::styled(draft.history_size.to_string(), value_style()),
                ]),
                Line::from(vec![
                    Span::raw("Автозапуск: "),
                    Span::styled(
                        if draft.autostart_enabled { "вкл" } else { "выкл" },
                        if draft.autostart_enabled {
                            positive_style()
                        } else {
                            muted_style()
                        },
                    ),
                ]),
            ])
            .block(panel("Профиль", false));
            frame.render_widget(summary, sidebar[0]);

            let active_field = draft.current_field();
            let tips = Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(active_field.label, title_style()),
                    Span::raw("  "),
                    Span::styled(active_field.value, value_style()),
                ]),
                Line::from(""),
                Line::from(active_field.description),
                Line::from(""),
                Line::from("Рекомендация: начните с интервала 30 сек и порога 3."),
                Line::from("Автозапуск нужен только если вы хотите фоновую работу сразу после логина."),
            ])
            .block(panel("Подсказка", false))
            .wrap(Wrap { trim: true });
            frame.render_widget(tips, sidebar[1]);

            let footer = Paragraph::new(
                "↑/↓ поле • ←/→ изменить • Enter сохранить • Esc отмена",
            )
            .block(panel("Управление", false));
            frame.render_widget(footer, outer[3]);
        })?;

        if let Event::Key(key) = event::read().context("Не удалось прочитать клавиатуру")? {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            match key.code {
                KeyCode::Up => draft.focus = draft.focus.saturating_sub(1),
                KeyCode::Down => draft.focus = (draft.focus + 1).min(4),
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
    let mut dashboard = DashboardState::default();

    loop {
        let (config, state, autostart) = app::load_status_snapshot(paths)?;
        let watcher_online = app::watcher_process_exists() || app::watcher_is_recent(&config, &state);
        let actions = dashboard_actions(&state, &config, &autostart, watcher_online);
        dashboard.focus = dashboard.focus.min(actions.len().saturating_sub(1));
        dashboard.sync_error(&state.last_error);

        session.terminal.draw(|frame| {
            render_dashboard(
                frame,
                paths,
                &config,
                &state,
                &autostart,
                watcher_online,
                &dashboard,
                &actions,
            );
        })?;

        if !event::poll(Duration::from_millis(250))? {
            continue;
        }

        let Event::Key(key) = event::read()? else {
            continue;
        };

        if key.kind != KeyEventKind::Press {
            continue;
        }

        let selected = actions[dashboard.focus];
        let direct = match key.code {
            KeyCode::Up => {
                dashboard.focus = dashboard.focus.saturating_sub(1);
                None
            }
            KeyCode::Down => {
                dashboard.focus = (dashboard.focus + 1).min(actions.len().saturating_sub(1));
                None
            }
            KeyCode::Char('q') | KeyCode::Esc => Some(DashboardAction::Exit),
            KeyCode::Enter => Some(selected),
            KeyCode::Char('s') => find_action(&actions, DashboardAction::SwitchNow),
            KeyCode::Char('p') => find_action(&actions, DashboardAction::ApplyPending),
            KeyCode::Char('w') => find_action(&actions, DashboardAction::WatchControl),
            KeyCode::Char('x') => find_action(&actions, DashboardAction::StopWatcher),
            KeyCode::Char('a') => find_action(&actions, DashboardAction::ToggleAutostart),
            KeyCode::Char('e') => find_action(&actions, DashboardAction::Settings),
            KeyCode::Char('d') => find_action(&actions, DashboardAction::Doctor),
            KeyCode::Char('l') => find_action(&actions, DashboardAction::OpenLog),
            KeyCode::Char('o') => find_action(&actions, DashboardAction::OpenDataDir),
            KeyCode::Char('r') => Some(DashboardAction::Refresh),
            _ => None,
        };

        let Some(action) = direct else {
            continue;
        };

        match action {
            DashboardAction::Exit => return Ok(()),
            DashboardAction::SwitchNow => match app::switch_to_candidate(paths, false) {
                Ok(message) => dashboard.set_result("Переключение", vec![message.clone()]),
                Err(error) => dashboard.push_activity(format!("switch: {error}")),
            },
            DashboardAction::ApplyPending => match app::apply_pending_proxy(paths) {
                Ok(message) => dashboard.set_result("Pending proxy", vec![message.clone()]),
                Err(error) => dashboard.push_activity(format!("pending: {error}")),
            },
            DashboardAction::WatchControl => {
                let result = if watcher_online {
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
                    Ok(message) => dashboard.set_result("Watcher", vec![message.clone()]),
                    Err(error) => dashboard.push_activity(format!("watcher: {error}")),
                }
            }
            DashboardAction::StopWatcher => match app::stop_background_watcher(paths) {
                Ok(stopped) => dashboard.set_result(
                    "Watcher",
                    vec![if stopped == 0 {
                        "Фоновый watcher не найден.".to_string()
                    } else {
                        format!("Остановлено headless watcher-процессов: {stopped}")
                    }],
                ),
                Err(error) => dashboard.push_activity(format!("watcher stop: {error}")),
            },
            DashboardAction::ToggleAutostart => {
                let enable = !(autostart.installed || config.autostart.enabled);
                match app::set_autostart_enabled(paths, enable) {
                    Ok(message) => dashboard.set_result("Автозапуск", vec![message.clone()]),
                    Err(error) => dashboard.push_activity(format!("autostart: {error}")),
                }
            }
            DashboardAction::Settings => {
                let current = config.clone();
                let original_marker = toml::to_string(&current)?;
                drop(session);
                let edited = run_setup(current)?;
                let edited_marker = toml::to_string(&edited)?;
                session = TerminalSession::new()?;

                if original_marker == edited_marker {
                    dashboard.set_result(
                        "Настройки",
                        vec!["Изменений нет. Конфиг оставлен без правок.".to_string()],
                    );
                } else {
                    match app::persist_config(paths, edited) {
                        Ok(message) => dashboard.set_result("Настройки", vec![message.clone()]),
                        Err(error) => dashboard.push_activity(format!("settings: {error}")),
                    }
                }
            }
            DashboardAction::Doctor => match app::doctor_snapshot(paths) {
                Ok(report) => dashboard.set_inspector("Doctor", doctor_lines(&report)),
                Err(error) => dashboard.push_activity(format!("doctor: {error}")),
            },
            DashboardAction::OpenLog => match app::open_in_notepad(&paths.log_file) {
                Ok(_) => dashboard.set_result(
                    "Журнал",
                    vec![format!("Открыт {}", paths.log_file.display())],
                ),
                Err(error) => dashboard.push_activity(format!("log: {error}")),
            },
            DashboardAction::OpenDataDir => match app::open_in_shell(&paths.local_dir) {
                Ok(_) => dashboard.set_result(
                    "Данные",
                    vec![format!("Открыт {}", paths.local_dir.display())],
                ),
                Err(error) => dashboard.push_activity(format!("data-dir: {error}")),
            },
            DashboardAction::Refresh => {
                dashboard.set_result(
                    "Снимок",
                    vec!["Статус обновлён, данные перечитаны из state/config.".to_string()],
                );
            }
        }
    }
}

fn render_dashboard(
    frame: &mut ratatui::Frame<'_>,
    paths: &AppPaths,
    config: &AppConfig,
    state: &AppState,
    autostart: &windows::AutostartStatus,
    watcher_online: bool,
    dashboard: &DashboardState,
    actions: &[DashboardAction],
) {
    let area = frame.area();
    frame.render_widget(Clear, area);

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Min(18),
            Constraint::Length(3),
        ])
        .split(area);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(31), Constraint::Min(30)])
        .split(outer[2]);

    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Length(9)])
        .split(body[0]);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(11), Constraint::Min(9)])
        .split(body[1]);

    let cards = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(right[0]);

    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(right[1]);

    let title = Paragraph::new(Line::from(vec![
        Span::styled("ProtoSwitch", title_style()),
        Span::raw(" "),
        Span::styled(APP_VERSION, muted_style()),
        Span::raw("  "),
        status_badge(state, watcher_online),
        Span::raw(" "),
        telegram_badge(state.watcher.telegram_running),
        Span::raw(" "),
        autostart_badge(config, autostart),
    ]))
    .block(panel("Операторский режим", true));
    frame.render_widget(title, outer[0]);

    let summary = Paragraph::new(vec![
        summary_line("Источник", &config.provider.source_url),
        Line::from(vec![
            metric_span("Последний fetch", &format_time(state.last_fetch_at.as_ref())),
            Span::raw("   "),
            metric_span("Последний apply", &format_time(state.last_apply_at.as_ref())),
        ]),
        Line::from(vec![
            metric_span(
                "Следующая проверка",
                &format_time(state.watcher.next_check_at.as_ref()),
            ),
            Span::raw("   "),
            metric_span(
                "Fail streak",
                &format!(
                    "{} / {}",
                    state.watcher.failure_streak, config.watcher.failure_threshold
                ),
            ),
        ]),
    ])
    .block(panel("Сводка", false))
    .wrap(Wrap { trim: true });
    frame.render_widget(summary, outer[1]);

    let action_items = actions.iter().enumerate().map(|(index, action)| {
        let style = if index == dashboard.focus {
            Style::default()
                .fg(accent_color())
                .bg(selection_bg())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(text_color())
        };

        ListItem::new(Line::from(vec![
            Span::styled(format!("{:<20}", action.label(config, autostart, watcher_online)), style),
            Span::styled(action.shortcut(), muted_style()),
        ]))
    });
    let actions_panel = List::new(action_items).block(panel("Команды", true));
    frame.render_widget(actions_panel, left[0]);

    let activity_lines = dashboard.activity_lines();
    let activity = Paragraph::new(activity_lines)
        .block(panel("Лента", false))
        .wrap(Wrap { trim: true });
    frame.render_widget(activity, left[1]);

    let current = state
        .current_proxy
        .as_ref()
        .map(|entry| entry.proxy.short_label())
        .unwrap_or_else(|| "не выбран".to_string());
    let pending = state
        .pending_proxy
        .as_ref()
        .map(|entry| entry.proxy.short_label())
        .unwrap_or_else(|| "нет".to_string());
    let proxy_card = Paragraph::new(vec![
        Line::from(vec![
            Span::raw("Текущий "),
            Span::styled(compact(&current, 42), value_style()),
        ]),
        Line::from(vec![
            Span::raw("Pending "),
            Span::styled(compact(&pending, 42), muted_style()),
        ]),
        Line::from(vec![
            Span::raw("История "),
            Span::styled(state.recent_proxies.len().to_string(), value_style()),
            Span::raw(" из "),
            Span::styled(config.watcher.history_size.to_string(), value_style()),
        ]),
        Line::from(vec![
            Span::raw("Сеть "),
            Span::styled(
                if state.current_proxy.is_some() { "MTProto" } else { "ожидание" },
                if state.current_proxy.is_some() {
                    positive_style()
                } else {
                    muted_style()
                },
            ),
        ]),
    ])
    .block(panel("Proxy", true))
    .wrap(Wrap { trim: true });
    frame.render_widget(proxy_card, cards[0]);

    let watcher_card = Paragraph::new(vec![
        Line::from(vec![
            Span::raw("Режим "),
            Span::styled(mode_label(&state.watcher.mode), value_style()),
        ]),
        Line::from(vec![
            Span::raw("Фон "),
            Span::styled(
                if watcher_online { "online" } else { "offline" },
                if watcher_online {
                    positive_style()
                } else {
                    danger_style()
                },
            ),
        ]),
        Line::from(vec![
            Span::raw("Telegram "),
            Span::styled(
                if state.watcher.telegram_running { "запущен" } else { "не найден" },
                if state.watcher.telegram_running {
                    positive_style()
                } else {
                    muted_style()
                },
            ),
        ]),
        Line::from(vec![
            Span::raw("Автозапуск "),
            Span::styled(
                if autostart.installed {
                    autostart
                        .method
                        .as_ref()
                        .map(autostart_method_label)
                        .unwrap_or("unknown")
                } else {
                    "нет"
                },
                if autostart.installed {
                    value_style()
                } else {
                    muted_style()
                },
            ),
        ]),
    ])
    .block(panel("Watcher", true))
    .wrap(Wrap { trim: true });
    frame.render_widget(watcher_card, cards[1]);

    let history_lines = if state.recent_proxies.is_empty() {
        vec![Line::from("История пока пуста.")]
    } else {
        state
            .recent_proxies
            .iter()
            .take(8)
            .map(|record| {
                Line::from(format!(
                    "{}  {}",
                    record.captured_at.format("%Y-%m-%d %H:%M:%S"),
                    compact(&record.proxy.short_label(), 44)
                ))
            })
            .collect()
    };
    let history = Paragraph::new(history_lines)
        .block(panel("Последние proxy", false))
        .wrap(Wrap { trim: true });
    frame.render_widget(history, bottom[0]);

    let inspector = Paragraph::new(inspector_lines(
        dashboard,
        actions[dashboard.focus],
        config,
        autostart,
        watcher_online,
        paths,
    ))
    .block(panel(&dashboard.inspector_title, false))
    .wrap(Wrap { trim: true });
    frame.render_widget(inspector, bottom[1]);

    let footer = Paragraph::new(
        "↑↓ выбрать • Enter выполнить • S switch • P pending • W watcher • X stop • A autostart • E settings • D doctor • L log • O data • R refresh • Q exit",
    )
    .block(panel("Клавиши", false))
    .wrap(Wrap { trim: true });
    frame.render_widget(footer, outer[3]);
}

fn find_action(actions: &[DashboardAction], wanted: DashboardAction) -> Option<DashboardAction> {
    actions.iter().copied().find(|action| *action == wanted)
}

fn dashboard_actions(
    state: &AppState,
    config: &AppConfig,
    autostart: &windows::AutostartStatus,
    watcher_online: bool,
) -> Vec<DashboardAction> {
    let mut actions = vec![DashboardAction::SwitchNow];

    if state.pending_proxy.is_some() && state.watcher.telegram_running {
        actions.push(DashboardAction::ApplyPending);
    }

    actions.push(DashboardAction::WatchControl);

    if watcher_online {
        actions.push(DashboardAction::StopWatcher);
    }

    if autostart.installed || config.autostart.enabled || !watcher_online {
        actions.push(DashboardAction::ToggleAutostart);
    } else {
        actions.push(DashboardAction::ToggleAutostart);
    }

    actions.push(DashboardAction::Settings);
    actions.push(DashboardAction::Doctor);
    actions.push(DashboardAction::OpenLog);
    actions.push(DashboardAction::OpenDataDir);
    actions.push(DashboardAction::Refresh);
    actions.push(DashboardAction::Exit);
    actions
}

fn doctor_lines(report: &app::DoctorSnapshot) -> Vec<Line<'static>> {
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
        Line::from(format!(
            "Telegram запущен: {}",
            yes_no(report.telegram_running)
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
            } else {
                "нет"
            }
        )),
        Line::from(format!(
            "mtproto.ru: {}",
            match &report.provider_probe {
                Ok(proxy) => format!("ok ({proxy})"),
                Err(error) => format!("error ({error})"),
            }
        )),
    ]
}

fn inspector_lines(
    dashboard: &DashboardState,
    selected: DashboardAction,
    config: &AppConfig,
    autostart: &windows::AutostartStatus,
    watcher_online: bool,
    paths: &AppPaths,
) -> Vec<Line<'static>> {
    if !dashboard.inspector_lines.is_empty() {
        return dashboard.inspector_lines.clone();
    }

    let mut lines = vec![
        Line::from(selected.label(config, autostart, watcher_online)),
        Line::from(""),
    ];

    lines.extend(
        selected
            .description(config, autostart, watcher_online, paths)
            .into_iter()
            .map(Line::from),
    );
    lines
}

fn summary_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label}: "), muted_style()),
        Span::styled(compact(value, 96), value_style()),
    ])
}

fn metric_span(label: &str, value: &str) -> Span<'static> {
    Span::styled(format!("{label}: {}", compact(value, 30)), value_style())
}

fn status_badge(state: &AppState, watcher_online: bool) -> Span<'static> {
    if state.last_error.is_some() {
        return badge("attention", danger_color(), surface_color());
    }

    if watcher_online && state.current_proxy.is_some() {
        return badge("ready", positive_color(), surface_color());
    }

    badge("standby", warn_color(), surface_color())
}

fn telegram_badge(running: bool) -> Span<'static> {
    if running {
        badge("telegram on", positive_color(), surface_color())
    } else {
        badge("telegram off", warn_color(), surface_color())
    }
}

fn autostart_badge(config: &AppConfig, autostart: &windows::AutostartStatus) -> Span<'static> {
    if autostart.installed || config.autostart.enabled {
        badge("autostart", accent_color(), surface_color())
    } else {
        badge("manual", muted_color(), surface_color())
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "да" } else { "нет" }
}

fn compact(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }

    let head = max.saturating_sub(6) / 2;
    let tail = max.saturating_sub(6) - head;
    let prefix = value.chars().take(head).collect::<String>();
    let suffix = value
        .chars()
        .rev()
        .take(tail)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{prefix} … {suffix}")
}

fn panel(title: &str, accent: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(if accent {
            Style::default().fg(accent_color())
        } else {
            Style::default().fg(border_color())
        })
        .title(title.to_string())
}

fn accent_color() -> Color {
    Color::Rgb(77, 208, 255)
}

fn border_color() -> Color {
    Color::Rgb(50, 74, 89)
}

fn text_color() -> Color {
    Color::Rgb(232, 238, 245)
}

fn muted_color() -> Color {
    Color::Rgb(149, 166, 182)
}

fn positive_color() -> Color {
    Color::Rgb(104, 211, 145)
}

fn warn_color() -> Color {
    Color::Rgb(255, 193, 94)
}

fn danger_color() -> Color {
    Color::Rgb(255, 124, 135)
}

fn surface_color() -> Color {
    Color::Rgb(15, 23, 30)
}

fn selection_bg() -> Color {
    Color::Rgb(19, 45, 58)
}

fn title_style() -> Style {
    Style::default()
        .fg(text_color())
        .add_modifier(Modifier::BOLD)
}

fn value_style() -> Style {
    Style::default()
        .fg(accent_color())
        .add_modifier(Modifier::BOLD)
}

fn muted_style() -> Style {
    Style::default().fg(muted_color())
}

fn positive_style() -> Style {
    Style::default()
        .fg(positive_color())
        .add_modifier(Modifier::BOLD)
}

fn danger_style() -> Style {
    Style::default()
        .fg(danger_color())
        .add_modifier(Modifier::BOLD)
}

fn badge(text: &str, fg: Color, bg: Color) -> Span<'static> {
    Span::styled(
        text.to_string(),
        Style::default()
            .fg(bg)
            .bg(fg)
            .add_modifier(Modifier::BOLD),
    )
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

fn mode_label(mode: &WatcherMode) -> &'static str {
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

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TerminalSession {
    fn new() -> anyhow::Result<Self> {
        enable_raw_mode().context("Не удалось включить raw mode")?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen).context("Не удалось открыть alternate screen")?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).context("Не удалось создать TUI-терминал")?;
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
        }
    }

    fn adjust(&mut self, increase: bool) {
        match self.focus {
            0 => adjust_u64(&mut self.check_interval_secs, increase, 5, 300, 5),
            1 => adjust_u64(&mut self.connect_timeout_secs, increase, 1, 30, 1),
            2 => adjust_u32(&mut self.failure_threshold, increase, 1, 10, 1),
            3 => adjust_usize(&mut self.history_size, increase, 1, 20, 1),
            4 => self.autostart_enabled = !self.autostart_enabled,
            _ => {}
        }
    }

    fn into_config(self) -> AppConfig {
        let mut config = self.original;
        config.watcher.check_interval_secs = self.check_interval_secs;
        config.watcher.connect_timeout_secs = self.connect_timeout_secs;
        config.watcher.failure_threshold = self.failure_threshold;
        config.watcher.history_size = self.history_size;
        config.autostart.enabled = self.autostart_enabled;
        config
    }

    fn fields(&self) -> [SetupField; 5] {
        [
            SetupField {
                label: "Интервал проверки",
                value: format!("{} сек", self.check_interval_secs),
                description:
                    "Как часто watcher будет проверять текущий proxy и искать замену при деградации.",
            },
            SetupField {
                label: "TCP timeout",
                value: format!("{} сек", self.connect_timeout_secs),
                description:
                    "Сколько ждать ответа от сервера в TCP health-check до признания проверки неудачной.",
            },
            SetupField {
                label: "Порог сбоев",
                value: self.failure_threshold.to_string(),
                description:
                    "Количество подряд неудачных проверок, после которого ProtoSwitch начнёт ротацию proxy.",
            },
            SetupField {
                label: "История proxy",
                value: self.history_size.to_string(),
                description:
                    "Сколько последних proxy хранить, чтобы избегать мгновенного возврата на тот же сервер.",
            },
            SetupField {
                label: "Автозапуск watcher",
                value: if self.autostart_enabled {
                    "вкл".to_string()
                } else {
                    "выкл".to_string()
                },
                description:
                    "Включает фоновый запуск при логине Windows через scheduled_task или startup_folder fallback.",
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum DashboardAction {
    SwitchNow,
    ApplyPending,
    WatchControl,
    StopWatcher,
    ToggleAutostart,
    Settings,
    Doctor,
    OpenLog,
    OpenDataDir,
    Refresh,
    Exit,
}

impl DashboardAction {
    fn label(
        &self,
        config: &AppConfig,
        autostart: &windows::AutostartStatus,
        watcher_online: bool,
    ) -> &'static str {
        match self {
            DashboardAction::SwitchNow => "Switch proxy",
            DashboardAction::ApplyPending => "Применить pending",
            DashboardAction::WatchControl => {
                if watcher_online {
                    "Перезапустить watcher"
                } else {
                    "Запустить watcher"
                }
            }
            DashboardAction::StopWatcher => "Остановить watcher",
            DashboardAction::ToggleAutostart => {
                if autostart.installed || config.autostart.enabled {
                    "Выключить автозапуск"
                } else {
                    "Включить автозапуск"
                }
            }
            DashboardAction::Settings => "Настройки",
            DashboardAction::Doctor => "Doctor",
            DashboardAction::OpenLog => "Открыть watch.log",
            DashboardAction::OpenDataDir => "Открыть данные",
            DashboardAction::Refresh => "Обновить снимок",
            DashboardAction::Exit => "Закрыть",
        }
    }

    fn shortcut(&self) -> &'static str {
        match self {
            DashboardAction::SwitchNow => "[S]",
            DashboardAction::ApplyPending => "[P]",
            DashboardAction::WatchControl => "[W]",
            DashboardAction::StopWatcher => "[X]",
            DashboardAction::ToggleAutostart => "[A]",
            DashboardAction::Settings => "[E]",
            DashboardAction::Doctor => "[D]",
            DashboardAction::OpenLog => "[L]",
            DashboardAction::OpenDataDir => "[O]",
            DashboardAction::Refresh => "[R]",
            DashboardAction::Exit => "[Q]",
        }
    }

    fn description(
        &self,
        config: &AppConfig,
        autostart: &windows::AutostartStatus,
        watcher_online: bool,
        paths: &AppPaths,
    ) -> Vec<String> {
        match self {
            DashboardAction::SwitchNow => vec![
                "Принудительно запросить новый proxy у mtproto.ru и сразу применить его в Telegram.".to_string(),
                "Если Telegram открыт, подтверждение proxy выполнится автоматически.".to_string(),
            ],
            DashboardAction::ApplyPending => vec![
                "Применить уже сохранённый pending proxy без нового fetch.".to_string(),
                "Команда доступна только когда Telegram уже запущен.".to_string(),
            ],
            DashboardAction::WatchControl => vec![if watcher_online {
                "Остановить текущий headless watcher и поднять его заново с актуальным конфигом.".to_string()
            } else {
                "Запустить headless watcher в фоне без перезапуска интерфейса.".to_string()
            }],
            DashboardAction::StopWatcher => vec![
                "Завершить фоновые headless watcher-процессы ProtoSwitch.".to_string(),
                "UI останется открыт, но автоматическая ротация будет остановлена.".to_string(),
            ],
            DashboardAction::ToggleAutostart => vec![if autostart.installed || config.autostart.enabled {
                "Выключить запуск ProtoSwitch при входе в Windows.".to_string()
            } else {
                "Включить запуск ProtoSwitch при входе в Windows.".to_string()
            }],
            DashboardAction::Settings => vec![
                "Открыть экран настройки watcher и параметров хранения истории.".to_string(),
                "После сохранения watcher будет перезапущен с новым конфигом.".to_string(),
            ],
            DashboardAction::Doctor => vec![
                "Снять оперативную диагностику: tg:// handler, Telegram Desktop, mtproto.ru, файлы, автозапуск.".to_string(),
            ],
            DashboardAction::OpenLog => vec![
                format!("Открыть журнал в Notepad: {}", paths.log_file.display()),
            ],
            DashboardAction::OpenDataDir => vec![
                format!("Открыть рабочую папку данных: {}", paths.local_dir.display()),
            ],
            DashboardAction::Refresh => vec![
                "Перечитать state/config и перерисовать dashboard без побочных действий.".to_string(),
            ],
            DashboardAction::Exit => vec![
                "Закрыть операторский экран. Фоновый watcher продолжит работу, если уже запущен.".to_string(),
            ],
        }
    }
}

struct DashboardState {
    focus: usize,
    activity: Vec<String>,
    inspector_title: String,
    inspector_lines: Vec<Line<'static>>,
    last_seen_error: Option<String>,
}

impl DashboardState {
    fn push_activity(&mut self, message: String) {
        self.activity.insert(0, message);
        while self.activity.len() > 7 {
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
        self.inspector_lines = lines.clone();
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
                Line::from("Здесь появятся результаты команд и сигналы watcher."),
            ];
        }

        self.activity.iter().cloned().map(Line::from).collect()
    }
}

impl Default for DashboardState {
    fn default() -> Self {
        Self {
            focus: 0,
            activity: Vec::new(),
            inspector_title: "Инспектор".to_string(),
            inspector_lines: Vec::new(),
            last_seen_error: None,
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
