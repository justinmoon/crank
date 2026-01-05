use std::fs;
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use chrono::Local;
use crossterm::event::{self, Event, KeyCode, KeyEvent};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Terminal;
use serde::{Deserialize, Serialize};

use crate::task::model::Task;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AlertKind {
    Completed,
    NeedsHelp,
}

impl AlertKind {
    fn label(&self) -> &'static str {
        match self {
            AlertKind::Completed => "done",
            AlertKind::NeedsHelp => "help",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: String,
    pub task_id: String,
    pub title: String,
    pub kind: AlertKind,
    pub message: Option<String>,
    pub tmux_pane: Option<String>,
    pub tmux_window_id: Option<String>,
    pub tmux_window_name: Option<String>,
    pub created_at: i64,
}

pub fn create_task_alert(
    task: &Task,
    kind: AlertKind,
    message: Option<String>,
    tmux_pane: &str,
) -> Result<Alert> {
    let timestamp = Local::now().timestamp_millis();
    let alert_id = format!("{}-{}-{}", task.id, kind.label(), timestamp);
    let tmux_pane = tmux_pane.trim();
    let (window_id, window_name) = if tmux_pane.is_empty() {
        (None, None)
    } else {
        tmux_window_info(tmux_pane)?
    };

    let alert = Alert {
        id: alert_id,
        task_id: task.id.clone(),
        title: task.title.clone(),
        kind,
        message,
        tmux_pane: if tmux_pane.is_empty() {
            None
        } else {
            Some(tmux_pane.to_string())
        },
        tmux_window_id: window_id,
        tmux_window_name: window_name,
        created_at: timestamp,
    };

    write_alert(&alert)?;
    Ok(alert)
}

pub fn load_alerts() -> Result<Vec<Alert>> {
    let dir = alerts_dir()?;
    let mut alerts = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let content = match crate::crank_io::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => continue,
        };
        let alert: Alert = match serde_json::from_str(&content) {
            Ok(alert) => alert,
            Err(_) => continue,
        };
        alerts.push(alert);
    }
    alerts.sort_by_key(|alert| alert.created_at);
    alerts.reverse();
    Ok(alerts)
}

pub fn dismiss_alert(alert_id: &str) -> Result<()> {
    let path = alert_path(alert_id)?;
    let _ = fs::remove_file(path);
    Ok(())
}

fn show_alerts_popup_for_client(client: &str) -> Result<()> {
    if std::env::var("TMUX").unwrap_or_default().is_empty() {
        return Ok(());
    }
    let client = client.trim();
    if client.is_empty() {
        return Ok(());
    }

    let crank_bin = std::env::current_exe().context("failed to resolve crank binary path")?;
    let crank_bin = crank_bin
        .to_str()
        .ok_or_else(|| anyhow!("crank binary path is not valid UTF-8"))?;

    let status = Command::new("tmux")
        .args([
            "display-popup",
            "-E",
            "-c",
            client,
            "-w",
            "70%",
            "-h",
            "60%",
        ])
        .arg(crank_bin)
        .arg("alerts")
        .status()
        .context("failed to open alerts popup")?;
    if !status.success() {
        return Err(anyhow!("tmux display-popup failed"));
    }
    Ok(())
}

pub fn show_alerts_popup_for_all_clients() -> Result<()> {
    if std::env::var("TMUX").unwrap_or_default().is_empty() {
        return Ok(());
    }
    let clients = tmux_clients()?;
    for client in clients {
        let _ = show_alerts_popup_for_client(&client);
    }
    Ok(())
}

pub fn run_alerts_picker() -> Result<()> {
    let mut state = AlertState::new(load_alerts()?);
    let mut terminal = setup_terminal()?;

    let result = run_loop(&mut terminal, &mut state);

    restore_terminal(&mut terminal)?;
    result
}

pub fn run_alerts_watch() -> Result<()> {
    if std::env::var("TMUX").unwrap_or_default().is_empty() {
        return Err(anyhow!("crank alerts --watch must run inside tmux"));
    }

    let mut last_seen = latest_alert_timestamp(&load_alerts().unwrap_or_default());
    let mut last_popup = Instant::now()
        .checked_sub(Duration::from_secs(5))
        .unwrap_or_else(Instant::now);

    loop {
        let alerts = load_alerts().unwrap_or_default();
        let latest = latest_alert_timestamp(&alerts);

        if latest > last_seen {
            if last_popup.elapsed() >= Duration::from_millis(750) {
                let _ = show_alerts_popup_for_all_clients();
                last_popup = Instant::now();
            }
            last_seen = latest;
        }

        std::thread::sleep(Duration::from_millis(500));
    }
}

fn alerts_dir() -> Result<PathBuf> {
    let dir = crate::crank_io::user_crank_dir()?.join("alerts");
    crate::crank_io::ensure_dir(&dir)
        .with_context(|| format!("failed to create alerts dir: {}", dir.display()))?;
    Ok(dir)
}

fn alert_path(id: &str) -> Result<PathBuf> {
    Ok(alerts_dir()?.join(format!("{id}.json")))
}

fn write_alert(alert: &Alert) -> Result<()> {
    let path = alert_path(&alert.id)?;
    let content = serde_json::to_string_pretty(alert)?;
    crate::crank_io::write_string(&path, content)?;
    Ok(())
}

fn latest_alert_timestamp(alerts: &[Alert]) -> i64 {
    alerts
        .iter()
        .map(|alert| alert.created_at)
        .max()
        .unwrap_or(0)
}

fn tmux_window_info(pane: &str) -> Result<(Option<String>, Option<String>)> {
    let output = Command::new("tmux")
        .args([
            "display-message",
            "-p",
            "-t",
            pane,
            "#{window_id}|#{window_name}",
        ])
        .output();

    let output = match output {
        Ok(output) if output.status.success() => output,
        _ => return Ok((None, None)),
    };

    let raw = String::from_utf8_lossy(&output.stdout);
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok((None, None));
    }
    let mut parts = trimmed.splitn(2, '|');
    let window_id = parts.next().map(|value| value.to_string());
    let window_name = parts.next().map(|value| value.to_string());
    Ok((window_id, window_name))
}

fn tmux_clients() -> Result<Vec<String>> {
    let output = Command::new("tmux")
        .args(["list-clients", "-F", "#{client_tty} #{client_attached}"])
        .output();

    let output = match output {
        Ok(output) if output.status.success() => output,
        _ => return Ok(Vec::new()),
    };

    let raw = String::from_utf8_lossy(&output.stdout);
    let mut clients = Vec::new();
    for line in raw.lines() {
        let mut parts = line.split_whitespace();
        let client_tty = parts.next();
        let client_attached = parts.next();

        if client_attached == Some("1") {
            if let Some(client_tty) = client_tty {
                clients.push(client_tty.to_string());
            }
        }
    }

    Ok(clients)
}

struct AlertState {
    alerts: Vec<Alert>,
    list_state: ListState,
    error: Option<String>,
}

impl AlertState {
    fn new(alerts: Vec<Alert>) -> Self {
        let mut list_state = ListState::default();
        if !alerts.is_empty() {
            list_state.select(Some(0));
        }
        Self {
            alerts,
            list_state,
            error: None,
        }
    }

    fn selected_alert(&self) -> Option<&Alert> {
        let index = self.list_state.selected()?;
        self.alerts.get(index)
    }

    fn move_selection(&mut self, delta: i32) {
        if self.alerts.is_empty() {
            return;
        }
        let max_index = self.alerts.len().saturating_sub(1) as i32;
        let current = self.list_state.selected().unwrap_or(0) as i32;
        let next = (current + delta).clamp(0, max_index) as usize;
        self.list_state.select(Some(next));
    }

    fn remove_selected(&mut self) {
        if let Some(index) = self.list_state.selected() {
            if index < self.alerts.len() {
                self.alerts.remove(index);
            }
        }
        if self.alerts.is_empty() {
            self.list_state.select(None);
        } else if let Some(selected) = self.list_state.selected() {
            let next = selected.min(self.alerts.len() - 1);
            self.list_state.select(Some(next));
        }
    }
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut AlertState,
) -> Result<()> {
    loop {
        terminal.draw(|frame| {
            let size = frame.area();
            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(2),
                    Constraint::Min(1),
                    Constraint::Length(2),
                ])
                .split(size);

            let header = Paragraph::new("Alerts").style(Style::default().fg(Color::Cyan));
            frame.render_widget(header, layout[0]);

            if state.alerts.is_empty() {
                let empty = Paragraph::new("No active alerts")
                    .style(Style::default().fg(Color::DarkGray))
                    .block(Block::default().borders(Borders::ALL));
                frame.render_widget(empty, layout[1]);
            } else {
                let list = render_list(state);
                frame.render_stateful_widget(list, layout[1], &mut state.list_state);
            }

            let footer = render_footer(state);
            frame.render_widget(footer, layout[2]);
        })?;

        if event::poll(std::time::Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if handle_key(state, key)? {
                    return Ok(());
                }
            }
        }
    }
}

fn handle_key(state: &mut AlertState, key: KeyEvent) -> Result<bool> {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
        KeyCode::Char('j') | KeyCode::Down => state.move_selection(1),
        KeyCode::Char('k') | KeyCode::Up => state.move_selection(-1),
        KeyCode::Char('d') => {
            if let Some(alert) = state.selected_alert().cloned() {
                dismiss_alert(&alert.id)?;
                state.remove_selected();
            }
        }
        KeyCode::Enter => {
            if let Some(alert) = state.selected_alert().cloned() {
                if let Err(err) = jump_to_alert(&alert) {
                    state.error = Some(err.to_string());
                    return Ok(false);
                }
                dismiss_alert(&alert.id)?;
                return Ok(true);
            }
        }
        _ => {}
    }
    Ok(false)
}

fn jump_to_alert(alert: &Alert) -> Result<()> {
    if let Some(window_id) = alert.tmux_window_id.as_deref() {
        let status = Command::new("tmux")
            .args(["select-window", "-t", window_id])
            .status()
            .context("failed to select tmux window")?;
        if !status.success() {
            return Err(anyhow!("tmux select-window failed"));
        }
    }
    if let Some(pane) = alert.tmux_pane.as_deref() {
        let status = Command::new("tmux")
            .args(["select-pane", "-t", pane])
            .status()
            .context("failed to select tmux pane")?;
        if !status.success() {
            return Err(anyhow!("tmux select-pane failed"));
        }
    }
    Ok(())
}

fn render_list(state: &AlertState) -> List<'static> {
    let items = state
        .alerts
        .iter()
        .map(|alert| {
            let kind_style = match alert.kind {
                AlertKind::Completed => Style::default().fg(Color::Green),
                AlertKind::NeedsHelp => Style::default().fg(Color::Yellow),
            };
            let mut spans = vec![Span::styled(
                format!("[{}] ", alert.kind.label()),
                kind_style.add_modifier(Modifier::BOLD),
            )];
            spans.push(Span::styled(
                format!("{} ", alert.task_id),
                Style::default().fg(Color::DarkGray),
            ));
            spans.push(Span::raw(alert.title.clone()));
            if let Some(window_name) = &alert.tmux_window_name {
                spans.push(Span::styled(
                    format!(" ({window_name})"),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect::<Vec<_>>();

    List::new(items)
        .block(Block::default().borders(Borders::ALL))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ")
}

fn render_footer(state: &AlertState) -> Paragraph<'_> {
    let (footer, style) = if let Some(err) = &state.error {
        (format!("Error: {err}"), Style::default().fg(Color::Red))
    } else {
        (
            "j/k move  enter jump  d dismiss  q quit".to_string(),
            Style::default().fg(Color::DarkGray),
        )
    };

    Paragraph::new(Text::from(footer))
        .style(style)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::NONE))
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode().context("failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode().context("failed to disable raw mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)
        .context("failed to leave alternate screen")?;
    terminal.show_cursor()?;
    Ok(())
}
