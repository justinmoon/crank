use std::cmp::min;
use std::collections::HashMap;
use std::io::{self, Stdout};
use std::path::Path;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Terminal;

use crate::task::store;
use crate::tutorial::{
    load_full_tutorial, load_index, set_tutorial_status, TutorialFull, TutorialIndexEntry,
};

pub fn run_inbox(repo_root: &Path) -> Result<()> {
    let entries = load_index(repo_root)?;
    if entries.is_empty() {
        println!("No tutorials found.");
        return Ok(());
    }

    let mut state = InboxState::new(entries);
    let mut terminal = setup_terminal()?;
    let result = run_loop(&mut terminal, &mut state, repo_root);
    restore_terminal(&mut terminal)?;
    result
}

struct InboxState {
    entries: Vec<TutorialIndexEntry>,
    filtered: Vec<usize>,
    list_state: ListState,
    filter_mode: bool,
    filter_query: String,
    preview_scroll: u16,
    list_height: usize,
    show_help: bool,
    error: Option<String>,
    summary_cache: HashMap<String, String>,
}

impl InboxState {
    fn new(entries: Vec<TutorialIndexEntry>) -> Self {
        let mut state = Self {
            entries,
            filtered: Vec::new(),
            list_state: ListState::default(),
            filter_mode: false,
            filter_query: String::new(),
            preview_scroll: 0,
            list_height: 10,
            show_help: false,
            error: None,
            summary_cache: HashMap::new(),
        };
        state.refresh_filtered();
        state
    }

    fn refresh_filtered(&mut self) {
        self.filtered.clear();
        for (idx, entry) in self.entries.iter().enumerate() {
            if !self.filter_query.trim().is_empty() {
                let query = self.filter_query.to_lowercase();
                let haystack = format!("{} {} {}", entry.title, entry.id, entry.source_branch)
                    .to_lowercase();
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

    fn selected_index(&self) -> Option<usize> {
        self.list_state
            .selected()
            .and_then(|idx| self.filtered.get(idx).copied())
    }

    fn selected_entry(&self) -> Option<&TutorialIndexEntry> {
        self.selected_index()
            .and_then(|idx| self.entries.get(idx))
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

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut InboxState,
    repo_root: &Path,
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

            render_body(frame, state, repo_root, layout[1]);

            let footer = render_footer(state, layout[2]);
            frame.render_widget(footer, layout[2]);
        })?;

        if event::poll(std::time::Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if handle_key(state, key, repo_root)? {
                    return Ok(());
                }
            }
        }
    }
}

fn handle_key(state: &mut InboxState, key: KeyEvent, repo_root: &Path) -> Result<bool> {
    if state.show_help {
        match key.code {
            KeyCode::Char('?') | KeyCode::Esc => state.show_help = false,
            _ => {}
        }
        return Ok(false);
    }

    if state.filter_mode {
        return handle_filter_input(state, key);
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('c') => return Ok(true),
            KeyCode::Char('d') | KeyCode::PageDown => {
                state.page_selection(1);
                return Ok(false);
            }
            KeyCode::Char('u') | KeyCode::PageUp => {
                state.page_selection(-1);
                return Ok(false);
            }
            _ => {}
        }
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
        KeyCode::Char('?') => state.show_help = true,
        KeyCode::Enter => {
            if let Some(entry) = state.selected_entry().cloned() {
                if entry.status != "read" {
                    if let Err(err) = set_tutorial_status(repo_root, &entry.id, "read") {
                        state.error = Some(err.to_string());
                    } else if let Some(sel) = state.selected_index() {
                        if let Some(item) = state.entries.get_mut(sel) {
                            item.status = "read".to_string();
                        }
                    }
                }
                run_viewer(repo_root, &entry.id)?;
                reload_entries(state, repo_root)?;
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
        KeyCode::Char('/') => {
            state.filter_mode = true;
            state.filter_query.clear();
        }
        KeyCode::Char('r') => toggle_read(state, repo_root),
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

fn handle_filter_input(state: &mut InboxState, key: KeyEvent) -> Result<bool> {
    match key.code {
        KeyCode::Esc => {
            state.filter_mode = false;
            state.filter_query.clear();
            state.refresh_filtered();
        }
        KeyCode::Enter => {
            state.filter_mode = false;
            state.refresh_filtered();
        }
        KeyCode::Backspace => {
            state.filter_query.pop();
            state.refresh_filtered();
        }
        KeyCode::Char(ch) => {
            state.filter_query.push(ch);
            state.refresh_filtered();
        }
        _ => {}
    }
    Ok(false)
}

fn toggle_read(state: &mut InboxState, repo_root: &Path) {
    if let Some(entry) = state.selected_entry().cloned() {
        let status = if entry.status == "read" {
            "unread"
        } else {
            "read"
        };
        if let Err(err) = set_tutorial_status(repo_root, &entry.id, status) {
            state.error = Some(err.to_string());
        } else if let Some(sel) = state.selected_index() {
            if let Some(item) = state.entries.get_mut(sel) {
                item.status = status.to_string();
            }
        }
    }
}

fn reload_entries(state: &mut InboxState, repo_root: &Path) -> Result<()> {
    let entries = load_index(repo_root)?;
    state.entries = entries;
    state.refresh_filtered();
    Ok(())
}

fn render_header(state: &InboxState, area: Rect) -> Paragraph<'_> {
    let count = state.entries.len();
    let unread = state
        .entries
        .iter()
        .filter(|entry| entry.status != "read")
        .count();
    let title = format!("Inbox ({count} total, {unread} unread)");
    let help_hint = "? help  q quit";

    let mut line = title.clone();
    if area.width as usize > title.len() + help_hint.len() + 2 {
        let padding = area.width as usize - title.len() - help_hint.len() - 2;
        line = format!("{title}{} {help_hint}", " ".repeat(padding));
    }

    Paragraph::new(line).style(Style::default().fg(Color::Cyan))
}

fn render_body(
    frame: &mut ratatui::Frame<'_>,
    state: &mut InboxState,
    repo_root: &Path,
    area: Rect,
) {
    if state.show_help {
        let help = render_help(area);
        frame.render_widget(help, area);
        return;
    }

    let show_preview = area.width >= 120;
    if show_preview {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(area);
        state.list_height = chunks[0].height.saturating_sub(2) as usize;
        let list = render_list(state);
        frame.render_stateful_widget(list, chunks[0], &mut state.list_state);
        let preview = render_preview(state, repo_root, chunks[1]);
        frame.render_widget(preview, chunks[1]);
    } else {
        state.list_height = area.height.saturating_sub(2) as usize;
        let list = render_list(state);
        frame.render_stateful_widget(list, area, &mut state.list_state);
    }
}

fn render_list(state: &InboxState) -> List<'static> {
    let items = build_list_items(state);
    let highlight = Style::default()
        .fg(Color::Green)
        .add_modifier(Modifier::BOLD);
    List::new(items)
        .block(Block::default().borders(Borders::NONE))
        .highlight_style(highlight)
        .highlight_symbol("> ")
}

fn build_list_items(state: &InboxState) -> Vec<ListItem<'static>> {
    state
        .filtered
        .iter()
        .map(|idx| {
            let entry = &state.entries[*idx];
            let marker = if entry.status == "read" { " " } else { "*" };
            let title = if entry.title.trim().is_empty() {
                "(untitled)".to_string()
            } else {
                entry.title.clone()
            };
            let line = format!(
                "{marker} {title} ({})",
                entry.created_at.split_whitespace().next().unwrap_or("")
            );
            ListItem::new(Line::from(line))
        })
        .collect()
}

fn render_preview(state: &mut InboxState, repo_root: &Path, _area: Rect) -> Paragraph<'static> {
    let mut lines = Vec::new();
    if let Some(entry) = state.selected_entry().cloned() {
        lines.push(format!("Title: {}", entry.title));
        lines.push(format!("ID: {}", entry.id));
        lines.push(format!("Status: {}", entry.status));
        lines.push(format!("Source: {}", entry.source_branch));
        lines.push(format!("Base: {}", entry.base_branch));
        lines.push(format!("Merge: {}", entry.merge_commit));
        lines.push(format!("Steps: {}", entry.steps));
        lines.push(String::new());

        let summary = load_summary_cached(state, repo_root, &entry.id);
        if !summary.trim().is_empty() {
            lines.push("Summary:".to_string());
            lines.extend(summary.lines().take(8).map(|line| line.to_string()));
        }
    } else {
        lines.push("No tutorials.".to_string());
    }

    let text = Text::from(lines.join("\n"));
    Paragraph::new(text)
        .block(Block::default().borders(Borders::NONE))
        .wrap(Wrap { trim: false })
        .scroll((state.preview_scroll, 0))
}

fn render_help(_area: Rect) -> Paragraph<'static> {
    let lines = vec![
        "Keys:",
        "  j/k or arrows: move",
        "  enter: open tutorial",
        "  r: mark read/unread",
        "  /: filter",
        "  q: quit",
        "  ?: close help",
    ];
    let text = Text::from(lines.join("\n"));
    Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title("Help"))
        .wrap(Wrap { trim: false })
        .scroll((0, 0))
        .style(Style::default().fg(Color::White))
}

fn render_footer(state: &InboxState, _area: Rect) -> Paragraph<'static> {
    let hint = if let Some(error) = &state.error {
        format!("Error: {error}")
    } else if state.filter_mode {
        format!("Filter: {}", state.filter_query)
    } else {
        "enter open  r read/unread  / filter".to_string()
    };
    Paragraph::new(hint)
        .style(Style::default().fg(Color::DarkGray))
        .wrap(Wrap { trim: true })
}

fn load_summary_cached(
    state: &mut InboxState,
    repo_root: &Path,
    id: &str,
) -> String {
    if let Some(summary) = state.summary_cache.get(id) {
        return summary.clone();
    }
    let summary_path = repo_root.join(".crank").join("tutorials").join(id).join("summary.md");
    let summary = crate::crank_io::read_to_string(&summary_path).unwrap_or_default();
    state.summary_cache.insert(id.to_string(), summary.clone());
    summary
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

fn run_viewer(repo_root: &Path, id: &str) -> Result<()> {
    let full = load_full_tutorial(repo_root, id)?;
    let mut state = ViewerState::new(full);
    let mut terminal = setup_terminal()?;
    let result = run_viewer_loop(&mut terminal, &mut state, repo_root);
    restore_terminal(&mut terminal)?;
    result
}

struct ViewerState {
    tutorial: TutorialFull,
    list_state: ListState,
    note_scroll: u16,
    list_height: usize,
    show_help: bool,
    error: Option<String>,
}

impl ViewerState {
    fn new(tutorial: TutorialFull) -> Self {
        let mut state = Self {
            tutorial,
            list_state: ListState::default(),
            note_scroll: 0,
            list_height: 10,
            show_help: false,
            error: None,
        };
        if !state.tutorial.steps.is_empty() {
            state.list_state.select(Some(0));
        }
        state
    }

    fn selected_step_index(&self) -> Option<usize> {
        self.list_state.selected()
    }

    fn selected_step(&self) -> Option<&crate::tutorial::TutorialStepContent> {
        self.selected_step_index()
            .and_then(|idx| self.tutorial.steps.get(idx))
    }

    fn move_selection(&mut self, delta: i32) {
        if self.tutorial.steps.is_empty() {
            return;
        }
        let max_index = self.tutorial.steps.len() as i32 - 1;
        let current = self.list_state.selected().unwrap_or(0) as i32;
        let next = min(max_index, (current + delta).max(0)) as usize;
        self.list_state.select(Some(next));
        self.note_scroll = 0;
    }

    fn page_selection(&mut self, direction: i32) {
        let delta = (self.list_height as i32).saturating_sub(1).max(1) * direction;
        self.move_selection(delta);
    }
}

fn run_viewer_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut ViewerState,
    repo_root: &Path,
) -> Result<()> {
    loop {
        terminal.draw(|frame| {
            let size = frame.area();
            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(7),
                    Constraint::Min(1),
                    Constraint::Length(2),
                ])
                .split(size);

            let header = render_viewer_header(state, layout[0]);
            frame.render_widget(header, layout[0]);

            render_viewer_body(frame, state, layout[1]);

            let footer = render_viewer_footer(state, layout[2]);
            frame.render_widget(footer, layout[2]);
        })?;

        if event::poll(std::time::Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if handle_viewer_key(state, key, repo_root)? {
                    return Ok(());
                }
            }
        }
    }
}

fn handle_viewer_key(state: &mut ViewerState, key: KeyEvent, repo_root: &Path) -> Result<bool> {
    if state.show_help {
        match key.code {
            KeyCode::Char('?') | KeyCode::Esc => state.show_help = false,
            _ => {}
        }
        return Ok(false);
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('c') => return Ok(true),
            KeyCode::Char('d') | KeyCode::PageDown => {
                state.page_selection(1);
                return Ok(false);
            }
            KeyCode::Char('u') | KeyCode::PageUp => {
                state.page_selection(-1);
                return Ok(false);
            }
            _ => {}
        }
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
        KeyCode::Char('?') => state.show_help = true,
        KeyCode::Char('j') | KeyCode::Down => state.move_selection(1),
        KeyCode::Char('k') | KeyCode::Up => state.move_selection(-1),
        KeyCode::Char('g') => {
            state.list_state.select(Some(0));
            state.note_scroll = 0;
        }
        KeyCode::Char('G') => {
            if !state.tutorial.steps.is_empty() {
                state
                    .list_state
                    .select(Some(state.tutorial.steps.len() - 1));
                state.note_scroll = 0;
            }
        }
        KeyCode::Char('J') => state.note_scroll = state.note_scroll.saturating_add(3),
        KeyCode::Char('K') => state.note_scroll = state.note_scroll.saturating_sub(3),
        KeyCode::Char('d') => {
            if let Some(step) = state.selected_step() {
                let diff = step.step.diff.clone();
                open_diff(repo_root, &diff, state)?;
            }
        }
        KeyCode::Char('r') => {
            let status = if state.tutorial.manifest.status == "read" {
                "unread"
            } else {
                "read"
            };
            if let Err(err) = set_tutorial_status(repo_root, &state.tutorial.manifest.id, status) {
                state.error = Some(err.to_string());
            } else {
                state.tutorial.manifest.status = status.to_string();
            }
        }
        KeyCode::PageDown => state.page_selection(1),
        KeyCode::PageUp => state.page_selection(-1),
        _ => {}
    }

    Ok(false)
}

fn render_viewer_header(state: &ViewerState, _area: Rect) -> Paragraph<'static> {
    let title = format!(
        "{} [{}]",
        state.tutorial.manifest.title, state.tutorial.manifest.status
    );
    let mut lines = vec![title, String::new()];
    let issue = trim_text(&state.tutorial.issue, 3);
    let summary = trim_text(&state.tutorial.summary, 3);
    if !issue.is_empty() {
        lines.push("Issue:".to_string());
        lines.extend(issue);
    }
    if !summary.is_empty() {
        lines.push(String::new());
        lines.push("Summary:".to_string());
        lines.extend(summary);
    }

    Paragraph::new(lines.join("\n"))
        .block(Block::default().borders(Borders::NONE))
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(Color::Cyan))
}

fn render_viewer_body(frame: &mut ratatui::Frame<'_>, state: &mut ViewerState, area: Rect) {
    if state.show_help {
        let help = render_viewer_help(area);
        frame.render_widget(help, area);
        return;
    }

    let show_preview = area.width >= 120;
    if show_preview {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(area);
        state.list_height = chunks[0].height.saturating_sub(2) as usize;
        let list = render_step_list(state);
        frame.render_stateful_widget(list, chunks[0], &mut state.list_state);
        let note = render_step_note(state, chunks[1]);
        frame.render_widget(note, chunks[1]);
    } else {
        state.list_height = area.height.saturating_sub(2) as usize;
        let list = render_step_list(state);
        frame.render_stateful_widget(list, area, &mut state.list_state);
    }
}

fn render_viewer_footer(state: &ViewerState, _area: Rect) -> Paragraph<'static> {
    let hint = if let Some(error) = &state.error {
        format!("Error: {error}")
    } else {
        "d diff  r read/unread  q quit".to_string()
    };
    Paragraph::new(hint)
        .style(Style::default().fg(Color::DarkGray))
        .wrap(Wrap { trim: true })
}

fn render_step_list(state: &ViewerState) -> List<'static> {
    let items: Vec<ListItem> = state
        .tutorial
        .steps
        .iter()
        .map(|step| {
            let line = format!("Step {:02}: {}", step.step.index, step.step.subject);
            ListItem::new(Line::from(line))
        })
        .collect();

    let highlight = Style::default()
        .fg(Color::Green)
        .add_modifier(Modifier::BOLD);
    List::new(items)
        .block(Block::default().borders(Borders::NONE))
        .highlight_style(highlight)
        .highlight_symbol("> ")
}

fn render_step_note(state: &ViewerState, _area: Rect) -> Paragraph<'static> {
    let text = if let Some(step) = state.selected_step() {
        step.note.clone()
    } else {
        "No steps.".to_string()
    };
    Paragraph::new(text)
        .block(Block::default().borders(Borders::NONE))
        .wrap(Wrap { trim: false })
        .scroll((state.note_scroll, 0))
}

fn render_viewer_help(_area: Rect) -> Paragraph<'static> {
    let lines = vec![
        "Keys:",
        "  j/k or arrows: move",
        "  d: open diff in $EDITOR",
        "  r: mark read/unread",
        "  q: quit",
        "  ?: close help",
    ];
    let text = Text::from(lines.join("\n"));
    Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title("Help"))
        .wrap(Wrap { trim: false })
}

fn open_diff(repo_root: &Path, diff_rel: &str, state: &mut ViewerState) -> Result<()> {
    let path = repo_root
        .join(".crank")
        .join("tutorials")
        .join(&state.tutorial.manifest.id)
        .join(diff_rel);
    let mut guard = TerminalGuard::enter()?;
    if let Err(err) = store::open_editor(&path) {
        state.error = Some(err.to_string());
    }
    guard.restore()?;
    Ok(())
}

fn trim_text(text: &str, max_lines: usize) -> Vec<String> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .take(max_lines)
        .map(|line| line.to_string())
        .collect()
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
