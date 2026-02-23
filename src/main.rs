use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs;
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseButton,
    MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use serde_json::Value;
use uuid::Uuid;

fn main() -> Result<()> {
    let mut app = App::load()?;
    let mut tui = Tui::new()?;

    let run_result = run_app(&mut tui, &mut app);
    let restore_result = tui.restore();

    run_result?;
    restore_result?;
    Ok(())
}

fn run_app(tui: &mut Tui, app: &mut App) -> Result<()> {
    loop {
        // Debounce expensive search filtering: apply only when event queue is idle.
        if app.search_dirty && !event::poll(Duration::from_millis(0))? {
            app.apply_search_filter();
            app.search_dirty = false;
        }

        tui.draw(app)?;

        if !event::poll(Duration::from_millis(150))? {
            continue;
        }

        match event::read()? {
            Event::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match app.mode {
                    Mode::Normal => {
                        if handle_normal_mode(key.code, app)? {
                            return Ok(());
                        }
                    }
                    Mode::Input => handle_input_mode(key.code, app)?,
                }
            }
            Event::Mouse(mouse) => handle_mouse_event(mouse, app),
            _ => {}
        }
    }
}

fn handle_mouse_event(mouse: MouseEvent, app: &mut App) {
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if is_on_splitter(
                mouse.column,
                mouse.row,
                app.panes.projects,
                app.panes.sessions,
            ) {
                app.drag_target = Some(DragTarget::LeftSplitter);
                return;
            }
            if is_on_splitter(
                mouse.column,
                mouse.row,
                app.panes.sessions,
                app.panes.preview,
            ) {
                app.drag_target = Some(DragTarget::RightSplitter);
                return;
            }

            if point_in_rect(mouse.column, mouse.row, app.panes.search) {
                app.search_focused = true;
                app.input_focused = false;
            } else if point_in_rect(mouse.column, mouse.row, app.panes.projects) {
                app.search_focused = false;
                if app.mode == Mode::Input {
                    app.input_focused = false;
                }
                app.focus = Focus::Projects;
                let idx = app.project_scroll + mouse_row_to_index(mouse.row, app.panes.projects);
                if idx < app.projects.len() {
                    app.project_idx = idx;
                    app.clamp_session_idx();
                    app.preview_scroll = 0;
                    app.ensure_selection_visible();
                }
            } else if point_in_rect(mouse.column, mouse.row, app.panes.sessions) {
                app.search_focused = false;
                if app.mode == Mode::Input {
                    app.input_focused = false;
                }
                app.focus = Focus::Sessions;
                let len = app.current_project().map(|p| p.sessions.len()).unwrap_or(0);
                let idx = app.session_scroll + mouse_row_to_index(mouse.row, app.panes.sessions);
                if idx < len {
                    app.session_idx = idx;
                    app.preview_scroll = 0;
                    app.ensure_selection_visible();
                }
            } else if point_in_rect(mouse.column, mouse.row, app.panes.preview) {
                app.search_focused = false;
                if app.mode == Mode::Input {
                    app.input_focused = false;
                }
                app.focus = Focus::Preview;
            } else if point_in_rect(mouse.column, mouse.row, app.panes.status) {
                app.search_focused = false;
                handle_status_click(mouse.column, mouse.row, app);
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(target) = app.drag_target {
                app.resize_from_mouse(target, mouse.column);
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            app.drag_target = None;
        }
        MouseEventKind::ScrollUp => {
            if point_in_rect(mouse.column, mouse.row, app.panes.projects) {
                app.focus = Focus::Projects;
                app.move_up();
            } else if point_in_rect(mouse.column, mouse.row, app.panes.sessions) {
                app.focus = Focus::Sessions;
                app.move_up();
            } else if point_in_rect(mouse.column, mouse.row, app.panes.preview) {
                app.focus = Focus::Preview;
                app.move_up();
            }
        }
        MouseEventKind::ScrollDown => {
            if point_in_rect(mouse.column, mouse.row, app.panes.projects) {
                app.focus = Focus::Projects;
                app.move_down();
            } else if point_in_rect(mouse.column, mouse.row, app.panes.sessions) {
                app.focus = Focus::Sessions;
                app.move_down();
            } else if point_in_rect(mouse.column, mouse.row, app.panes.preview) {
                app.focus = Focus::Preview;
                app.move_down();
            }
        }
        _ => {}
    }
}

fn is_on_splitter(
    x: u16,
    y: u16,
    left: ratatui::layout::Rect,
    right: ratatui::layout::Rect,
) -> bool {
    let splitter_x = right.x;
    let y0 = left.y;
    let y1 = left.y.saturating_add(left.height);
    y >= y0 && y < y1 && (x == splitter_x || x.saturating_add(1) == splitter_x)
}

fn handle_status_click(x: u16, y: u16, app: &mut App) {
    let content_y = app.panes.status.y.saturating_add(1);
    let controls_y = content_y.saturating_add(2);
    if y == controls_y {
        // Second status content row: pseudo-buttons.
        if app.mode == Mode::Input {
            // [Apply] [Cancel]
            let rel_x = x.saturating_sub(app.panes.status.x.saturating_add(1));
            if rel_x <= 6 {
                let _ = app.submit_input();
            } else if (8..=15).contains(&rel_x) {
                app.cancel_input();
            }
        } else {
            // [Move] [Copy] [Fork] [Refresh] [Quit]
            let rel_x = x.saturating_sub(app.panes.status.x.saturating_add(1));
            if rel_x <= 5 {
                app.start_action(Action::Move);
            } else if (7..=12).contains(&rel_x) {
                app.start_action(Action::Copy);
            } else if (14..=19).contains(&rel_x) {
                app.start_action(Action::Fork);
            } else if (21..=29).contains(&rel_x) {
                let _ = app.reload();
            } else if (31..=36).contains(&rel_x) {
                app.status = String::from("Use q to quit");
            }
        }
    }

    if app.mode == Mode::Input && y == controls_y.saturating_add(1) {
        app.input_focused = true;
    }
}

fn point_in_rect(x: u16, y: u16, rect: ratatui::layout::Rect) -> bool {
    x >= rect.x
        && x < rect.x.saturating_add(rect.width)
        && y >= rect.y
        && y < rect.y.saturating_add(rect.height)
}

fn mouse_row_to_index(y: u16, pane: ratatui::layout::Rect) -> usize {
    // Exclude the top border/title row.
    y.saturating_sub(pane.y.saturating_add(1)) as usize
}

fn handle_normal_mode(code: KeyCode, app: &mut App) -> Result<bool> {
    if app.search_focused {
        match code {
            KeyCode::Esc => {
                app.search_focused = false;
            }
            KeyCode::Enter => {
                app.search_focused = false;
            }
            KeyCode::Backspace => {
                app.search_query.pop();
                app.search_dirty = true;
            }
            KeyCode::Char(ch) => {
                app.search_query.push(ch);
                app.search_dirty = true;
            }
            _ => {}
        }
        return Ok(false);
    }

    match code {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Char('/') => {
            app.search_focused = true;
        }
        KeyCode::Tab => app.next_focus(),
        KeyCode::BackTab => app.prev_focus(),
        KeyCode::Up | KeyCode::Char('k') => app.move_up(),
        KeyCode::Down | KeyCode::Char('j') => app.move_down(),
        KeyCode::Char('g') => app.reload()?,
        KeyCode::Char('m') => app.start_action(Action::Move),
        KeyCode::Char('c') => app.start_action(Action::Copy),
        KeyCode::Char('f') => app.start_action(Action::Fork),
        KeyCode::Char('v') => app.toggle_preview_mode(),
        KeyCode::Char('H') | KeyCode::Char('h') => app.resize_focused_pane(-2),
        KeyCode::Char('L') | KeyCode::Char('l') => app.resize_focused_pane(2),
        _ => {}
    }

    Ok(false)
}

fn handle_input_mode(code: KeyCode, app: &mut App) -> Result<()> {
    match code {
        KeyCode::Esc => app.cancel_input(),
        KeyCode::Enter => app.submit_input()?,
        KeyCode::Backspace => {
            if app.input_focused {
                app.input.pop();
            }
        }
        KeyCode::Char(ch) => {
            if app.input_focused {
                app.input.push(ch);
            }
        }
        _ => {}
    }

    Ok(())
}

struct Tui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl Tui {
    fn new() -> Result<Self> {
        enable_raw_mode().context("failed to enable raw mode")?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
            .context("failed to enter alternate screen")?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).context("failed to create terminal")?;
        Ok(Self { terminal })
    }

    fn draw(&mut self, app: &mut App) -> Result<()> {
        self.terminal.draw(|frame| {
            let search_height = if app.search_visible() { 3 } else { 0 };
            let root = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(search_height),
                    Constraint::Min(10),
                    Constraint::Length(7),
                ])
                .split(frame.area());

            let panes = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(app.project_width_pct),
                    Constraint::Percentage(app.session_width_pct),
                    Constraint::Percentage(app.preview_width_pct()),
                ])
                .split(root[1]);

            app.panes = PaneLayout {
                search: root[0],
                projects: panes[0],
                sessions: panes[1],
                preview: panes[2],
                status: root[2],
            };
            app.ensure_selection_visible();
            if app.search_visible() {
                render_search(frame, root[0], app);
            }
            render_projects(frame, app.panes.projects, app);
            render_sessions(frame, app.panes.sessions, app);
            render_preview(frame, app.panes.preview, app);
            render_status(frame, root[2], app);
        })?;

        Ok(())
    }

    fn restore(&mut self) -> Result<()> {
        disable_raw_mode().context("failed to disable raw mode")?;
        execute!(
            self.terminal.backend_mut(),
            DisableMouseCapture,
            LeaveAlternateScreen
        )
        .context("failed to leave alternate screen")?;
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Projects,
    Sessions,
    Preview,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Move,
    Copy,
    Fork,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    Input,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PreviewMode {
    Chat,
    Events,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DragTarget {
    LeftSplitter,
    RightSplitter,
}

#[derive(Clone)]
struct SessionSummary {
    path: PathBuf,
    file_name: String,
    id: String,
    cwd: String,
    started_at: String,
    event_count: usize,
    search_blob: String,
}

#[derive(Clone)]
struct ProjectBucket {
    cwd: String,
    sessions: Vec<SessionSummary>,
}

#[derive(Clone, Copy, Default)]
struct PaneLayout {
    search: ratatui::layout::Rect,
    projects: ratatui::layout::Rect,
    sessions: ratatui::layout::Rect,
    preview: ratatui::layout::Rect,
    status: ratatui::layout::Rect,
}

struct App {
    sessions_root: PathBuf,
    all_projects: Vec<ProjectBucket>,
    projects: Vec<ProjectBucket>,
    project_idx: usize,
    session_idx: usize,
    focus: Focus,
    mode: Mode,
    pending_action: Option<Action>,
    input: String,
    input_focused: bool,
    search_query: String,
    search_focused: bool,
    search_dirty: bool,
    preview_mode: PreviewMode,
    drag_target: Option<DragTarget>,
    status: String,
    panes: PaneLayout,
    project_width_pct: u16,
    session_width_pct: u16,
    project_scroll: usize,
    session_scroll: usize,
    preview_scroll: usize,
}

impl App {
    fn load() -> Result<Self> {
        let codex_home = resolve_codex_home()?;
        let sessions_root = codex_home.join("sessions");

        let mut app = Self {
            sessions_root,
            all_projects: Vec::new(),
            projects: Vec::new(),
            project_idx: 0,
            session_idx: 0,
            focus: Focus::Projects,
            mode: Mode::Normal,
            pending_action: None,
            input: String::new(),
            input_focused: false,
            search_query: String::new(),
            search_focused: false,
            search_dirty: false,
            preview_mode: PreviewMode::Chat,
            drag_target: None,
            status: String::from("Press q to quit, g to refresh"),
            panes: PaneLayout::default(),
            project_width_pct: 28,
            session_width_pct: 38,
            project_scroll: 0,
            session_scroll: 0,
            preview_scroll: 0,
        };

        app.reload()?;
        Ok(app)
    }

    fn reload(&mut self) -> Result<()> {
        self.all_projects = scan_sessions(&self.sessions_root)?;
        self.apply_search_filter();

        if self.projects.is_empty() {
            self.project_idx = 0;
            self.session_idx = 0;
            self.status = format!("No sessions found under {}", self.sessions_root.display());
            return Ok(());
        }

        self.project_idx = self.project_idx.min(self.projects.len().saturating_sub(1));
        let sessions_len = self
            .current_project()
            .map(|p| p.sessions.len())
            .unwrap_or(0);
        if sessions_len > 0 {
            self.session_idx = self.session_idx.min(sessions_len.saturating_sub(1));
        } else {
            self.session_idx = 0;
        }

        self.status = format!("Loaded {} projects", self.projects.len());
        self.ensure_selection_visible();
        Ok(())
    }

    fn next_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Projects => Focus::Sessions,
            Focus::Sessions => Focus::Preview,
            Focus::Preview => Focus::Projects,
        };
    }

    fn prev_focus(&mut self) {
        self.next_focus();
    }

    fn move_up(&mut self) {
        if self.projects.is_empty() {
            return;
        }

        match self.focus {
            Focus::Projects => {
                if self.project_idx > 0 {
                    self.project_idx -= 1;
                }
                self.clamp_session_idx();
                self.preview_scroll = 0;
                self.ensure_selection_visible();
            }
            Focus::Sessions => {
                if self.session_idx > 0 {
                    self.session_idx -= 1;
                }
                self.preview_scroll = 0;
                self.ensure_selection_visible();
            }
            Focus::Preview => {
                self.preview_scroll = self.preview_scroll.saturating_sub(1);
            }
        }
    }

    fn move_down(&mut self) {
        if self.projects.is_empty() {
            return;
        }

        match self.focus {
            Focus::Projects => {
                if self.project_idx + 1 < self.projects.len() {
                    self.project_idx += 1;
                }
                self.clamp_session_idx();
                self.preview_scroll = 0;
                self.ensure_selection_visible();
            }
            Focus::Sessions => {
                if let Some(project) = self.current_project() {
                    if self.session_idx + 1 < project.sessions.len() {
                        self.session_idx += 1;
                    }
                }
                self.preview_scroll = 0;
                self.ensure_selection_visible();
            }
            Focus::Preview => {
                self.preview_scroll = self.preview_scroll.saturating_add(1);
            }
        }
    }

    fn clamp_session_idx(&mut self) {
        let len = self
            .current_project()
            .map(|p| p.sessions.len())
            .unwrap_or(0);
        if len == 0 {
            self.session_idx = 0;
            return;
        }

        self.session_idx = self.session_idx.min(len.saturating_sub(1));
    }

    fn visible_rows(pane_height: u16) -> usize {
        pane_height.saturating_sub(2) as usize
    }

    fn ensure_selection_visible(&mut self) {
        let project_visible = Self::visible_rows(self.panes.projects.height).max(1);
        if self.project_idx < self.project_scroll {
            self.project_scroll = self.project_idx;
        } else if self.project_idx >= self.project_scroll + project_visible {
            self.project_scroll = self.project_idx + 1 - project_visible;
        }

        let session_visible = Self::visible_rows(self.panes.sessions.height).max(1);
        if self.session_idx < self.session_scroll {
            self.session_scroll = self.session_idx;
        } else if self.session_idx >= self.session_scroll + session_visible {
            self.session_scroll = self.session_idx + 1 - session_visible;
        }
    }

    fn apply_search_filter(&mut self) {
        if self.search_query.trim().is_empty() {
            self.projects = self.all_projects.clone();
            self.project_idx = self.project_idx.min(self.projects.len().saturating_sub(1));
            self.clamp_session_idx();
            self.project_scroll = 0;
            self.session_scroll = 0;
            self.preview_scroll = 0;
            self.search_dirty = false;
            return;
        }

        let query = self.search_query.to_lowercase();
        let mut filtered = Vec::new();

        for project in &self.all_projects {
            let mut scored: Vec<(i64, SessionSummary)> = Vec::new();
            for session in &project.sessions {
                let session_text = format!(
                    "{}\n{}\n{}\n{}",
                    session.search_blob, session.file_name, session.id, project.cwd
                );
                let mut best = fuzzy_score(&query, &session_text).unwrap_or(i64::MIN);
                if let Some(path_score) = fuzzy_score(&query, &project.cwd.to_lowercase()) {
                    best = best.max(path_score / 2);
                }
                if best > i64::MIN {
                    scored.push((best, session.clone()));
                }
            }

            if !scored.is_empty() {
                scored.sort_by(|a, b| {
                    b.0.cmp(&a.0)
                        .then_with(|| b.1.started_at.cmp(&a.1.started_at))
                });
                filtered.push(ProjectBucket {
                    cwd: project.cwd.clone(),
                    sessions: scored.into_iter().map(|(_, s)| s).collect(),
                });
            }
        }

        filtered.sort_by(|a, b| {
            b.sessions
                .len()
                .cmp(&a.sessions.len())
                .then_with(|| a.cwd.cmp(&b.cwd))
        });
        self.projects = filtered;
        self.project_idx = 0;
        self.session_idx = 0;
        self.project_scroll = 0;
        self.session_scroll = 0;
        self.preview_scroll = 0;
        self.status = format!(
            "Search '{}' matched {} projects",
            self.search_query,
            self.projects.len()
        );
        self.search_dirty = false;
    }

    fn toggle_preview_mode(&mut self) {
        self.preview_mode = match self.preview_mode {
            PreviewMode::Chat => PreviewMode::Events,
            PreviewMode::Events => PreviewMode::Chat,
        };
        self.preview_scroll = 0;
    }

    fn preview_width_pct(&self) -> u16 {
        100u16.saturating_sub(self.project_width_pct + self.session_width_pct)
    }

    fn search_visible(&self) -> bool {
        self.search_focused || !self.search_query.trim().is_empty()
    }

    fn resize_focused_pane(&mut self, delta: i16) {
        let min = 15i16;
        let mut p = self.project_width_pct as i16;
        let mut s = self.session_width_pct as i16;
        let mut r = 100i16 - p - s;

        match self.focus {
            Focus::Projects => {
                p += delta;
                r -= delta;
            }
            Focus::Sessions => {
                s += delta;
                r -= delta;
            }
            Focus::Preview => {
                r += delta;
                s -= delta;
            }
        }

        if p < min || s < min || r < min {
            return;
        }

        self.project_width_pct = p as u16;
        self.session_width_pct = s as u16;
    }

    fn resize_from_mouse(&mut self, target: DragTarget, mouse_x: u16) {
        let total_width = self
            .panes
            .projects
            .width
            .saturating_add(self.panes.sessions.width)
            .saturating_add(self.panes.preview.width);
        if total_width == 0 {
            return;
        }

        let x0 = self.panes.projects.x;
        let x1 = self.panes.sessions.x;
        let x2 = self.panes.preview.x;
        let right = x0.saturating_add(total_width);

        let mut split1 = x1;
        let mut split2 = x2;

        match target {
            DragTarget::LeftSplitter => {
                split1 = mouse_x.clamp(x0.saturating_add(8), split2.saturating_sub(8));
            }
            DragTarget::RightSplitter => {
                split2 = mouse_x.clamp(split1.saturating_add(8), right.saturating_sub(8));
            }
        }

        let p = split1.saturating_sub(x0) as f32 / total_width as f32 * 100.0;
        let s = split2.saturating_sub(split1) as f32 / total_width as f32 * 100.0;
        let mut p_pct = p.round() as i16;
        let mut s_pct = s.round() as i16;
        let min = 15i16;
        let mut r_pct = 100 - p_pct - s_pct;

        if p_pct < min {
            let d = min - p_pct;
            p_pct += d;
            r_pct -= d;
        }
        if s_pct < min {
            let d = min - s_pct;
            s_pct += d;
            r_pct -= d;
        }
        if r_pct < min {
            let d = min - r_pct;
            if target == DragTarget::LeftSplitter {
                p_pct -= d;
            } else {
                s_pct -= d;
            }
        }

        if p_pct >= min && s_pct >= min && (100 - p_pct - s_pct) >= min {
            self.project_width_pct = p_pct as u16;
            self.session_width_pct = s_pct as u16;
        }
    }

    fn current_project(&self) -> Option<&ProjectBucket> {
        self.projects.get(self.project_idx)
    }

    fn current_session(&self) -> Option<&SessionSummary> {
        self.current_project()
            .and_then(|project| project.sessions.get(self.session_idx))
    }

    fn start_action(&mut self, action: Action) {
        if self.current_session().is_none() {
            self.status = String::from("No session selected");
            return;
        }

        self.mode = Mode::Input;
        self.pending_action = Some(action);
        self.input.clear();
        self.input_focused = true;
        self.search_focused = false;
        self.status = match action {
            Action::Move => String::from("Move: enter target project path and press Enter"),
            Action::Copy => String::from("Copy: enter target project path and press Enter"),
            Action::Fork => String::from("Fork: enter target project path and press Enter"),
        };
    }

    fn cancel_input(&mut self) {
        self.mode = Mode::Normal;
        self.pending_action = None;
        self.input.clear();
        self.input_focused = false;
        self.status = String::from("Action cancelled");
    }

    fn submit_input(&mut self) -> Result<()> {
        let Some(action) = self.pending_action else {
            self.cancel_input();
            return Ok(());
        };

        let target = expand_tilde(self.input.trim());
        if target.as_os_str().is_empty() {
            self.status = String::from("Target path is empty");
            return Ok(());
        }

        let session = self
            .current_session()
            .cloned()
            .ok_or_else(|| anyhow!("no selected session"))?;

        let target_str = target.to_string_lossy().to_string();

        match action {
            Action::Move => {
                rewrite_session_file(&session.path, &target_str, false)?;
                self.status = format!("Moved {} -> {}", session.file_name, target_str);
            }
            Action::Copy => {
                let new_path =
                    duplicate_session_file(&self.sessions_root, &session, &target_str, false)?;
                self.status = format!("Copied to {}", new_path.display());
            }
            Action::Fork => {
                let new_path =
                    duplicate_session_file(&self.sessions_root, &session, &target_str, true)?;
                self.status = format!("Forked to {}", new_path.display());
            }
        }

        self.mode = Mode::Normal;
        self.pending_action = None;
        self.input.clear();
        self.input_focused = false;
        self.reload()?;
        Ok(())
    }
}

fn render_projects(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &App) {
    let items: Vec<ListItem> = app
        .projects
        .iter()
        .map(|project| {
            let label = format!("{} ({})", project.cwd, project.sessions.len());
            ListItem::new(label)
        })
        .collect();

    let mut state = ListState::default();
    if !app.projects.is_empty() {
        state.select(Some(app.project_idx));
        state = state.with_offset(app.project_scroll);
    }

    let focus_style = if app.focus == Focus::Projects && app.mode == Mode::Normal {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };

    let list = List::new(items)
        .block(
            Block::default()
                .title("Projects (cwd)")
                .borders(Borders::ALL)
                .border_style(focus_style),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Rgb(44, 54, 84))
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(" > ");

    frame.render_stateful_widget(list, area, &mut state);
}

fn render_search(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &App) {
    let focus_style = if app.search_focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };

    let query_prefix = if app.search_focused { ">" } else { " " };
    let content = format!("{query_prefix} {}", app.search_query);

    let para = Paragraph::new(Line::from(vec![
        Span::styled("Search ", Style::default().fg(Color::Cyan)),
        Span::raw(content),
    ]))
    .block(
        Block::default()
            .title("Search")
            .borders(Borders::ALL)
            .border_style(focus_style),
    )
    .wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

fn render_sessions(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &App) {
    let sessions = app
        .current_project()
        .map(|project| project.sessions.clone())
        .unwrap_or_default();

    let items: Vec<ListItem> = sessions
        .iter()
        .map(|session| {
            let label = format!(
                "{} | {} events | {}",
                session.started_at, session.event_count, session.file_name
            );
            ListItem::new(label)
        })
        .collect();

    let mut state = ListState::default();
    if !sessions.is_empty() {
        state.select(Some(app.session_idx));
        state = state.with_offset(app.session_scroll);
    }

    let focus_style = if app.focus == Focus::Sessions && app.mode == Mode::Normal {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };

    let list = List::new(items)
        .block(
            Block::default()
                .title("Sessions")
                .borders(Borders::ALL)
                .border_style(focus_style),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Rgb(39, 62, 84))
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(" > ");

    frame.render_stateful_widget(list, area, &mut state);
}

fn render_preview(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &App) {
    let preview = if let Some(session) = app.current_session() {
        let inner_width = area.width.saturating_sub(2) as usize;
        match build_preview(session, app.preview_mode, inner_width) {
            Ok(preview) => preview,
            Err(err) => PreviewData {
                lines: vec![Line::from(format!("Preview error: {err:#}"))],
                user_rows: Vec::new(),
            },
        }
    } else {
        PreviewData {
            lines: vec![Line::from("No session selected")],
            user_rows: Vec::new(),
        }
    };

    let focus_style = if app.focus == Focus::Preview && app.mode == Mode::Normal {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let mode_name = match app.preview_mode {
        PreviewMode::Chat => "Chat",
        PreviewMode::Events => "Events",
    };
    let block = Block::default()
        .title(format!("Preview ({mode_name})"))
        .borders(Borders::ALL)
        .border_style(focus_style);
    let para = Paragraph::new(preview.lines.clone())
        .block(block)
        .scroll((app.preview_scroll as u16, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(para, area);

    let inner_x = area.x.saturating_add(1);
    let inner_y = area.y.saturating_add(1);
    let inner_w = area.width.saturating_sub(2);
    let inner_h = area.height.saturating_sub(2) as usize;
    let scroll = app.preview_scroll;

    for row in preview.user_rows {
        if row < scroll || row >= scroll + inner_h {
            continue;
        }
        let screen_y = inner_y + (row - scroll) as u16;
        frame.buffer_mut().set_style(
            ratatui::layout::Rect {
                x: inner_x,
                y: screen_y,
                width: inner_w,
                height: 1,
            },
            Style::default().add_modifier(Modifier::DIM | Modifier::REVERSED),
        );
    }
}

fn render_status(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &App) {
    let key_line = Line::from(vec![
        Span::styled("tab", Style::default().fg(Color::Cyan)),
        Span::raw(" focus  "),
        Span::styled("j/k", Style::default().fg(Color::Cyan)),
        Span::raw(" nav  "),
        Span::styled("/", Style::default().fg(Color::Cyan)),
        Span::raw(" search  "),
        Span::styled("v", Style::default().fg(Color::Cyan)),
        Span::raw(" preview-mode  "),
        Span::styled("h/l", Style::default().fg(Color::Cyan)),
        Span::raw(" resize-pane  "),
        Span::styled("drag", Style::default().fg(Color::Cyan)),
        Span::raw(" splitter  "),
        Span::styled("m/c/f", Style::default().fg(Color::Green)),
        Span::raw(" move/copy/fork  "),
        Span::styled("g", Style::default().fg(Color::Yellow)),
        Span::raw(" refresh  "),
        Span::styled("q", Style::default().fg(Color::Red)),
        Span::raw(" quit"),
    ]);
    let search_meta = if app.search_query.trim().is_empty() {
        String::from("search: <none>")
    } else {
        format!(
            "search: '{}' ({} projects)",
            app.search_query,
            app.projects.len()
        )
    };
    let preview_mode = match app.preview_mode {
        PreviewMode::Chat => "chat",
        PreviewMode::Events => "events",
    };
    let pane_meta = format!(
        "pane widths p/s/r: {}/{}/{}  preview: {}",
        app.project_width_pct,
        app.session_width_pct,
        app.preview_width_pct(),
        preview_mode
    );
    let meta_line = Line::from(vec![
        Span::styled(search_meta, Style::default().fg(Color::DarkGray)),
        Span::raw("  "),
        Span::styled(pane_meta, Style::default().fg(Color::DarkGray)),
    ]);

    let mut lines = if app.mode == Mode::Input {
        vec![Line::from(vec![
            Span::styled("[Apply]", Style::default().fg(Color::Green)),
            Span::raw(" "),
            Span::styled("[Cancel]", Style::default().fg(Color::Red)),
            Span::raw("  (click buttons or press Enter/Esc)"),
        ])]
    } else {
        vec![Line::from(vec![
            Span::styled("[Move]", Style::default().fg(Color::Green)),
            Span::raw(" "),
            Span::styled("[Copy]", Style::default().fg(Color::Green)),
            Span::raw(" "),
            Span::styled("[Fork]", Style::default().fg(Color::Green)),
            Span::raw(" "),
            Span::styled("[Refresh]", Style::default().fg(Color::Yellow)),
            Span::raw(" "),
            Span::styled("[Quit]", Style::default().fg(Color::Red)),
            Span::raw("  wheel scrolls panes"),
        ])]
    };

    lines.insert(0, meta_line);
    lines.insert(0, key_line);

    if app.mode == Mode::Input {
        let action = match app.pending_action {
            Some(Action::Move) => "MOVE",
            Some(Action::Copy) => "COPY",
            Some(Action::Fork) => "FORK",
            None => "ACTION",
        };

        let focus_mark = if app.input_focused { "*" } else { " " };
        lines.push(Line::from(format!(
            "{focus_mark} {action} target> {}",
            app.input
        )));
    } else {
        lines.push(Line::from(app.status.clone()));
    }

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Status"))
        .wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

fn build_preview(
    session: &SessionSummary,
    mode: PreviewMode,
    inner_width: usize,
) -> Result<PreviewData> {
    let content = fs::read_to_string(&session.path)
        .with_context(|| format!("failed to read {}", session.path.display()))?;

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Session ", Style::default().fg(Color::Cyan)),
            Span::raw(session.id.clone()),
        ]),
        Line::from(vec![
            Span::styled("Path    ", Style::default().fg(Color::DarkGray)),
            Span::raw(session.path.display().to_string()),
        ]),
        Line::from(vec![
            Span::styled("Cwd     ", Style::default().fg(Color::DarkGray)),
            Span::raw(session.cwd.clone()),
        ]),
        Line::from(vec![
            Span::styled("Started ", Style::default().fg(Color::DarkGray)),
            Span::raw(session.started_at.clone()),
        ]),
        Line::from(String::new()),
    ];
    let mut user_rows = Vec::new();

    if mode == PreviewMode::Events {
        lines.push(Line::from(Span::styled(
            "Event Stream",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
        append_event_preview(&mut lines, &content);
        return Ok(PreviewData { lines, user_rows });
    }

    lines.push(Line::from(Span::styled(
        "Conversation",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )));

    let turns = extract_chat_turns(&content);
    if turns.is_empty() {
        lines.push(Line::from(
            "No user/assistant chat messages found in this session.",
        ));
        return Ok(PreviewData { lines, user_rows });
    }

    const MAX_TURNS: usize = 120;
    let start = turns.len().saturating_sub(MAX_TURNS);
    if start > 0 {
        lines.push(Line::from(format!(
            "... showing last {} of {} turns ...",
            MAX_TURNS,
            turns.len()
        )));
        lines.push(Line::from(String::new()));
    }

    for turn in turns.into_iter().skip(start) {
        let role_style = match turn.role.as_str() {
            "user" => Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
            "assistant" => Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
            "developer" => Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
            _ => Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        };
        let display_role = if turn.role == "developer" {
            String::from("USER")
        } else {
            turn.role.to_uppercase()
        };
        lines.push(Line::from(vec![
            Span::styled(format!(" {} ", display_role), role_style),
            Span::raw(" "),
            Span::styled(turn.timestamp, Style::default().fg(Color::DarkGray)),
        ]));
        if turn.role == "user" || turn.role == "developer" {
            user_rows.push(lines.len().saturating_sub(1));
        }

        for body_line in turn.text.lines() {
            lines.push(Line::from(format!("  {body_line}")));
            if turn.role == "user" || turn.role == "developer" {
                user_rows.push(lines.len().saturating_sub(1));
            }
        }
        lines.push(Line::from(String::new()));
        if turn.role == "user" || turn.role == "developer" {
            user_rows.push(lines.len().saturating_sub(1));
        }
    }

    let _ = inner_width;
    Ok(PreviewData { lines, user_rows })
}

fn append_event_preview(lines: &mut Vec<Line<'static>>, content: &str) {
    let all: Vec<&str> = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let start = all.len().saturating_sub(220);
    if start > 0 {
        lines.push(Line::from(format!(
            "... showing last {} of {} events ...",
            all.len() - start,
            all.len()
        )));
        lines.push(Line::from(String::new()));
    }

    for raw in all.into_iter().skip(start) {
        let Ok(v) = serde_json::from_str::<Value>(raw) else {
            continue;
        };
        lines.push(Line::from(summarize_event_line(&v)));
    }
}

fn summarize_event_line(v: &Value) -> String {
    let ts = v.get("timestamp").and_then(Value::as_str).unwrap_or("-");
    let ty = v.get("type").and_then(Value::as_str).unwrap_or("unknown");
    if ty == "response_item" {
        let payload = v.get("payload").unwrap_or(&Value::Null);
        let pty = payload.get("type").and_then(Value::as_str).unwrap_or("?");
        if pty == "message" {
            let role = payload.get("role").and_then(Value::as_str).unwrap_or("?");
            return format!("[{ts}] response_item/message role={role}");
        }
        return format!("[{ts}] response_item/{pty}");
    }
    if ty == "event_msg" {
        let payload = v.get("payload").unwrap_or(&Value::Null);
        let pty = payload.get("type").and_then(Value::as_str).unwrap_or("?");
        return format!("[{ts}] event_msg/{pty}");
    }
    format!("[{ts}] {ty}")
}

#[derive(Clone)]
struct ChatTurn {
    role: String,
    timestamp: String,
    text: String,
}

#[derive(Clone)]
struct PreviewData {
    lines: Vec<Line<'static>>,
    user_rows: Vec<usize>,
}

fn extract_chat_turns(content: &str) -> Vec<ChatTurn> {
    let mut turns = Vec::new();
    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };

        if value.get("type").and_then(Value::as_str) != Some("response_item") {
            continue;
        }

        let payload = value.get("payload").unwrap_or(&Value::Null);
        if payload.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }

        let mut role = payload
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        if role == "developer" {
            role = String::from("user");
        }

        let timestamp = value
            .get("timestamp")
            .and_then(Value::as_str)
            .unwrap_or("-")
            .to_string();

        let mut text_parts = Vec::new();
        if let Some(items) = payload.get("content").and_then(Value::as_array) {
            for item in items {
                if let Some(text) = item
                    .get("text")
                    .or_else(|| item.get("input_text"))
                    .or_else(|| item.get("output_text"))
                    .and_then(Value::as_str)
                {
                    if !text.trim().is_empty() {
                        text_parts.push(text.to_string());
                    }
                }
            }
        }

        if text_parts.is_empty() {
            continue;
        }

        turns.push(ChatTurn {
            role,
            timestamp,
            text: text_parts.join("\n"),
        });
    }

    if turns.is_empty() {
        for line in content.lines().filter(|line| !line.trim().is_empty()) {
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if value.get("type").and_then(Value::as_str) != Some("event_msg") {
                continue;
            }
            let payload = value.get("payload").unwrap_or(&Value::Null);
            if payload.get("type").and_then(Value::as_str) != Some("user_message") {
                continue;
            }
            let Some(text) = payload.get("message").and_then(Value::as_str) else {
                continue;
            };
            let timestamp = value
                .get("timestamp")
                .and_then(Value::as_str)
                .unwrap_or("-")
                .to_string();
            turns.push(ChatTurn {
                role: String::from("user"),
                timestamp,
                text: text.to_string(),
            });
        }
    }

    turns
}

fn fuzzy_score(query: &str, haystack: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }

    let mut score = 0i64;
    let mut qi = 0usize;
    let qchars: Vec<char> = query.chars().collect();
    let hchars: Vec<char> = haystack.chars().collect();
    let mut prev_match: Option<usize> = None;

    for (i, hc) in hchars.iter().enumerate() {
        if qi >= qchars.len() {
            break;
        }
        if hc.eq_ignore_ascii_case(&qchars[qi]) {
            score += 10;
            if let Some(prev) = prev_match {
                if i == prev + 1 {
                    score += 8;
                }
            }
            if i == 0
                || hchars
                    .get(i.saturating_sub(1))
                    .is_some_and(|c| matches!(c, ' ' | '/' | '_' | '-' | '.'))
            {
                score += 6;
            }
            prev_match = Some(i);
            qi += 1;
        }
    }

    if qi == qchars.len() {
        Some(score - (hchars.len() as i64 / 8))
    } else {
        None
    }
}

fn scan_sessions(root: &Path) -> Result<Vec<ProjectBucket>> {
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    collect_jsonl_files(root, &mut files)?;

    let mut projects: HashMap<String, Vec<SessionSummary>> = HashMap::new();
    for path in files {
        if let Ok(summary) = parse_session_summary(&path) {
            projects
                .entry(summary.cwd.clone())
                .or_default()
                .push(summary);
        }
    }

    let mut sorted_projects = BTreeMap::new();
    for (cwd, mut sessions) in projects {
        sessions.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        sorted_projects.insert(cwd, sessions);
    }

    Ok(sorted_projects
        .into_iter()
        .map(|(cwd, sessions)| ProjectBucket { cwd, sessions })
        .collect())
}

fn collect_jsonl_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(root).with_context(|| format!("failed to read {}", root.display()))? {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;

        if metadata.is_dir() {
            collect_jsonl_files(&path, files)?;
            continue;
        }

        if metadata.is_file()
            && path
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|name| name.ends_with(".jsonl"))
        {
            files.push(path);
        }
    }

    Ok(())
}

fn parse_session_summary(path: &Path) -> Result<SessionSummary> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;

    let mut session_id = String::from("unknown");
    let mut cwd = String::from("<unknown>");
    let mut started_at = String::from("unknown");
    let mut event_count = 0usize;
    let mut search_parts = Vec::new();

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }

        event_count += 1;

        let value: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        match value.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                if let Some(payload) = value.get("payload") {
                    if let Some(id) = payload.get("id").and_then(Value::as_str) {
                        session_id = id.to_string();
                    }
                    if let Some(session_cwd) = payload.get("cwd").and_then(Value::as_str) {
                        cwd = session_cwd.to_string();
                    }
                    if let Some(ts) = payload.get("timestamp").and_then(Value::as_str) {
                        started_at = ts.to_string();
                    }
                }
            }
            Some("response_item") => {
                if let Some(payload) = value.get("payload")
                    && payload.get("type").and_then(Value::as_str) == Some("message")
                {
                    if let Some(content_items) = payload.get("content").and_then(Value::as_array) {
                        for item in content_items {
                            if let Some(text) = item
                                .get("text")
                                .or_else(|| item.get("input_text"))
                                .or_else(|| item.get("output_text"))
                                .and_then(Value::as_str)
                            {
                                search_parts.push(text.to_lowercase());
                            }
                        }
                    }
                }
            }
            Some("event_msg") => {
                if let Some(payload) = value.get("payload")
                    && payload.get("type").and_then(Value::as_str) == Some("user_message")
                    && let Some(text) = payload.get("message").and_then(Value::as_str)
                {
                    search_parts.push(text.to_lowercase());
                }
            }
            _ => {}
        };
    }

    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("<unknown>")
        .to_string();

    Ok(SessionSummary {
        path: path.to_path_buf(),
        file_name,
        id: session_id,
        cwd,
        started_at,
        event_count,
        search_blob: search_parts.join("\n"),
    })
}

fn rewrite_session_file(path: &Path, target_cwd: &str, rewrite_id: bool) -> Result<()> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;

    let new_id = if rewrite_id {
        Some(Uuid::new_v4().to_string())
    } else {
        None
    };

    let mut out = String::with_capacity(content.len() + 1024);
    for line in content.lines() {
        if line.trim().is_empty() {
            out.push('\n');
            continue;
        }

        let mut value: Value = serde_json::from_str(line)
            .with_context(|| format!("invalid JSON line in {}", path.display()))?;

        rewrite_cwd_fields(&mut value, target_cwd);
        if let Some(id) = new_id.as_deref() {
            rewrite_session_id(&mut value, id);
        }

        out.push_str(&serde_json::to_string(&value)?);
        out.push('\n');
    }

    backup_file(path)?;
    atomic_write(path, &out)?;
    Ok(())
}

fn duplicate_session_file(
    sessions_root: &Path,
    source: &SessionSummary,
    target_cwd: &str,
    fork: bool,
) -> Result<PathBuf> {
    let content = fs::read_to_string(&source.path)
        .with_context(|| format!("failed to read {}", source.path.display()))?;

    let new_id = if fork {
        Some(Uuid::new_v4().to_string())
    } else {
        None
    };

    let mut out = String::with_capacity(content.len() + 1024);
    for line in content.lines() {
        if line.trim().is_empty() {
            out.push('\n');
            continue;
        }

        let mut value: Value = serde_json::from_str(line)
            .with_context(|| format!("invalid JSON line in {}", source.path.display()))?;

        rewrite_cwd_fields(&mut value, target_cwd);
        if let Some(id) = new_id.as_deref() {
            rewrite_session_id(&mut value, id);
            rewrite_session_start_timestamp(&mut value);
        }

        out.push_str(&serde_json::to_string(&value)?);
        out.push('\n');
    }

    let now = Utc::now();
    let mut target_path = sessions_root
        .join(now.format("%Y").to_string())
        .join(now.format("%m").to_string())
        .join(now.format("%d").to_string());

    fs::create_dir_all(&target_path)
        .with_context(|| format!("failed to create {}", target_path.display()))?;

    let id_for_name = new_id.unwrap_or_else(|| source.id.clone());
    let file_name = format!(
        "rollout-{}-{}.jsonl",
        now.format("%Y-%m-%dT%H-%M-%S"),
        id_for_name
    );

    target_path.push(file_name);
    let final_path = unique_path(target_path);

    atomic_write(&final_path, &out)?;
    Ok(final_path)
}

fn rewrite_cwd_fields(value: &mut Value, target_cwd: &str) {
    match value {
        Value::Object(obj) => {
            for (key, val) in obj.iter_mut() {
                if key == "cwd" && val.is_string() {
                    *val = Value::String(target_cwd.to_string());
                } else {
                    rewrite_cwd_fields(val, target_cwd);
                }
            }
        }
        Value::Array(arr) => {
            for item in arr {
                rewrite_cwd_fields(item, target_cwd);
            }
        }
        _ => {}
    }
}

fn rewrite_session_id(value: &mut Value, new_id: &str) {
    if value.get("type").and_then(Value::as_str) != Some("session_meta") {
        return;
    }

    let Some(payload) = value.get_mut("payload") else {
        return;
    };

    let Value::Object(payload_obj) = payload else {
        return;
    };

    payload_obj.insert("id".to_string(), Value::String(new_id.to_string()));
}

fn rewrite_session_start_timestamp(value: &mut Value) {
    if value.get("type").and_then(Value::as_str) != Some("session_meta") {
        return;
    }

    let now = DateTime::<Utc>::from(SystemTime::now()).to_rfc3339();
    let Some(payload) = value.get_mut("payload") else {
        return;
    };

    let Value::Object(payload_obj) = payload else {
        return;
    };

    payload_obj.insert("timestamp".to_string(), Value::String(now));
}

fn backup_file(path: &Path) -> Result<()> {
    let ts = Utc::now().format("%Y%m%d%H%M%S");
    let backup = path.with_extension(format!("jsonl.bak.{ts}"));
    fs::copy(path, &backup).with_context(|| {
        format!(
            "failed to create backup {} from {}",
            backup.display(),
            path.display()
        )
    })?;
    Ok(())
}

fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let mut tmp = path.to_path_buf();
    tmp.set_extension("jsonl.tmp");

    fs::write(&tmp, content).with_context(|| format!("failed writing {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("failed renaming {} to {}", tmp.display(), path.display()))?;
    Ok(())
}

fn unique_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }

    let parent = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("rollout")
        .to_string();
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("jsonl")
        .to_string();

    for idx in 1..10_000 {
        let candidate = parent.join(format!("{stem}-{idx}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }

    parent.join(format!("{stem}-{}.{}", Uuid::new_v4(), ext))
}

fn resolve_codex_home() -> Result<PathBuf> {
    if let Ok(path) = env::var("CODEX_HOME") {
        let expanded = expand_tilde(path.trim());
        if !expanded.as_os_str().is_empty() {
            return Ok(expanded);
        }
    }

    let home = env::var("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".codex"))
}

fn expand_tilde(input: &str) -> PathBuf {
    if input.is_empty() {
        return PathBuf::new();
    }

    if input == "~" {
        if let Ok(home) = env::var("HOME") {
            return PathBuf::from(home);
        }
    }

    if let Some(rest) = input.strip_prefix("~/") {
        if let Ok(home) = env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }

    PathBuf::from(input)
}
