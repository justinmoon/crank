use std::cmp::min;
use std::collections::HashMap;
use std::io::{self, Read, Stdout, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
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

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use pulldown_cmark::{
    Event as MdEvent, Options as MdOptions, Parser as MdParser, Tag as MdTag, TagEnd as MdTagEnd,
};
use vt100::{Cell as VtCell, Color as VtColor, Parser as VtParser};

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
                let haystack =
                    format!("{} {} {}", entry.title, entry.id, entry.source_branch).to_lowercase();
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
        self.selected_index().and_then(|idx| self.entries.get(idx))
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
    let lines = [
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

fn load_summary_cached(state: &mut InboxState, repo_root: &Path, id: &str) -> String {
    if let Some(summary) = state.summary_cache.get(id) {
        return summary.clone();
    }
    let summary_path = repo_root
        .join(".crank")
        .join("tutorials")
        .join(id)
        .join("summary.md");
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
    let mut terminal = setup_terminal()?;
    let size = terminal.size()?;
    let area = Rect::new(0, 0, size.width, size.height);
    let editor_cmd = EditorCommand::from_env();
    let mut state = ViewerState::new(full, repo_root, area, editor_cmd)?;
    let result = run_viewer_loop(&mut terminal, &mut state, repo_root);
    restore_terminal(&mut terminal)?;
    result
}

struct ViewerState {
    tutorial: TutorialFull,
    page: ViewerPage,
    focus: ViewerFocus,
    show_help: bool,
    error: Option<String>,
    editor: Option<EmbeddedEditor>,
    editor_cmd: Option<EditorCommand>,
    current_path: Option<PathBuf>,
    last_editor_size: (u16, u16),
    last_area: Rect,
    editor_snapshot: Text<'static>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewerFocus {
    Viewer,
    Editor,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewerPage {
    Overview,
    Step(usize),
}

impl ViewerState {
    fn new(
        tutorial: TutorialFull,
        repo_root: &Path,
        area: Rect,
        editor_cmd: Result<EditorCommand>,
    ) -> Result<Self> {
        let editor_cmd = match editor_cmd {
            Ok(cmd) => Some(cmd),
            Err(err) => {
                let mut state = Self {
                    tutorial,
                    page: ViewerPage::Overview,
                    focus: ViewerFocus::Viewer,
                    show_help: false,
                    error: Some(err.to_string()),
                    editor: None,
                    editor_cmd: None,
                    current_path: None,
                    last_editor_size: (0, 0),
                    last_area: area,
                    editor_snapshot: Text::from(""),
                };
                state.reset_step_index();
                return Ok(state);
            }
        };

        let mut state = Self {
            tutorial,
            page: ViewerPage::Overview,
            focus: ViewerFocus::Viewer,
            show_help: false,
            error: None,
            editor: None,
            editor_cmd,
            current_path: None,
            last_editor_size: (0, 0),
            last_area: area,
            editor_snapshot: Text::from(""),
        };
        state.reset_step_index();
        state.ensure_editor(repo_root, area)?;
        Ok(state)
    }

    fn reset_step_index(&mut self) {
        if let ViewerPage::Step(index) = self.page {
            if self.tutorial.steps.is_empty() {
                self.page = ViewerPage::Overview;
            } else if index >= self.tutorial.steps.len() {
                self.page = ViewerPage::Step(self.tutorial.steps.len() - 1);
            }
        }
    }

    fn current_step(&self) -> Option<&crate::tutorial::TutorialStepContent> {
        match self.page {
            ViewerPage::Step(index) => self.tutorial.steps.get(index),
            ViewerPage::Overview => None,
        }
    }

    fn move_step(&mut self, delta: i32) {
        if self.tutorial.steps.is_empty() {
            return;
        }
        match self.page {
            ViewerPage::Overview => {
                if delta > 0 {
                    self.page = ViewerPage::Step(0);
                }
            }
            ViewerPage::Step(index) => {
                let max = self.tutorial.steps.len() as i32 - 1;
                let next = (index as i32 + delta).max(-1).min(max);
                if next < 0 {
                    self.page = ViewerPage::Overview;
                    self.focus = ViewerFocus::Viewer;
                } else {
                    self.page = ViewerPage::Step(next as usize);
                }
            }
        }
    }

    fn ensure_editor(&mut self, repo_root: &Path, area: Rect) -> Result<()> {
        if self.tutorial.steps.is_empty() {
            self.editor = None;
            return Ok(());
        }

        if self.editor_cmd.is_none() {
            return Ok(());
        }

        if !matches!(self.page, ViewerPage::Step(_)) {
            self.editor = None;
            return Ok(());
        }

        self.sync_editor(repo_root, area)?;
        Ok(())
    }

    fn current_diff_path(&self, repo_root: &Path) -> PathBuf {
        let rel = self
            .current_step()
            .map(|step| step.step.diff.clone())
            .unwrap_or_default();
        repo_root
            .join(".crank")
            .join("tutorials")
            .join(&self.tutorial.manifest.id)
            .join(rel)
    }

    fn refresh_editor(&mut self, repo_root: &Path, area: Rect) -> Result<()> {
        if self.editor_cmd.is_none() || self.tutorial.steps.is_empty() {
            return Ok(());
        }
        if !matches!(self.page, ViewerPage::Step(_)) {
            self.editor = None;
            return Ok(());
        }
        self.sync_editor(repo_root, area)?;
        Ok(())
    }

    fn sync_editor(&mut self, repo_root: &Path, area: Rect) -> Result<()> {
        let Some(cmd) = self.editor_cmd.clone() else {
            return Ok(());
        };
        let layout = step_layout(area);
        let Some(editor_rect) = layout.editor else {
            return Ok(());
        };
        let inner = inner_rect(editor_rect);
        let rows = inner.height;
        let cols = inner.width;
        if rows == 0 || cols == 0 {
            return Ok(());
        }

        let diff_path = self.current_diff_path(repo_root);
        let needs_new = self.editor.is_none();

        if needs_new || cmd.adapter == EditorAdapter::Generic {
            if let Some(editor) = self.editor.as_mut() {
                self.editor_snapshot = editor.text();
            }
            self.editor = Some(EmbeddedEditor::new(&cmd, &diff_path, rows, cols)?);
            self.last_editor_size = (rows, cols);
            self.current_path = Some(diff_path);
            return Ok(());
        }

        if let Some(editor) = self.editor.as_mut() {
            if self.current_path.as_ref() != Some(&diff_path) {
                editor.open_file(cmd.adapter, &diff_path)?;
                self.current_path = Some(diff_path);
            }
            if (rows, cols) != self.last_editor_size {
                editor.resize(rows, cols)?;
                self.last_editor_size = (rows, cols);
            }
        }

        Ok(())
    }
}

fn run_viewer_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut ViewerState,
    repo_root: &Path,
) -> Result<()> {
    loop {
        let size = terminal.size()?;
        let area = Rect::new(0, 0, size.width, size.height);
        state.last_area = area;
        if let Some(editor) = state.editor.as_mut() {
            editor.drain();
        }

        terminal.draw(|frame| {
            let layout = match state.page {
                ViewerPage::Overview => overview_layout(area),
                ViewerPage::Step(_) => step_layout(area),
            };

            let header = render_viewer_header(state, &layout);
            frame.render_widget(header, layout.header);

            render_viewer_body(frame, state, &layout);

            let footer = render_viewer_footer(state, &layout);
            frame.render_widget(footer, layout.footer);
        })?;

        if event::poll(Duration::from_millis(80))? {
            if let Event::Key(key) = event::read()? {
                if handle_viewer_key(state, key, repo_root)? {
                    return Ok(());
                }
            } else if let Event::Resize(_, _) = event::read()? {
                // handled on next draw
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

    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Ok(true);
    }

    if is_focus_toggle(key) {
        if !matches!(state.page, ViewerPage::Step(_)) {
            state.error = Some("Editor is available on step pages only".to_string());
        } else if state.editor.is_none() {
            state.error = Some("Editor unavailable".to_string());
        } else {
            state.focus = if state.focus == ViewerFocus::Viewer {
                ViewerFocus::Editor
            } else {
                ViewerFocus::Viewer
            };
        }
        return Ok(false);
    }

    if state.focus == ViewerFocus::Editor {
        if let Some(editor) = state.editor.as_mut() {
            if let Some(bytes) = key_to_bytes(key) {
                editor.send_bytes(&bytes)?;
            }
        }
        return Ok(false);
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
        KeyCode::Char('?') => state.show_help = true,
        KeyCode::Char('h') | KeyCode::Left => {
            let prev = state.page;
            state.move_step(-1);
            if state.page != prev {
                state.refresh_editor(repo_root, state.last_area)?;
            }
        }
        KeyCode::Char('l') | KeyCode::Right => {
            let prev = state.page;
            state.move_step(1);
            if state.page != prev {
                state.refresh_editor(repo_root, state.last_area)?;
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
        _ => {}
    }

    Ok(false)
}

fn is_focus_toggle(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('t')
}

struct ViewerLayout {
    header: Rect,
    body: Rect,
    editor: Option<Rect>,
    footer: Rect,
}

fn overview_layout(area: Rect) -> ViewerLayout {
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(6),
            Constraint::Length(1),
        ])
        .split(area);

    ViewerLayout {
        header: parts[0],
        body: parts[1],
        editor: None,
        footer: parts[2],
    }
}

fn step_layout(area: Rect) -> ViewerLayout {
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Percentage(35),
            Constraint::Percentage(65),
            Constraint::Length(1),
        ])
        .split(area);

    ViewerLayout {
        header: parts[0],
        body: parts[1],
        editor: Some(parts[2]),
        footer: parts[3],
    }
}

fn render_viewer_header(state: &ViewerState, _layout: &ViewerLayout) -> Paragraph<'static> {
    let focus = if state.focus == ViewerFocus::Editor {
        "editor"
    } else {
        "viewer"
    };
    let title = match state.page {
        ViewerPage::Overview => format!("{} [overview] ({focus})", state.tutorial.manifest.title),
        ViewerPage::Step(index) => {
            let total = state.tutorial.steps.len().max(1);
            format!(
                "{} [step {}/{}] ({focus})",
                state.tutorial.manifest.title,
                index + 1,
                total
            )
        }
    };
    Paragraph::new(title)
        .block(Block::default().borders(Borders::NONE))
        .style(Style::default().fg(Color::Cyan))
        .wrap(Wrap { trim: true })
        .scroll((0, 0))
}

fn render_viewer_body(
    frame: &mut ratatui::Frame<'_>,
    state: &mut ViewerState,
    layout: &ViewerLayout,
) {
    if state.show_help {
        if let Some(editor_rect) = layout.editor {
            let help = render_viewer_help(layout);
            frame.render_widget(help, editor_rect);
        } else {
            let help = render_viewer_help(layout);
            frame.render_widget(help, layout.body);
        }
        return;
    }

    match state.page {
        ViewerPage::Overview => {
            let overview = render_issue_summary(state, layout, true);
            frame.render_widget(overview, layout.body);
        }
        ViewerPage::Step(_) => {
            if let Some(editor_rect) = layout.editor {
                let inner = inner_rect(editor_rect);
                if let Some(editor) = state.editor.as_mut() {
                    if (inner.height, inner.width) != state.last_editor_size
                        && editor.resize(inner.height, inner.width).is_ok()
                    {
                        state.last_editor_size = (inner.height, inner.width);
                    }
                }
            }

            let explanation =
                render_step_explanation(state, layout, state.focus == ViewerFocus::Viewer);
            frame.render_widget(explanation, layout.body);

            if let Some(editor_rect) = layout.editor {
                let editor = render_editor(state, layout, state.focus == ViewerFocus::Editor);
                frame.render_widget(editor, editor_rect);
            }
        }
    }
}

fn render_issue_summary(
    state: &ViewerState,
    layout: &ViewerLayout,
    focused: bool,
) -> Paragraph<'static> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let issue_text = markdown_to_text(&state.tutorial.issue);
    let summary_text = markdown_to_text(&state.tutorial.summary);

    if !issue_text.lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "Issue:",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.extend(limit_lines(
            &issue_text,
            layout.body.height.saturating_sub(1) as usize,
        ));
    }
    if !summary_text.lines.is_empty() {
        if !lines.is_empty() {
            lines.push(Line::from("".to_string()));
        }
        lines.push(Line::from(Span::styled(
            "Summary:",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.extend(limit_lines(
            &summary_text,
            layout.body.height.saturating_sub(1) as usize,
        ));
    }

    Paragraph::new(Text::from(lines))
        .block(focus_block("Overview", focused))
        .wrap(Wrap { trim: false })
}

fn render_step_explanation(
    state: &ViewerState,
    _layout: &ViewerLayout,
    focused: bool,
) -> Paragraph<'static> {
    let text = state
        .current_step()
        .map(|step| markdown_to_text(&step.note))
        .unwrap_or_else(|| Text::from("No steps."));
    Paragraph::new(text)
        .block(focus_block("Explanation", focused))
        .wrap(Wrap { trim: false })
        .scroll((0, 0))
}

fn render_editor(
    state: &mut ViewerState,
    _layout: &ViewerLayout,
    focused: bool,
) -> Paragraph<'static> {
    if state.editor.is_none() {
        let text = if state.tutorial.steps.is_empty() {
            "No diff available.".to_string()
        } else {
            "Editor unavailable. Set $EDITOR to enable diff view.".to_string()
        };
        return Paragraph::new(text)
            .block(focus_block("Diff", focused))
            .wrap(Wrap { trim: false });
    }

    let output = state
        .editor
        .as_mut()
        .map(|editor| editor.text())
        .unwrap_or_else(|| Text::from(""));

    if text_has_content(&output) {
        state.editor_snapshot = output.clone();
    }

    let text = if text_has_content(&output) {
        output
    } else if text_has_content(&state.editor_snapshot) {
        state.editor_snapshot.clone()
    } else {
        Text::from("Loading editor...")
    };

    Paragraph::new(text)
        .block(focus_block("Diff", focused))
        .scroll((0, 0))
}

fn focus_block<'a>(title: &'a str, focused: bool) -> Block<'a> {
    let color = if focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color))
        .title(title)
}

fn inner_rect(area: Rect) -> Rect {
    let width = area.width.saturating_sub(2);
    let height = area.height.saturating_sub(2);
    Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        width,
        height,
    )
}

fn render_viewer_footer(state: &ViewerState, _layout: &ViewerLayout) -> Paragraph<'static> {
    let hint = if let Some(error) = &state.error {
        format!("Error: {error}")
    } else if state.focus == ViewerFocus::Editor {
        "Ctrl-T to return to viewer".to_string()
    } else {
        "h/l pages  Ctrl-T focus editor  Esc quit".to_string()
    };
    Paragraph::new(hint)
        .style(Style::default().fg(Color::DarkGray))
        .wrap(Wrap { trim: true })
}

fn render_viewer_help(_layout: &ViewerLayout) -> Paragraph<'static> {
    let lines = [
        "Keys:",
        "  h/l or ←/→: navigate pages",
        "  Ctrl-T: toggle editor focus",
        "  Esc: exit (viewer focus)",
        "  ?: close help",
    ];
    let text = Text::from(lines.join("\n"));
    Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title("Help"))
        .wrap(Wrap { trim: false })
}

fn limit_lines(text: &Text<'static>, max_lines: usize) -> Vec<Line<'static>> {
    let mut lines = text.lines.clone();
    while lines.first().map(line_is_blank).unwrap_or(false) {
        lines.remove(0);
    }
    while lines.last().map(line_is_blank).unwrap_or(false) {
        lines.pop();
    }
    lines.into_iter().take(max_lines).collect()
}

fn line_is_blank(line: &Line<'static>) -> bool {
    line.spans.iter().all(|span| span.content.trim().is_empty())
}

fn markdown_to_text(input: &str) -> Text<'static> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut style_stack: Vec<Style> = vec![Style::default()];
    let mut list_stack: Vec<(bool, usize)> = Vec::new();
    let mut in_code_block = false;

    let push_line = |lines: &mut Vec<Line<'static>>, spans: &mut Vec<Span<'static>>| {
        if spans.is_empty() {
            lines.push(Line::from("".to_string()));
        } else {
            lines.push(Line::from(std::mem::take(spans)));
        }
    };

    for event in MdParser::new_ext(input, MdOptions::empty()) {
        match event {
            MdEvent::Start(MdTag::Heading { .. }) => {
                push_line(&mut lines, &mut spans);
                let mut style = current_style(&style_stack);
                style = style.add_modifier(Modifier::BOLD).fg(Color::Cyan);
                style_stack.push(style);
            }
            MdEvent::End(MdTagEnd::Heading(_)) => {
                push_line(&mut lines, &mut spans);
                lines.push(Line::from("".to_string()));
                style_stack.pop();
            }
            MdEvent::Start(MdTag::Paragraph) => {}
            MdEvent::End(MdTagEnd::Paragraph) => {
                push_line(&mut lines, &mut spans);
                lines.push(Line::from("".to_string()));
            }
            MdEvent::Start(MdTag::List(start)) => {
                let ordered = start.is_some();
                let index = start.unwrap_or(1) as usize;
                list_stack.push((ordered, index));
            }
            MdEvent::End(MdTagEnd::List(_)) => {
                list_stack.pop();
                lines.push(Line::from("".to_string()));
            }
            MdEvent::Start(MdTag::Item) => {
                push_line(&mut lines, &mut spans);
                let indent = "  ".repeat(list_stack.len().saturating_sub(1));
                if !indent.is_empty() {
                    spans.push(Span::styled(indent, current_style(&style_stack)));
                }
                if let Some((ordered, index)) = list_stack.last_mut() {
                    let prefix = if *ordered {
                        let prefix = format!("{}. ", *index);
                        *index += 1;
                        prefix
                    } else {
                        "- ".to_string()
                    };
                    spans.push(Span::styled(prefix, current_style(&style_stack)));
                }
            }
            MdEvent::End(MdTagEnd::Item) => {
                push_line(&mut lines, &mut spans);
            }
            MdEvent::Start(MdTag::CodeBlock(_)) => {
                push_line(&mut lines, &mut spans);
                in_code_block = true;
                let mut style = current_style(&style_stack);
                style = style.fg(Color::DarkGray);
                style_stack.push(style);
            }
            MdEvent::End(MdTagEnd::CodeBlock) => {
                in_code_block = false;
                push_line(&mut lines, &mut spans);
                lines.push(Line::from("".to_string()));
                style_stack.pop();
            }
            MdEvent::Start(MdTag::Emphasis) => {
                let mut style = current_style(&style_stack);
                style = style.add_modifier(Modifier::ITALIC);
                style_stack.push(style);
            }
            MdEvent::End(MdTagEnd::Emphasis) => {
                style_stack.pop();
            }
            MdEvent::Start(MdTag::Strong) => {
                let mut style = current_style(&style_stack);
                style = style.add_modifier(Modifier::BOLD);
                style_stack.push(style);
            }
            MdEvent::End(MdTagEnd::Strong) => {
                style_stack.pop();
            }
            MdEvent::Start(MdTag::BlockQuote) => {
                let mut style = current_style(&style_stack);
                style = style.add_modifier(Modifier::ITALIC);
                style_stack.push(style);
            }
            MdEvent::End(MdTagEnd::BlockQuote) => {
                style_stack.pop();
            }
            MdEvent::Text(text) => {
                let style = current_style(&style_stack);
                spans.push(Span::styled(text.to_string(), style));
            }
            MdEvent::Code(text) => {
                let mut style = current_style(&style_stack);
                style = style.add_modifier(Modifier::BOLD);
                spans.push(Span::styled(text.to_string(), style));
            }
            MdEvent::SoftBreak => {
                if in_code_block {
                    push_line(&mut lines, &mut spans);
                } else {
                    spans.push(Span::raw(" "));
                }
            }
            MdEvent::HardBreak => {
                push_line(&mut lines, &mut spans);
            }
            MdEvent::Rule => {
                push_line(&mut lines, &mut spans);
                lines.push(Line::from("----".to_string()));
                lines.push(Line::from("".to_string()));
            }
            _ => {}
        }
    }

    if !spans.is_empty() {
        lines.push(Line::from(spans));
    }

    Text::from(lines)
}

fn current_style(stack: &[Style]) -> Style {
    stack.last().cloned().unwrap_or_default()
}

fn text_has_content(text: &Text<'static>) -> bool {
    text.lines.iter().any(|line| !line_is_blank(line))
}

#[derive(Clone)]
struct EditorCommand {
    bin: String,
    args: Vec<String>,
    adapter: EditorAdapter,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditorAdapter {
    Generic,
    Vim,
    Helix,
}

impl EditorCommand {
    fn from_env() -> Result<Self> {
        let editor = std::env::var("EDITOR").context("$EDITOR is not set")?;
        let editor = editor.trim();
        if editor.is_empty() {
            return Err(anyhow!("$EDITOR is empty"));
        }
        let mut parts = editor.split_whitespace();
        let bin = parts.next().ok_or_else(|| anyhow!("$EDITOR is empty"))?;
        let args: Vec<String> = parts.map(|part| part.to_string()).collect();
        let adapter = detect_adapter(bin);
        Ok(Self {
            bin: bin.to_string(),
            args,
            adapter,
        })
    }
}

fn detect_adapter(bin: &str) -> EditorAdapter {
    let name = std::path::Path::new(bin)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(bin)
        .to_lowercase();

    if name == "nvim" || name == "vim" || name == "vi" {
        EditorAdapter::Vim
    } else if name == "hx" || name == "helix" {
        EditorAdapter::Helix
    } else {
        EditorAdapter::Generic
    }
}

struct EmbeddedEditor {
    master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    parser: VtParser,
    rx: mpsc::Receiver<Vec<u8>>,
    _reader: thread::JoinHandle<()>,
    child: Box<dyn portable_pty::Child + Send>,
}

impl EmbeddedEditor {
    fn new(cmd: &EditorCommand, path: &Path, rows: u16, cols: u16) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to open pty")?;

        let mut builder = CommandBuilder::new(&cmd.bin);
        builder.args(&cmd.args);
        builder.env("TERM", "xterm-256color");
        builder.env("COLORTERM", "truecolor");
        builder.arg(path);

        let child = pair
            .slave
            .spawn_command(builder)
            .context("failed to spawn editor")?;

        let mut reader = pair
            .master
            .try_clone_reader()
            .context("failed to clone pty reader")?;
        let writer = pair
            .master
            .take_writer()
            .context("failed to take pty writer")?;
        let (tx, rx) = mpsc::channel();
        let reader_thread = thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            master: pair.master,
            writer,
            parser: VtParser::new(rows, cols, 0),
            rx,
            _reader: reader_thread,
            child,
        })
    }

    fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to resize pty")?;
        self.parser.set_size(rows, cols);
        Ok(())
    }

    fn drain(&mut self) {
        while let Ok(chunk) = self.rx.try_recv() {
            self.parser.process(&chunk);
        }
    }

    fn text(&mut self) -> Text<'static> {
        self.drain();
        let screen = self.parser.screen();
        screen_to_text(screen)
    }

    fn open_file(&mut self, adapter: EditorAdapter, path: &Path) -> Result<()> {
        let command = match adapter {
            EditorAdapter::Vim => {
                let escaped = escape_vim_path(path);
                format!(":e {}\r\x1b", escaped)
            }
            EditorAdapter::Helix => {
                let escaped = escape_helix_path(path);
                format!(":open {}\r\x1b", escaped)
            }
            EditorAdapter::Generic => {
                return Ok(());
            }
        };

        self.send_bytes(b"\x1b")?;
        thread::sleep(Duration::from_millis(30));
        self.send_bytes(command.as_bytes())
    }

    fn send_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }
}

fn screen_to_text(screen: &vt100::Screen) -> Text<'static> {
    let (rows, cols) = screen.size();
    let mut lines: Vec<Line<'static>> = Vec::new();

    for row in 0..rows {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut buffer = String::new();
        let mut current_style: Option<Style> = None;

        for col in 0..cols {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }

            let mut text = cell.contents();
            if text.is_empty() {
                text = " ".to_string();
            }

            let style = style_from_cell(cell);
            if let Some(existing) = current_style {
                if existing == style {
                    buffer.push_str(&text);
                    continue;
                }
                if !buffer.is_empty() {
                    spans.push(Span::styled(buffer.clone(), existing));
                    buffer.clear();
                }
            }

            current_style = Some(style);
            buffer.push_str(&text);
        }

        if let Some(style) = current_style {
            if !buffer.is_empty() {
                spans.push(Span::styled(buffer, style));
            }
        }

        lines.push(Line::from(spans));
    }

    Text::from(lines)
}

fn style_from_cell(cell: &VtCell) -> Style {
    let mut style = Style::default();

    let fg = cell.fgcolor();
    if fg != VtColor::Default {
        style = style.fg(map_vt_color(fg));
    }
    let bg = cell.bgcolor();
    if bg != VtColor::Default {
        style = style.bg(map_vt_color(bg));
    }
    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.inverse() {
        style = style.add_modifier(Modifier::REVERSED);
    }

    style
}

fn map_vt_color(color: VtColor) -> Color {
    match color {
        VtColor::Default => Color::Reset,
        VtColor::Idx(idx) => Color::Indexed(idx),
        VtColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

fn escape_vim_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let mut escaped = String::new();
    for ch in raw.chars() {
        match ch {
            ' ' => escaped.push_str("\\ "),
            '\\' => escaped.push_str("\\\\"),
            '|' => escaped.push_str("\\|"),
            '"' => escaped.push_str("\\\""),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn escape_helix_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    if raw.contains(' ') || raw.contains('"') || raw.contains('\\') {
        let mut escaped = String::new();
        escaped.push('"');
        for ch in raw.chars() {
            match ch {
                '"' => escaped.push_str("\\\""),
                '\\' => escaped.push_str("\\\\"),
                _ => escaped.push(ch),
            }
        }
        escaped.push('"');
        escaped
    } else {
        raw.to_string()
    }
}

impl Drop for EmbeddedEditor {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn key_to_bytes(key: KeyEvent) -> Option<Vec<u8>> {
    match key.code {
        KeyCode::Char(ch) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                let byte = (ch as u8) & 0x1f;
                Some(vec![byte])
            } else if key.modifiers.contains(KeyModifiers::ALT) {
                Some(vec![0x1b, ch as u8])
            } else {
                Some(ch.to_string().into_bytes())
            }
        }
        KeyCode::Enter => Some(vec![b'\r']),
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Tab => Some(vec![b'\t']),
        KeyCode::Esc => Some(vec![0x1b]),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        KeyCode::PageUp => Some(b"\x1b[5~".to_vec()),
        KeyCode::PageDown => Some(b"\x1b[6~".to_vec()),
        KeyCode::Home => Some(b"\x1b[H".to_vec()),
        KeyCode::End => Some(b"\x1b[F".to_vec()),
        KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
        KeyCode::Insert => Some(b"\x1b[2~".to_vec()),
        _ => None,
    }
}
