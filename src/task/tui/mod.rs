use std::cmp::min;
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Terminal;

use crate::task::model::{sort_tasks, Task};
use crate::task::store;

struct TuiState {
    tasks: Vec<Task>,
    filtered: Vec<usize>,
    list_state: ListState,
    show_all: bool,
    preview_on: bool,
    show_help: bool,
    confirm_delete: bool,
    delete_target: Option<usize>,
    filter_mode: bool,
    filter_query: String,
    preview_scroll: u16,
    list_height: usize,
    error: Option<String>,
    require_selection: bool,
}

impl TuiState {
    fn new(mut tasks: Vec<Task>, options: PickerOptions) -> Self {
        sort_tasks(&mut tasks);
        let mut state = Self {
            tasks,
            filtered: Vec::new(),
            list_state: ListState::default(),
            show_all: false,
            preview_on: true,
            show_help: false,
            confirm_delete: false,
            delete_target: None,
            filter_mode: false,
            filter_query: String::new(),
            preview_scroll: 0,
            list_height: 10,
            error: None,
            require_selection: options.require_selection,
        };
        state.refresh_filtered();
        state
    }

    fn refresh_filtered(&mut self) {
        self.filtered.clear();
        for (idx, task) in self.tasks.iter().enumerate() {
            if !self.show_all && task.is_closed() {
                continue;
            }
            if !self.filter_query.trim().is_empty() {
                let query = self.filter_query.to_lowercase();
                let haystack = format!("{} {}", task.title, task.id).to_lowercase();
                if !haystack.contains(&query) {
                    continue;
                }
            }
            self.filtered.push(idx);
        }

        if self.filtered.is_empty() {
            self.list_state.select(None);
        } else if let Some(selected) = self.list_state.selected() {
            if selected >= self.filtered.len() {
                self.list_state.select(Some(self.filtered.len() - 1));
            }
        } else {
            self.list_state.select(Some(0));
        }

        self.preview_scroll = 0;
    }

    fn selected_task_index(&self) -> Option<usize> {
        self.list_state
            .selected()
            .and_then(|idx| self.filtered.get(idx).copied())
    }

    fn selected_task(&self) -> Option<&Task> {
        self.selected_task_index()
            .and_then(|idx| self.tasks.get(idx))
    }

    fn move_selection(&mut self, delta: i32) {
        if self.filtered.is_empty() {
            return;
        }
        let max_index = self.filtered.len() as i32 - 1;
        let current = self.list_state.selected().unwrap_or(0) as i32;
        let next = min(max_index, (current + delta).max(0)) as usize;
        self.list_state.select(Some(next));
        self.preview_scroll = 0;
    }

    fn page_selection(&mut self, direction: i32) {
        let delta = (self.list_height as i32).saturating_sub(1).max(1) * direction;
        self.move_selection(delta);
    }
}

#[derive(Clone, Copy, Default)]
pub struct PickerOptions {
    pub require_selection: bool,
}

pub fn run_picker(
    tasks: &[Task],
    git_root: &Path,
    options: PickerOptions,
) -> Result<Option<PathBuf>> {
    let mut state = TuiState::new(tasks.to_vec(), options);

    let mut terminal = setup_terminal()?;

    let mut selected: Option<PathBuf> = None;
    let result = run_loop(&mut terminal, &mut state, git_root, &mut selected);

    restore_terminal(&mut terminal)?;

    result?;
    Ok(selected)
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut TuiState,
    git_root: &Path,
    selected: &mut Option<PathBuf>,
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

            let header = render_header(state, layout[0]);
            frame.render_widget(header, layout[0]);

            render_body(frame, state, layout[1]);

            let footer = render_footer(state, layout[2]);
            frame.render_widget(footer, layout[2]);
        })?;

        if event::poll(std::time::Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if handle_key(state, key, git_root, selected)? {
                    return Ok(());
                }
            }
        }
    }
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

fn handle_key(
    state: &mut TuiState,
    key: KeyEvent,
    git_root: &Path,
    selected: &mut Option<PathBuf>,
) -> Result<bool> {
    if state.confirm_delete {
        return handle_confirm_delete(state, key, git_root);
    }

    if state.show_help {
        match key.code {
            KeyCode::Char('?') | KeyCode::Esc => {
                state.show_help = false;
            }
            _ => {}
        }
        return Ok(false);
    }

    if state.filter_mode {
        return handle_filter_input(state, key);
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('c') => {
                if state.require_selection {
                    state.error = Some("Select a task to continue".to_string());
                } else {
                    return Ok(true);
                }
            }
            KeyCode::Char('d') | KeyCode::PageDown => {
                state.page_selection(1);
                return Ok(false);
            }
            KeyCode::Char('u') | KeyCode::PageUp => {
                state.page_selection(-1);
                return Ok(false);
            }
            KeyCode::Char('j') => {
                adjust_priority(state, -1);
                return Ok(false);
            }
            KeyCode::Char('k') => {
                adjust_priority(state, 1);
                return Ok(false);
            }
            KeyCode::Char('f') => {
                state.preview_scroll = state.preview_scroll.saturating_add(10);
                return Ok(false);
            }
            KeyCode::Char('b') => {
                state.preview_scroll = state.preview_scroll.saturating_sub(10);
                return Ok(false);
            }
            _ => {}
        }
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => {
            if state.require_selection {
                state.error = Some("Select a task to continue".to_string());
            } else {
                return Ok(true);
            }
        }
        KeyCode::Char('?') => state.show_help = true,
        KeyCode::Enter => {
            if let Some(task) = state.selected_task() {
                *selected = Some(task.path.clone());
                return Ok(true);
            }
        }
        KeyCode::Char('j') | KeyCode::Down => state.move_selection(1),
        KeyCode::Char('k') | KeyCode::Up => state.move_selection(-1),
        KeyCode::Char('g') => {
            state.list_state.select(Some(0));
            state.preview_scroll = 0;
        }
        KeyCode::Char('G') => {
            if !state.filtered.is_empty() {
                state.list_state.select(Some(state.filtered.len() - 1));
                state.preview_scroll = 0;
            }
        }
        KeyCode::Char('a') => {
            state.show_all = !state.show_all;
            state.refresh_filtered();
        }
        KeyCode::Char('p') => state.preview_on = !state.preview_on,
        KeyCode::Char('n') => run_create_task(state, git_root)?,
        KeyCode::Char('e') => run_edit_task(state, git_root)?,
        KeyCode::Char('d') | KeyCode::Char('x') => {
            if state.selected_task().is_some() {
                state.confirm_delete = true;
                state.delete_target = state.selected_task_index();
            }
        }
        KeyCode::Tab => {
            if let Some(task_index) = state.selected_task_index() {
                let task = &mut state.tasks[task_index];
                if let Err(err) = store::toggle_task_status(task) {
                    state.error = Some(err.to_string());
                } else if !state.show_all && task.is_closed() {
                    state.refresh_filtered();
                }
            }
        }
        KeyCode::Char('/') => {
            state.filter_mode = true;
            state.filter_query.clear();
        }
        KeyCode::Char('J') => {
            state.preview_scroll = state.preview_scroll.saturating_add(3);
        }
        KeyCode::Char('K') => {
            state.preview_scroll = state.preview_scroll.saturating_sub(3);
        }
        KeyCode::PageDown => state.page_selection(1),
        KeyCode::PageUp => state.page_selection(-1),
        _ => {}
    }

    Ok(false)
}

fn handle_filter_input(state: &mut TuiState, key: KeyEvent) -> Result<bool> {
    match key {
        KeyEvent {
            code: KeyCode::Esc, ..
        } => {
            state.filter_mode = false;
            state.filter_query.clear();
            state.refresh_filtered();
        }
        KeyEvent {
            code: KeyCode::Enter,
            ..
        } => {
            state.filter_mode = false;
            state.refresh_filtered();
        }
        KeyEvent {
            code: KeyCode::Backspace,
            ..
        } => {
            state.filter_query.pop();
            state.refresh_filtered();
        }
        KeyEvent {
            code: KeyCode::Char(ch),
            ..
        } => {
            state.filter_query.push(ch);
            state.refresh_filtered();
        }
        _ => {}
    }
    Ok(false)
}

fn handle_confirm_delete(state: &mut TuiState, key: KeyEvent, git_root: &Path) -> Result<bool> {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            if let Some(index) = state.delete_target {
                let task = state.tasks.get(index).cloned();
                if let Some(task) = task {
                    if let Err(err) = store::delete_task(&task) {
                        state.error = Some(err.to_string());
                    } else {
                        reload_tasks(state, git_root)?;
                    }
                }
            }
            state.confirm_delete = false;
            state.delete_target = None;
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            state.confirm_delete = false;
            state.delete_target = None;
        }
        _ => {}
    }
    Ok(false)
}

fn adjust_priority(state: &mut TuiState, delta: i32) {
    if let Some(index) = state.selected_task_index() {
        if let Some(task) = state.tasks.get_mut(index) {
            if let Err(err) = store::change_task_priority(task, delta) {
                state.error = Some(err.to_string());
            }
        }
    }
}

fn run_create_task(state: &mut TuiState, git_root: &Path) -> Result<()> {
    let mut terminal_guard = TerminalGuard::enter()?;
    let status = std::process::Command::new("task")
        .args(["create", "--edit"])
        .status()
        .context("failed to launch task creation")?;
    if !status.success() {
        state.error = Some("task creation failed".to_string());
    }
    terminal_guard.restore()?;
    reload_tasks(state, git_root)?;
    Ok(())
}

fn run_edit_task(state: &mut TuiState, git_root: &Path) -> Result<()> {
    let Some(task) = state.selected_task() else {
        return Ok(());
    };
    let mut terminal_guard = TerminalGuard::enter()?;
    if let Err(err) = store::open_editor(&task.path) {
        state.error = Some(err.to_string());
    }
    terminal_guard.restore()?;
    reload_tasks(state, git_root)?;
    Ok(())
}

fn reload_tasks(state: &mut TuiState, git_root: &Path) -> Result<()> {
    let tasks = store::load_tasks(git_root)?;
    state.tasks = tasks;
    sort_tasks(&mut state.tasks);
    state.refresh_filtered();
    Ok(())
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        disable_raw_mode().ok();
        let mut stdout = io::stdout();
        execute!(stdout, LeaveAlternateScreen).ok();
        Ok(Self {
            terminal: Terminal::new(CrosstermBackend::new(stdout))?,
        })
    }

    fn restore(&mut self) -> Result<()> {
        enable_raw_mode().ok();
        execute!(self.terminal.backend_mut(), EnterAlternateScreen).ok();
        Ok(())
    }
}

fn render_header(state: &TuiState, area: Rect) -> Paragraph<'_> {
    let open_count = state.tasks.iter().filter(|task| !task.is_closed()).count();
    let closed_count = state.tasks.len().saturating_sub(open_count);
    let filter_mode = if state.show_all { "all" } else { "open only" };

    let title = format!("Tasks ({open_count} open, {closed_count} closed) [{filter_mode}]");
    let help_hint = if state.require_selection {
        "? help"
    } else {
        "? help  q quit"
    };

    let mut line = title.clone();
    if area.width as usize > title.len() + help_hint.len() + 2 {
        let padding = area.width as usize - title.len() - help_hint.len() - 2;
        line = format!("{title}{} {help_hint}", " ".repeat(padding));
    }

    Paragraph::new(line).style(Style::default().fg(Color::Cyan))
}

fn render_body(frame: &mut ratatui::Frame<'_>, state: &mut TuiState, area: Rect) {
    if state.show_help {
        let help = render_help(state);
        frame.render_widget(help, area);
        return;
    }

    if state.confirm_delete {
        let confirm = render_confirm(state, area);
        frame.render_widget(confirm, area);
        return;
    }

    let show_preview = state.preview_on && area.width >= 120;
    if show_preview {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);
        state.list_height = chunks[0].height.saturating_sub(2) as usize;
        let list = render_list(state);
        frame.render_stateful_widget(list, chunks[0], &mut state.list_state);
        let preview = render_preview(state);
        frame.render_widget(preview, chunks[1]);
    } else {
        state.list_height = area.height.saturating_sub(2) as usize;
        let list = render_list(state);
        frame.render_stateful_widget(list, area, &mut state.list_state);
    }
}

fn render_list(state: &TuiState) -> List<'static> {
    let items = build_list_items(state);

    let highlight = Style::default()
        .fg(Color::Green)
        .add_modifier(Modifier::BOLD);
    let list = List::new(items)
        .block(Block::default().borders(Borders::NONE))
        .highlight_style(highlight)
        .highlight_symbol("> ");

    list
}

fn build_list_items(state: &TuiState) -> Vec<ListItem<'static>> {
    let blockers = state
        .tasks
        .iter()
        .map(|task| {
            let ids: Vec<String> = task
                .blockers(&state.tasks)
                .into_iter()
                .map(|task| task.id.clone())
                .collect();
            ids
        })
        .collect::<Vec<_>>();

    state
        .filtered
        .iter()
        .map(|idx| {
            let task = &state.tasks[*idx];
            let priority_style = match task.priority {
                5 => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                4 => Style::default().fg(Color::Yellow),
                3 => Style::default().fg(Color::White),
                2 => Style::default().fg(Color::DarkGray),
                _ => Style::default().fg(Color::DarkGray),
            };

            let mut spans = Vec::new();
            spans.push(Span::styled(
                format!("[P{}] ", task.priority),
                priority_style,
            ));

            let title_style = if task.is_closed() {
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::CROSSED_OUT)
            } else {
                Style::default().fg(Color::White)
            };
            spans.push(Span::styled(task.title.clone(), title_style));

            if state.show_all && task.is_closed() {
                spans.push(Span::styled(
                    " [closed]",
                    Style::default().fg(Color::DarkGray),
                ));
            }

            let blocker_ids = blockers.get(*idx).cloned().unwrap_or_default();
            if !blocker_ids.is_empty() {
                spans.push(Span::styled(
                    format!(" (blocked by {})", blocker_ids.join(", ")),
                    Style::default().fg(Color::Yellow),
                ));
            }

            ListItem::new(Line::from(spans))
        })
        .collect()
}

fn render_preview(state: &TuiState) -> Paragraph<'_> {
    let content = if let Some(task) = state.selected_task() {
        match crate::crank_io::read_to_string(&task.path) {
            Ok(content) => content,
            Err(err) => format!("Error reading file: {err}"),
        }
    } else {
        "No task selected".to_string()
    };

    Paragraph::new(Text::from(content))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .wrap(Wrap { trim: false })
        .scroll((state.preview_scroll, 0))
}

fn render_footer(state: &TuiState, _area: Rect) -> Paragraph<'_> {
    let (footer, style) = if let Some(err) = &state.error {
        (format!("Error: {err}"), Style::default().fg(Color::Red))
    } else if state.filter_mode {
        (
            format!("/{}", state.filter_query),
            Style::default().fg(Color::DarkGray),
        )
    } else if state.confirm_delete {
        (
            "Delete task? y confirm, n cancel".to_string(),
            Style::default().fg(Color::DarkGray),
        )
    } else if state.show_help {
        (
            "Press ? or esc to close help".to_string(),
            Style::default().fg(Color::DarkGray),
        )
    } else {
        let base =
            "j/k list  J/K preview  enter select  tab toggle  n new  e edit  d delete  a all  / filter  ? help";
        let footer = if state.require_selection {
            base.to_string()
        } else {
            format!("{base}  q quit")
        };
        (footer, Style::default().fg(Color::DarkGray))
    };

    Paragraph::new(footer)
        .style(style)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::NONE))
}

fn render_help(state: &TuiState) -> Paragraph<'static> {
    let mut lines = vec![
        "Keyboard Shortcuts",
        "",
        "j / k / arrows  Move up/down in list",
        "ctrl+u / ctrl+d  Page up/down in list",
        "g / G            Go to top/bottom of list",
        "J / K            Scroll preview up/down",
        "ctrl+k / ctrl+j  Increase/decrease priority",
        "enter            Select task and continue",
        "tab              Toggle status (open/closed)",
        "n                New task",
        "e                Edit task",
        "d / x            Delete task (with confirmation)",
        "a                Toggle show all/open only",
        "p                Toggle preview pane",
        "/                Start filtering",
        "?                Toggle this help",
    ];
    if state.require_selection {
        lines.push("esc              Clear filter");
    } else {
        lines.push("esc              Clear filter / quit");
        lines.push("q                Quit");
    }

    let text = lines.join("\n");
    Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title("Help"))
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(Color::Cyan))
}

fn render_confirm(state: &TuiState, _area: Rect) -> Paragraph<'static> {
    let mut lines = Vec::new();
    lines.push("Delete Task?".to_string());
    lines.push("".to_string());

    if let Some(task) = state.selected_task() {
        lines.push(format!("  Title:    {}", task.title));
        lines.push(format!("  Priority: P{}", task.priority));
        lines.push(format!("  Status:   {}", task.status));
        lines.push(format!("  File:     .crank/{}.md", task.id));
    }

    lines.push("".to_string());
    lines.push("This will permanently delete the task file.".to_string());
    lines.push("".to_string());
    lines.push("[y] confirm, [n/esc] cancel".to_string());

    Paragraph::new(lines.join("\n"))
        .block(Block::default().borders(Borders::ALL).title("Confirm"))
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(Color::Red))
}
