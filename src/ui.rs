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
use ratatui::layout::{Constraint, Direction, Layout, Margin};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};

use crate::APP_VERSION;
use crate::model::{AppConfig, AppState, WatcherMode};
use crate::paths::AppPaths;

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

            let sections = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(8),
                    Constraint::Length(3),
                ])
                .split(area);

            let title = Paragraph::new(format!("ProtoSwitch {APP_VERSION}")).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Первый запуск"),
            );
            frame.render_widget(title, sections[0]);

            let items = [
                format!("Интервал проверки: {} сек", draft.check_interval_secs),
                format!("Таймаут TCP-проверки: {} сек", draft.connect_timeout_secs),
                format!("Порог подряд идущих сбоев: {}", draft.failure_threshold),
                format!("Размер истории proxy: {}", draft.history_size),
                format!(
                    "Автозапуск watcher: {}",
                    if draft.autostart_enabled {
                        "вкл"
                    } else {
                        "выкл"
                    }
                ),
            ];

            let rows = items.iter().enumerate().map(|(index, item)| {
                let style = if index == draft.focus {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                ListItem::new(item.as_str()).style(style)
            });

            let list =
                List::new(rows).block(Block::default().borders(Borders::ALL).title("Настройки"));
            frame.render_widget(list, sections[1]);

            let footer =
                Paragraph::new("↑/↓ выбрать • ←/→ изменить • Enter сохранить • Esc отмена")
                    .block(Block::default().borders(Borders::ALL).title("Управление"));
            frame.render_widget(footer, sections[2]);
        })?;

        if let Event::Key(key) = event::read().context("Не удалось прочитать клавиатуру")?
        {
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

pub fn run_status(paths: &AppPaths, config: &AppConfig, state: &AppState) -> anyhow::Result<()> {
    let mut session = TerminalSession::new()?;

    loop {
        session.terminal.draw(|frame| {
            let area = frame.area();
            let outer = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Length(9),
                    Constraint::Length(9),
                    Constraint::Min(6),
                    Constraint::Length(3),
                ])
                .split(area);

            let title = Paragraph::new(format!("ProtoSwitch {APP_VERSION}"))
                .block(Block::default().borders(Borders::ALL).title("Статус"));
            frame.render_widget(title, outer[0]);

            let top = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(outer[1]);

            let current = state
                .current_proxy
                .as_ref()
                .map(|entry| entry.proxy.short_label())
                .unwrap_or_else(|| "Не выбран".to_string());
            let pending = state
                .pending_proxy
                .as_ref()
                .map(|entry| entry.proxy.short_label())
                .unwrap_or_else(|| "Нет".to_string());

            let proxy_panel = Paragraph::new(vec![
                Line::from(vec![
                    Span::raw("Текущий: "),
                    Span::styled(current, highlight()),
                ]),
                Line::from(vec![
                    Span::raw("Pending: "),
                    Span::styled(pending, highlight()),
                ]),
                Line::from(format!("Источник: {}", config.provider.source_url)),
                Line::from(format!("История: {} proxy", state.recent_proxies.len())),
            ])
            .block(Block::default().borders(Borders::ALL).title("Proxy"));
            frame.render_widget(proxy_panel, top[0]);

            let watcher_panel = Paragraph::new(vec![
                Line::from(format!("Режим: {}", mode_label(&state.watcher.mode))),
                Line::from(format!(
                    "Telegram запущен: {}",
                    yes_no(state.watcher.telegram_running)
                )),
                Line::from(format!(
                    "Failure streak: {} / {}",
                    state.watcher.failure_streak, config.watcher.failure_threshold
                )),
                Line::from(format!(
                    "Следующая проверка: {}",
                    format_time(state.watcher.next_check_at.as_ref())
                )),
            ])
            .block(Block::default().borders(Borders::ALL).title("Watcher"));
            frame.render_widget(watcher_panel, top[1]);

            let middle = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(outer[2]);

            let timing_panel = Paragraph::new(vec![
                Line::from(format!(
                    "Интервал: {} сек",
                    config.watcher.check_interval_secs
                )),
                Line::from(format!(
                    "Таймаут TCP: {} сек",
                    config.watcher.connect_timeout_secs
                )),
                Line::from(format!(
                    "Последний fetch: {}",
                    format_time(state.last_fetch_at.as_ref())
                )),
                Line::from(format!(
                    "Последний apply: {}",
                    format_time(state.last_apply_at.as_ref())
                )),
            ])
            .block(Block::default().borders(Borders::ALL).title("Тайминг"));
            frame.render_widget(timing_panel, middle[0]);

            let paths_panel = Paragraph::new(vec![
                Line::from(paths.config_file.display().to_string()),
                Line::from(paths.state_file.display().to_string()),
                Line::from(paths.log_file.display().to_string()),
                Line::from(format!(
                    "Автозапуск: {}",
                    if config.autostart.enabled {
                        "вкл"
                    } else {
                        "выкл"
                    }
                )),
            ])
            .block(Block::default().borders(Borders::ALL).title("Файлы"));
            frame.render_widget(paths_panel, middle[1]);

            let history_lines = if state.recent_proxies.is_empty() {
                vec![Line::from("История пока пуста")]
            } else {
                state
                    .recent_proxies
                    .iter()
                    .take(6)
                    .map(|record| {
                        Line::from(format!(
                            "{} • {}",
                            record.captured_at.format("%Y-%m-%d %H:%M:%S"),
                            record.proxy.short_label()
                        ))
                    })
                    .collect()
            };
            let history_panel = Paragraph::new(history_lines)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Последние proxy"),
                )
                .wrap(ratatui::widgets::Wrap { trim: true });
            frame.render_widget(history_panel, outer[3].inner(Margin::new(0, 0)));

            let footer_text = state
                .last_error
                .clone()
                .unwrap_or_else(|| "q / Esc чтобы закрыть".to_string());
            let footer = Paragraph::new(footer_text)
                .block(Block::default().borders(Borders::ALL).title("Сигнал"));
            frame.render_widget(footer, outer[4]);
        })?;

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => return Ok(()),
                        _ => {}
                    }
                }
            }
        }
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "да" } else { "нет" }
}

fn highlight() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
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
