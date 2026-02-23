use std::collections::HashSet;
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs;
use std::io::{self, Stdout, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, anyhow};
use base64::Engine as _;
use chrono::{DateTime, Utc};
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use pulldown_cmark::{Event as MdEvent, Options as MdOptions, Parser as MdParser, Tag, TagEnd};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState, Wrap,
};
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
                        if handle_normal_mode(key, app)? {
                            return Ok(());
                        }
                    }
                    Mode::Input => handle_input_mode(key, app)?,
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
            if let Some(target) = scrollbar_target_at(mouse.column, mouse.row, app) {
                app.scroll_drag = Some(target);
                jump_to_scroll_from_mouse(target, mouse.row, app);
                return;
            }
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
                    app.session_select_anchor = None;
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
                let idx =
                    app.session_scroll + (mouse_row_to_index(mouse.row, app.panes.sessions) / 2);
                if idx < len {
                    let checkbox_hit =
                        is_sessions_checkbox_hit(mouse.column, mouse.row, app.panes.sessions);
                    if checkbox_hit {
                        app.session_idx = idx;
                        app.toggle_current_session_selection();
                    } else {
                        app.session_idx = idx;
                        app.session_select_anchor = Some(idx);
                    }
                    app.preview_scroll = 0;
                    app.ensure_selection_visible();
                }
            } else if point_in_rect(mouse.column, mouse.row, app.panes.preview) {
                app.search_focused = false;
                if app.mode == Mode::Input {
                    app.input_focused = false;
                }
                app.focus = Focus::Preview;
                let row = app.preview_scroll + mouse_row_to_index(mouse.row, app.panes.preview);
                let col = mouse_col_to_index(mouse.column, app.panes.preview);
                if let Some((_, turn_idx)) = app
                    .preview_header_rows
                    .iter()
                    .filter(|(header_row, _)| *header_row <= row)
                    .max_by_key(|(header_row, _)| *header_row)
                {
                    app.preview_focus_turn = Some(*turn_idx);
                }
                app.preview_mouse_down_pos = Some(app.clamp_preview_pos(row, col));
                app.preview_selecting = false;
                app.preview_selection = None;
            } else if point_in_rect(mouse.column, mouse.row, app.panes.status) {
                app.search_focused = false;
                handle_status_click(mouse.column, mouse.row, app);
            }
        }
        MouseEventKind::Down(MouseButton::Right) => {
            // Intentionally do nothing. Some terminals can still show context menus
            // even with mouse reporting enabled depending on configuration.
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(target) = app.scroll_drag {
                jump_to_scroll_from_mouse(target, mouse.row, app);
                return;
            }
            if let Some(target) = app.drag_target {
                app.resize_from_mouse(target, mouse.column);
                return;
            }
            if let Some(start) = app.preview_mouse_down_pos
                && point_in_rect(mouse.column, mouse.row, app.panes.preview)
            {
                let row = app.preview_scroll + mouse_row_to_index(mouse.row, app.panes.preview);
                let col = mouse_col_to_index(mouse.column, app.panes.preview);
                let current = app.clamp_preview_pos(row, col);
                if current != start {
                    app.preview_selection = Some((start, current));
                    app.preview_selecting = true;
                }
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if app.scroll_drag.is_some() || app.drag_target.is_some() {
                app.scroll_drag = None;
                app.drag_target = None;
                return;
            }
            if let Some(start) = app.preview_mouse_down_pos.take() {
                let row = app.preview_scroll + mouse_row_to_index(mouse.row, app.panes.preview);
                let col = mouse_col_to_index(mouse.column, app.panes.preview);
                let current = app.clamp_preview_pos(row, col);
                if app.preview_selecting {
                    app.preview_selection = Some((start, current));
                    if let Some((a, b)) = app.preview_selection
                        && let Some(text) = app.preview_selected_text(a, b)
                    {
                        if copy_to_clipboard_osc52(&text).is_ok() {
                            let line_count =
                                a.0.max(b.0).saturating_sub(a.0.min(b.0)).saturating_add(1);
                            app.status =
                                format!("Copied selection ({} line(s)) to clipboard", line_count);
                        } else {
                            app.status = String::from("Selection captured (clipboard copy failed)");
                        }
                    }
                } else if point_in_rect(mouse.column, mouse.row, app.panes.preview) {
                    app.preview_selection = None;
                    app.toggle_fold_by_row(current.0);
                } else {
                    app.preview_selection = None;
                }
            }
            app.preview_selecting = false;
            app.scroll_drag = None;
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum StatusButton {
    Apply,
    Cancel,
    SelectAll,
    Invert,
    Move,
    Copy,
    Fork,
    Delete,
    ProjectRename,
    ProjectCopy,
    Refresh,
    Quit,
}

fn status_buttons(app: &App) -> Vec<StatusButton> {
    if app.mode == Mode::Input {
        return vec![StatusButton::Apply, StatusButton::Cancel];
    }
    if app.focus == Focus::Projects {
        return vec![
            StatusButton::ProjectRename,
            StatusButton::ProjectCopy,
            StatusButton::Refresh,
            StatusButton::Quit,
        ];
    }
    if app.focus == Focus::Sessions {
        return vec![
            StatusButton::SelectAll,
            StatusButton::Invert,
            StatusButton::Move,
            StatusButton::Copy,
            StatusButton::Fork,
            StatusButton::Delete,
            StatusButton::Refresh,
            StatusButton::Quit,
        ];
    }
    vec![
        StatusButton::Move,
        StatusButton::Copy,
        StatusButton::Fork,
        StatusButton::Delete,
        StatusButton::Refresh,
        StatusButton::Quit,
    ]
}

fn status_button_label(button: StatusButton) -> &'static str {
    match button {
        StatusButton::Apply => "[Apply]",
        StatusButton::Cancel => "[Cancel]",
        StatusButton::SelectAll => "[Select All]",
        StatusButton::Invert => "[Invert]",
        StatusButton::Move => "[Move]",
        StatusButton::Copy => "[Copy]",
        StatusButton::Fork => "[Fork]",
        StatusButton::Delete => "[Delete]",
        StatusButton::ProjectRename => "[Rename Folder]",
        StatusButton::ProjectCopy => "[Copy Folder]",
        StatusButton::Refresh => "[Refresh]",
        StatusButton::Quit => "[Quit]",
    }
}

fn trigger_status_button(button: StatusButton, app: &mut App) {
    match button {
        StatusButton::Apply => {
            let _ = app.submit_input();
        }
        StatusButton::Cancel => app.cancel_input(),
        StatusButton::SelectAll => app.select_all_sessions_current_project(),
        StatusButton::Invert => app.invert_sessions_selection_current_project(),
        StatusButton::Move => app.start_action(Action::Move),
        StatusButton::Copy => app.start_action(Action::Copy),
        StatusButton::Fork => app.start_action(Action::Fork),
        StatusButton::Delete => app.start_action(Action::Delete),
        StatusButton::ProjectRename => app.start_action(Action::ProjectRename),
        StatusButton::ProjectCopy => app.start_action(Action::ProjectCopy),
        StatusButton::Refresh => {
            let _ = app.reload();
        }
        StatusButton::Quit => app.status = String::from("Use q to quit"),
    }
}

fn handle_status_click(x: u16, y: u16, app: &mut App) {
    let content_y = app.panes.status.y.saturating_add(1);
    let controls_y = content_y.saturating_add(2);
    if y == controls_y {
        let mut cursor = 0u16;
        let rel_x = x.saturating_sub(app.panes.status.x.saturating_add(1));
        for button in status_buttons(app) {
            let label = status_button_label(button);
            let width = label.chars().count() as u16;
            if rel_x >= cursor && rel_x < cursor.saturating_add(width) {
                trigger_status_button(button, app);
                break;
            }
            cursor = cursor.saturating_add(width + 1);
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

fn scrollbar_target_at(x: u16, y: u16, app: &App) -> Option<ScrollTarget> {
    if is_on_scrollbar(x, y, app.panes.projects) {
        return Some(ScrollTarget::Projects);
    }
    if is_on_scrollbar(x, y, app.panes.sessions) {
        return Some(ScrollTarget::Sessions);
    }
    if is_on_scrollbar(x, y, app.panes.preview) {
        return Some(ScrollTarget::Preview);
    }
    None
}

fn is_on_scrollbar(x: u16, y: u16, pane: ratatui::layout::Rect) -> bool {
    if pane.width < 2 || pane.height < 3 {
        return false;
    }
    let bar_x = pane.x.saturating_add(pane.width.saturating_sub(1));
    let y0 = pane.y.saturating_add(1);
    let y1 = pane.y.saturating_add(pane.height.saturating_sub(1));
    x == bar_x && y >= y0 && y < y1
}

fn scroll_offset_from_mouse_row(
    y: u16,
    pane: ratatui::layout::Rect,
    content_len: usize,
    viewport_len: usize,
) -> usize {
    if viewport_len == 0 || content_len <= viewport_len || pane.height <= 2 {
        return 0;
    }
    let inner_h = pane.height.saturating_sub(2) as usize;
    let rel = y.saturating_sub(pane.y.saturating_add(1)) as usize;
    let rel = rel.min(inner_h.saturating_sub(1));
    let max_off = content_len.saturating_sub(viewport_len);
    if inner_h <= 1 {
        return max_off;
    }
    ((rel as f32 / (inner_h.saturating_sub(1) as f32)) * max_off as f32).round() as usize
}

fn jump_to_scroll_from_mouse(target: ScrollTarget, y: u16, app: &mut App) {
    match target {
        ScrollTarget::Projects => {
            let viewport = App::visible_rows(app.panes.projects.height, 1);
            let off =
                scroll_offset_from_mouse_row(y, app.panes.projects, app.projects.len(), viewport);
            app.project_scroll = off;
            app.focus = Focus::Projects;
        }
        ScrollTarget::Sessions => {
            let len = app.current_project().map(|p| p.sessions.len()).unwrap_or(0);
            let viewport = App::visible_rows(app.panes.sessions.height, 2);
            let off = scroll_offset_from_mouse_row(y, app.panes.sessions, len, viewport);
            app.session_scroll = off;
            app.focus = Focus::Sessions;
        }
        ScrollTarget::Preview => {
            let viewport = app.panes.preview.height.saturating_sub(2) as usize;
            let off = scroll_offset_from_mouse_row(
                y,
                app.panes.preview,
                app.preview_content_len,
                viewport,
            );
            app.preview_scroll = off;
            app.focus = Focus::Preview;
        }
    }
}

fn mouse_row_to_index(y: u16, pane: ratatui::layout::Rect) -> usize {
    // Exclude the top border/title row.
    y.saturating_sub(pane.y.saturating_add(1)) as usize
}

fn mouse_col_to_index(x: u16, pane: ratatui::layout::Rect) -> usize {
    // Exclude the left border.
    x.saturating_sub(pane.x.saturating_add(1)) as usize
}

fn is_sessions_checkbox_hit(x: u16, y: u16, pane: ratatui::layout::Rect) -> bool {
    let row = mouse_row_to_index(y, pane);
    let col = mouse_col_to_index(x, pane);
    // Session items are 2 rows high. Checkbox is rendered on the first row and
    // appears after list's highlight gutter, so allow a small left-column band.
    row.is_multiple_of(2) && col <= 7
}

fn copy_to_clipboard_osc52(text: &str) -> Result<()> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let mut out = io::stdout();
    write!(out, "\x1b]52;c;{b64}\x1b\\").context("failed OSC52 write")?;
    out.flush().context("failed stdout flush")?;
    Ok(())
}

fn handle_normal_mode(key: KeyEvent, app: &mut App) -> Result<bool> {
    let disallowed_mods = KeyModifiers::CONTROL | KeyModifiers::ALT;
    if app.search_focused {
        match key.code {
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
                if !key.modifiers.intersects(disallowed_mods) {
                    app.search_query.push(ch);
                    app.search_dirty = true;
                }
            }
            _ => {}
        }
        return Ok(false);
    }

    if key.modifiers.intersects(disallowed_mods) {
        return Ok(false);
    }

    match key.code {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Char('/') => {
            app.search_focused = true;
        }
        KeyCode::Char(' ') => {
            if app.focus == Focus::Sessions {
                app.toggle_current_session_selection();
            }
        }
        KeyCode::Char('a') => {
            if app.focus == Focus::Sessions {
                app.select_all_sessions_current_project();
            }
        }
        KeyCode::Char('i') => {
            if app.focus == Focus::Sessions {
                app.invert_sessions_selection_current_project();
            }
        }
        KeyCode::Tab => {
            if app.focus == Focus::Preview {
                app.toggle_fold_focused_preview_turn();
            } else {
                app.next_focus();
            }
        }
        KeyCode::BackTab => {
            if app.focus == Focus::Preview {
                app.toggle_fold_all_preview_turns();
            } else {
                app.prev_focus();
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.focus == Focus::Preview {
                app.focus_prev_preview_turn();
            } else {
                app.move_up();
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.focus == Focus::Preview {
                app.focus_next_preview_turn();
            } else {
                app.move_down();
            }
        }
        KeyCode::Left => {
            if app.focus == Focus::Preview {
                app.fold_focused_preview_turn();
            }
        }
        KeyCode::Right => {
            if app.focus == Focus::Preview {
                app.unfold_focused_preview_turn();
            }
        }
        KeyCode::Char('g') => app.reload()?,
        KeyCode::Char('m') => {
            if app.focus == Focus::Projects {
                app.start_action(Action::ProjectRename);
            } else {
                app.start_action(Action::Move);
            }
        }
        KeyCode::Char('c') => {
            if app.focus == Focus::Projects {
                app.start_action(Action::ProjectCopy);
            } else {
                app.start_action(Action::Copy);
            }
        }
        KeyCode::Char('f') => {
            if app.focus == Focus::Projects {
                app.status = String::from("Project scope supports rename/copy");
            } else {
                app.start_action(Action::Fork);
            }
        }
        KeyCode::Char('d') | KeyCode::Delete => {
            if app.focus != Focus::Projects {
                app.start_action(Action::Delete);
            }
        }
        KeyCode::Char('r') => {
            if app.focus == Focus::Projects {
                app.start_action(Action::ProjectRename);
            }
        }
        KeyCode::Char('y') => {
            if app.focus == Focus::Projects {
                app.start_action(Action::ProjectCopy);
            }
        }
        KeyCode::Char('v') => app.toggle_preview_mode(),
        KeyCode::Char('z') => app.toggle_fold_at_scroll(),
        KeyCode::Char('H') | KeyCode::Char('h') => app.resize_focused_pane(-2),
        KeyCode::Char('L') | KeyCode::Char('l') => app.resize_focused_pane(2),
        _ => {}
    }

    Ok(false)
}

fn handle_input_mode(key: KeyEvent, app: &mut App) -> Result<()> {
    let disallowed_mods = KeyModifiers::CONTROL | KeyModifiers::ALT;
    match key.code {
        KeyCode::Esc => {
            app.clear_input_completion_cycle();
            app.cancel_input();
        }
        KeyCode::Enter => {
            app.clear_input_completion_cycle();
            app.submit_input()?;
        }
        KeyCode::Tab => {
            if app.input_focused && !key.modifiers.intersects(disallowed_mods) {
                app.tab_complete_input_path();
            }
        }
        KeyCode::Backspace => {
            if app.input_focused {
                app.input.pop();
                app.clear_input_completion_cycle();
            }
        }
        KeyCode::Char(ch) => {
            if app.input_focused && !key.modifiers.intersects(disallowed_mods) {
                app.input.push(ch);
                app.clear_input_completion_cycle();
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
        execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;
        // Match edit's conservative mouse tracking (1002 + SGR 1006) instead of
        // crossterm's default capture set, which also enables 1003.
        write!(stdout, "\x1b[?1002;1006h").context("failed to enable mouse reporting")?;
        stdout
            .flush()
            .context("failed to flush mouse reporting setup")?;
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
        write!(self.terminal.backend_mut(), "\x1b[?1006;1002l")
            .context("failed to disable mouse reporting")?;
        self.terminal
            .backend_mut()
            .flush()
            .context("failed to flush mouse reporting disable")?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)
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
    Delete,
    ProjectRename,
    ProjectCopy,
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScrollTarget {
    Projects,
    Sessions,
    Preview,
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
    selected_sessions: HashSet<PathBuf>,
    session_select_anchor: Option<usize>,
    focus: Focus,
    mode: Mode,
    pending_action: Option<Action>,
    input: String,
    input_focused: bool,
    input_tab_last_at: Option<Instant>,
    input_tab_last_query: String,
    search_query: String,
    search_focused: bool,
    search_dirty: bool,
    preview_mode: PreviewMode,
    preview_selecting: bool,
    preview_mouse_down_pos: Option<(usize, usize)>,
    drag_target: Option<DragTarget>,
    scroll_drag: Option<ScrollTarget>,
    status: String,
    panes: PaneLayout,
    project_width_pct: u16,
    session_width_pct: u16,
    project_scroll: usize,
    session_scroll: usize,
    preview_scroll: usize,
    preview_content_len: usize,
    preview_selection: Option<((usize, usize), (usize, usize))>,
    preview_rendered_lines: Vec<String>,
    preview_focus_turn: Option<usize>,
    preview_cache: HashMap<PathBuf, CachedPreviewSource>,
    preview_folded: HashMap<PathBuf, HashSet<usize>>,
    preview_header_rows: Vec<(usize, usize)>,
    preview_session_path: Option<PathBuf>,
}

#[derive(Clone)]
struct CachedPreviewSource {
    mtime: SystemTime,
    turns: Vec<ChatTurn>,
    events: Vec<String>,
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
            selected_sessions: HashSet::new(),
            session_select_anchor: None,
            focus: Focus::Projects,
            mode: Mode::Normal,
            pending_action: None,
            input: String::new(),
            input_focused: false,
            input_tab_last_at: None,
            input_tab_last_query: String::new(),
            search_query: String::new(),
            search_focused: false,
            search_dirty: false,
            preview_mode: PreviewMode::Chat,
            preview_selecting: false,
            preview_mouse_down_pos: None,
            drag_target: None,
            scroll_drag: None,
            status: String::from("Press q to quit, g to refresh"),
            panes: PaneLayout::default(),
            project_width_pct: 20,
            session_width_pct: 38,
            project_scroll: 0,
            session_scroll: 0,
            preview_scroll: 0,
            preview_content_len: 0,
            preview_selection: None,
            preview_rendered_lines: Vec::new(),
            preview_focus_turn: None,
            preview_cache: HashMap::new(),
            preview_folded: HashMap::new(),
            preview_header_rows: Vec::new(),
            preview_session_path: None,
        };

        app.reload()?;
        Ok(app)
    }

    fn reload(&mut self) -> Result<()> {
        self.all_projects = scan_sessions(&self.sessions_root)?;
        self.prune_selected_sessions();
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

    fn prune_selected_sessions(&mut self) {
        let valid = self
            .all_projects
            .iter()
            .flat_map(|project| project.sessions.iter().map(|s| s.path.clone()))
            .collect::<HashSet<_>>();
        self.selected_sessions.retain(|p| valid.contains(p));
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
                self.session_select_anchor = None;
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
                self.session_select_anchor = None;
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

    fn visible_rows(pane_height: u16, item_height: usize) -> usize {
        let rows = pane_height.saturating_sub(2) as usize;
        (rows / item_height.max(1)).max(1)
    }

    fn ensure_selection_visible(&mut self) {
        let project_visible = Self::visible_rows(self.panes.projects.height, 1);
        if self.project_idx < self.project_scroll {
            self.project_scroll = self.project_idx;
        } else if self.project_idx >= self.project_scroll + project_visible {
            self.project_scroll = self.project_idx + 1 - project_visible;
        }

        let session_visible = Self::visible_rows(self.panes.sessions.height, 2);
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

    fn preview_for_session(
        &mut self,
        session: &SessionSummary,
        mode: PreviewMode,
        inner_width: usize,
    ) -> Result<PreviewData> {
        let meta = fs::metadata(&session.path)
            .with_context(|| format!("failed metadata {}", session.path.display()))?;
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);

        let stale = self
            .preview_cache
            .get(&session.path)
            .is_none_or(|cached| cached.mtime < mtime);

        if stale {
            let content = fs::read_to_string(&session.path)
                .with_context(|| format!("failed to read {}", session.path.display()))?;
            let turns = extract_chat_turns(&content);
            let events = content
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| {
                    serde_json::from_str::<Value>(line)
                        .map(|v| summarize_event_line(&v))
                        .unwrap_or_else(|_| String::from("<invalid event>"))
                })
                .collect::<Vec<_>>();
            self.preview_cache.insert(
                session.path.clone(),
                CachedPreviewSource {
                    mtime,
                    turns,
                    events,
                },
            );
        }

        let cached = self
            .preview_cache
            .get(&session.path)
            .ok_or_else(|| anyhow!("preview cache missing"))?;
        let folded = self
            .preview_folded
            .get(&session.path)
            .cloned()
            .unwrap_or_default();
        Ok(build_preview_from_cached(
            session,
            mode,
            inner_width,
            cached,
            &folded,
        ))
    }

    fn toggle_fold_by_row(&mut self, row: usize) {
        let Some(path) = self.preview_session_path.clone() else {
            return;
        };
        let Some((_, turn_idx)) = self
            .preview_header_rows
            .iter()
            .find(|(header_row, _)| *header_row == row)
            .copied()
        else {
            return;
        };

        let entry = self.preview_folded.entry(path).or_default();
        if entry.contains(&turn_idx) {
            entry.remove(&turn_idx);
        } else {
            entry.insert(turn_idx);
        }
        self.preview_focus_turn = Some(turn_idx);
    }

    fn toggle_fold_at_scroll(&mut self) {
        if self.focus != Focus::Preview {
            return;
        }
        let row = self.preview_scroll;
        let target_row = self
            .preview_header_rows
            .iter()
            .find(|(header_row, _)| *header_row >= row)
            .map(|(header_row, _)| *header_row)
            .or_else(|| {
                self.preview_header_rows
                    .last()
                    .map(|(header_row, _)| *header_row)
            });
        if let Some(r) = target_row {
            self.toggle_fold_by_row(r);
        }
    }

    fn ensure_preview_focus_valid(&mut self) {
        if self.preview_header_rows.is_empty() {
            self.preview_focus_turn = None;
            return;
        }
        let turn_ids = self
            .preview_header_rows
            .iter()
            .map(|(_, t)| *t)
            .collect::<Vec<_>>();
        if let Some(focused) = self.preview_focus_turn
            && turn_ids.contains(&focused)
        {
            return;
        }
        self.preview_focus_turn = turn_ids.first().copied();
    }

    fn focus_next_preview_turn(&mut self) {
        self.ensure_preview_focus_valid();
        let Some(current) = self.preview_focus_turn else {
            return;
        };
        let turns = self
            .preview_header_rows
            .iter()
            .map(|(_, t)| *t)
            .collect::<Vec<_>>();
        let Some(pos) = turns.iter().position(|t| *t == current) else {
            return;
        };
        let next = (pos + 1).min(turns.len().saturating_sub(1));
        self.preview_focus_turn = Some(turns[next]);
        self.scroll_preview_focus_into_view();
    }

    fn focus_prev_preview_turn(&mut self) {
        self.ensure_preview_focus_valid();
        let Some(current) = self.preview_focus_turn else {
            return;
        };
        let turns = self
            .preview_header_rows
            .iter()
            .map(|(_, t)| *t)
            .collect::<Vec<_>>();
        let Some(pos) = turns.iter().position(|t| *t == current) else {
            return;
        };
        let prev = pos.saturating_sub(1);
        self.preview_focus_turn = Some(turns[prev]);
        self.scroll_preview_focus_into_view();
    }

    fn scroll_preview_focus_into_view(&mut self) {
        let Some(focused) = self.preview_focus_turn else {
            return;
        };
        let Some((row, _)) = self
            .preview_header_rows
            .iter()
            .find(|(_, t)| *t == focused)
            .copied()
        else {
            return;
        };
        let visible = self.panes.preview.height.saturating_sub(2) as usize;
        if visible == 0 {
            return;
        }
        if row < self.preview_scroll {
            self.preview_scroll = row;
        } else if row >= self.preview_scroll + visible {
            self.preview_scroll = row + 1 - visible;
        }
    }

    fn toggle_fold_focused_preview_turn(&mut self) {
        self.ensure_preview_focus_valid();
        let Some(focused) = self.preview_focus_turn else {
            return;
        };
        let Some((row, _)) = self
            .preview_header_rows
            .iter()
            .find(|(_, t)| *t == focused)
            .copied()
        else {
            return;
        };
        self.toggle_fold_by_row(row);
        self.scroll_preview_focus_into_view();
    }

    fn fold_focused_preview_turn(&mut self) {
        self.ensure_preview_focus_valid();
        let (Some(path), Some(focused)) =
            (self.preview_session_path.clone(), self.preview_focus_turn)
        else {
            return;
        };
        self.preview_folded.entry(path).or_default().insert(focused);
        self.scroll_preview_focus_into_view();
    }

    fn unfold_focused_preview_turn(&mut self) {
        self.ensure_preview_focus_valid();
        let (Some(path), Some(focused)) =
            (self.preview_session_path.clone(), self.preview_focus_turn)
        else {
            return;
        };
        self.preview_folded
            .entry(path)
            .or_default()
            .remove(&focused);
        self.scroll_preview_focus_into_view();
    }

    fn toggle_fold_all_preview_turns(&mut self) {
        let Some(path) = self.preview_session_path.clone() else {
            return;
        };
        let turns = self
            .preview_header_rows
            .iter()
            .map(|(_, t)| *t)
            .collect::<Vec<_>>();
        if turns.is_empty() {
            return;
        }
        let entry = self.preview_folded.entry(path).or_default();
        let all_folded = turns.iter().all(|t| entry.contains(t));
        if all_folded {
            for turn in turns {
                entry.remove(&turn);
            }
            self.status = String::from("Expanded all preview blocks");
        } else {
            for turn in turns {
                entry.insert(turn);
            }
            self.status = String::from("Collapsed all preview blocks");
        }
    }

    fn clamp_preview_pos(&self, row: usize, col: usize) -> (usize, usize) {
        if self.preview_rendered_lines.is_empty() {
            return (0, 0);
        }
        let row = row.min(self.preview_rendered_lines.len().saturating_sub(1));
        let len = self.preview_rendered_lines[row].chars().count();
        let col = if len == 0 {
            0
        } else {
            col.min(len.saturating_sub(1))
        };
        (row, col)
    }

    fn preview_selected_text(&self, start: (usize, usize), end: (usize, usize)) -> Option<String> {
        if self.preview_rendered_lines.is_empty() {
            return None;
        }
        let start = self.clamp_preview_pos(start.0, start.1);
        let end = self.clamp_preview_pos(end.0, end.1);
        let (beg, fin) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        if beg.0 == fin.0 {
            let line = &self.preview_rendered_lines[beg.0];
            return Some(slice_chars(line, beg.1, fin.1.saturating_add(1)));
        }
        let mut out = Vec::new();
        let first = &self.preview_rendered_lines[beg.0];
        out.push(slice_chars(first, beg.1, first.chars().count()));
        for row in (beg.0 + 1)..fin.0 {
            out.push(self.preview_rendered_lines[row].clone());
        }
        let last = &self.preview_rendered_lines[fin.0];
        out.push(slice_chars(last, 0, fin.1.saturating_add(1)));
        Some(out.join("\n"))
    }

    fn current_project(&self) -> Option<&ProjectBucket> {
        self.projects.get(self.project_idx)
    }

    fn current_session(&self) -> Option<&SessionSummary> {
        self.current_project()
            .and_then(|project| project.sessions.get(self.session_idx))
    }

    fn selected_sessions_in_current_project(&self) -> Vec<SessionSummary> {
        let Some(project) = self.current_project() else {
            return Vec::new();
        };
        project
            .sessions
            .iter()
            .filter(|s| self.selected_sessions.contains(&s.path))
            .cloned()
            .collect()
    }

    fn selected_count_current_project(&self) -> usize {
        self.selected_sessions_in_current_project().len()
    }

    fn toggle_current_session_selection(&mut self) {
        let Some(session) = self.current_session().cloned() else {
            self.status = String::from("No session selected");
            return;
        };
        if self.selected_sessions.contains(&session.path) {
            self.selected_sessions.remove(&session.path);
        } else {
            self.selected_sessions.insert(session.path.clone());
        }
        self.session_select_anchor = Some(self.session_idx);
        self.status = format!(
            "Selected {} session(s)",
            self.selected_count_current_project()
        );
    }

    fn select_all_sessions_current_project(&mut self) {
        let Some(project) = self.current_project() else {
            return;
        };
        let paths = project
            .sessions
            .iter()
            .map(|s| s.path.clone())
            .collect::<Vec<_>>();
        let project_len = project.sessions.len();
        for path in paths {
            self.selected_sessions.insert(path);
        }
        if project_len > 0 {
            self.session_select_anchor = Some(self.session_idx.min(project_len - 1));
        }
        self.status = format!(
            "Selected {} session(s)",
            self.selected_count_current_project()
        );
    }

    fn invert_sessions_selection_current_project(&mut self) {
        let Some(project) = self.current_project() else {
            return;
        };
        let paths = project
            .sessions
            .iter()
            .map(|s| s.path.clone())
            .collect::<Vec<_>>();
        let project_len = project.sessions.len();
        for path in paths {
            if self.selected_sessions.contains(&path) {
                self.selected_sessions.remove(&path);
            } else {
                self.selected_sessions.insert(path);
            }
        }
        if project_len > 0 {
            self.session_select_anchor = Some(self.session_idx.min(project_len - 1));
        }
        self.status = format!(
            "Selected {} session(s)",
            self.selected_count_current_project()
        );
    }

    fn action_targets(&self, action: Action) -> Vec<SessionSummary> {
        match action {
            Action::ProjectRename | Action::ProjectCopy => self
                .current_project()
                .map(|p| p.sessions.clone())
                .unwrap_or_default(),
            Action::Move | Action::Copy | Action::Fork | Action::Delete => {
                let selected = self.selected_sessions_in_current_project();
                if !selected.is_empty() {
                    selected
                } else {
                    self.current_session().cloned().into_iter().collect()
                }
            }
        }
    }

    fn start_action(&mut self, action: Action) {
        let targets = self.action_targets(action);
        if targets.is_empty() {
            self.status = match action {
                Action::ProjectRename | Action::ProjectCopy => String::from("No project selected"),
                _ => String::from("No session selected"),
            };
            return;
        }

        self.mode = Mode::Input;
        self.pending_action = Some(action);
        self.input.clear();
        self.input_focused = true;
        self.clear_input_completion_cycle();
        self.search_focused = false;
        self.status = match action {
            Action::Move => format!(
                "Move {} session(s): enter target project path and press Enter",
                targets.len()
            ),
            Action::Copy => format!(
                "Copy {} session(s): enter target project path and press Enter",
                targets.len()
            ),
            Action::Fork => format!(
                "Fork {} session(s): enter target project path and press Enter",
                targets.len()
            ),
            Action::Delete => format!(
                "Delete {} session(s): type DELETE and press Enter",
                targets.len()
            ),
            Action::ProjectRename => format!(
                "Rename folder sessions ({}) to target path and press Enter",
                targets.len()
            ),
            Action::ProjectCopy => format!(
                "Copy folder sessions ({}) to target path and press Enter",
                targets.len()
            ),
        };
    }

    fn cancel_input(&mut self) {
        self.mode = Mode::Normal;
        self.pending_action = None;
        self.input.clear();
        self.input_focused = false;
        self.clear_input_completion_cycle();
        self.status = String::from("Action cancelled");
    }

    fn submit_input(&mut self) -> Result<()> {
        let Some(action) = self.pending_action else {
            self.cancel_input();
            return Ok(());
        };

        let targets = self.action_targets(action);
        if targets.is_empty() {
            self.status = String::from("No applicable sessions for this action");
            return Ok(());
        }
        let target_str = if action == Action::Delete {
            if !delete_confirmation_valid(&self.input) {
                self.status = String::from("Delete cancelled: type DELETE to confirm");
                return Ok(());
            }
            String::new()
        } else {
            let target = expand_tilde(self.input.trim());
            if target.as_os_str().is_empty() {
                self.status = String::from("Target path is empty");
                return Ok(());
            }
            target.to_string_lossy().to_string()
        };
        let mut ok = 0usize;
        let mut skipped = 0usize;
        let mut failures = Vec::new();

        for session in &targets {
            let result = match action {
                Action::Move | Action::ProjectRename => {
                    if session.cwd == target_str {
                        skipped += 1;
                        Ok(())
                    } else {
                        rewrite_session_file(&session.path, &target_str, false)
                    }
                }
                Action::Copy | Action::ProjectCopy => {
                    duplicate_session_file(&self.sessions_root, session, &target_str, false)
                        .map(|_| ())
                }
                Action::Fork => {
                    duplicate_session_file(&self.sessions_root, session, &target_str, true)
                        .map(|_| ())
                }
                Action::Delete => delete_session_file(&session.path),
            };
            match result {
                Ok(()) => ok += 1,
                Err(err) => failures.push(format!("{}: {}", session.file_name, err)),
            }
        }

        self.mode = Mode::Normal;
        self.pending_action = None;
        self.input.clear();
        self.input_focused = false;
        self.clear_input_completion_cycle();

        if ok > 0 || skipped > 0 {
            self.reload()?;
        }
        self.selected_sessions.clear();
        self.session_select_anchor = None;

        let action_name = match action {
            Action::Move => "moved",
            Action::Copy => "copied",
            Action::Fork => "forked",
            Action::Delete => "deleted",
            Action::ProjectRename => "renamed",
            Action::ProjectCopy => "copied",
        };
        self.status = if failures.is_empty() {
            if action == Action::Delete {
                format!("{action_name} {ok} session(s)")
            } else if skipped > 0 {
                format!(
                    "{action_name} {ok} session(s), skipped {skipped} unchanged -> {target_str}"
                )
            } else {
                format!("{action_name} {ok} session(s) -> {target_str}")
            }
        } else {
            let first = failures
                .first()
                .cloned()
                .unwrap_or_else(|| String::from("unknown error"));
            format!(
                "{action_name} {ok} session(s), {} failed, skipped {skipped}. First error: {first}",
                failures.len()
            )
        };
        Ok(())
    }

    fn clear_input_completion_cycle(&mut self) {
        self.input_tab_last_at = None;
        self.input_tab_last_query.clear();
    }

    fn tab_complete_input_path(&mut self) {
        let query = self.input.clone();
        let now = Instant::now();
        let repeated = self
            .input_tab_last_at
            .is_some_and(|at| now.duration_since(at) <= Duration::from_millis(800))
            && self.input_tab_last_query == query;
        self.input_tab_last_at = Some(now);
        self.input_tab_last_query = query.clone();

        let (dir_part, prefix) = if query.ends_with('/') {
            (query.as_str(), "")
        } else if let Some(pos) = query.rfind('/') {
            (&query[..=pos], &query[pos + 1..])
        } else {
            ("", query.as_str())
        };

        let dir_path = if dir_part.is_empty() {
            PathBuf::from(".")
        } else {
            expand_tilde(dir_part)
        };

        let mut matches = Vec::new();
        let read_dir = fs::read_dir(&dir_path);
        let Ok(entries) = read_dir else {
            self.status = format!("Cannot read directory: {}", dir_path.display());
            return;
        };

        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(prefix) {
                matches.push(name);
            }
        }
        matches.sort();

        if matches.is_empty() {
            self.status = format!("No directory matches for '{}'", query);
            return;
        }

        if matches.len() == 1 {
            self.input = format!("{dir_part}{}/", matches[0]);
            self.status = format!("Completed: {}", self.input);
            return;
        }

        let lcp = longest_common_prefix(&matches);
        if lcp.chars().count() > prefix.chars().count() {
            self.input = format!("{dir_part}{lcp}");
            self.status = format!("{} matches", matches.len());
            return;
        }

        if repeated {
            let shown = matches
                .iter()
                .take(12)
                .cloned()
                .collect::<Vec<_>>()
                .join("  ");
            if matches.len() > 12 {
                self.status = format!("Matches: {shown}  ... (+{} more)", matches.len() - 12);
            } else {
                self.status = format!("Matches: {shown}");
            }
        } else {
            self.status = format!("{} matches (Tab again to list)", matches.len());
        }
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
                .title("Projects (cwd) [m/r rename] [c/y copy]")
                .borders(Borders::ALL)
                .border_style(focus_style)
                .style(Style::default().add_modifier(Modifier::DIM)),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
        )
        .highlight_symbol(" > ");

    frame.render_stateful_widget(list, area, &mut state);
    render_thin_scrollbar(
        frame,
        area,
        app.project_scroll,
        app.projects.len(),
        App::visible_rows(area.height, 1),
    );
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
            let selected = app.selected_sessions.contains(&session.path);
            let (line1, line2) = format_session_item_lines(session);
            let mark = if selected { "[x]" } else { "[ ]" };
            let line1_style = if selected {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let line2_style = if selected {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            ListItem::new(vec![
                Line::from(Span::styled(format!("{mark} {line1}"), line1_style)),
                Line::from(Span::styled(format!("    {line2}"), line2_style)),
            ])
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
                .title(format!(
                    "Sessions [{} selected] (Space/checkbox toggle, a all, i invert)",
                    app.selected_count_current_project()
                ))
                .borders(Borders::ALL)
                .border_style(focus_style)
                .style(Style::default()),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
        )
        .highlight_symbol(" > ");

    frame.render_stateful_widget(list, area, &mut state);
    render_thin_scrollbar(
        frame,
        area,
        app.session_scroll,
        sessions.len(),
        App::visible_rows(area.height, 2),
    );
}

fn format_session_item_lines(session: &SessionSummary) -> (String, String) {
    let line1 = format!("{} | {} events", session.started_at, session.event_count);
    let short_id: String = session.id.chars().take(8).collect();
    let line2 = format!("{} | {}", short_id, session.file_name);
    (line1, line2)
}

fn render_preview(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &mut App) {
    let preview = if let Some(session) = app.current_session().cloned() {
        let inner_width = area.width.saturating_sub(2) as usize;
        match app.preview_for_session(&session, app.preview_mode, inner_width) {
            Ok(preview) => preview,
            Err(err) => PreviewData {
                lines: vec![Line::from(format!("Preview error: {err:#}"))],
                tone_rows: Vec::new(),
                header_rows: Vec::new(),
                block_ranges: Vec::new(),
            },
        }
    } else {
        PreviewData {
            lines: vec![Line::from("No session selected")],
            tone_rows: Vec::new(),
            header_rows: Vec::new(),
            block_ranges: Vec::new(),
        }
    };
    app.preview_content_len = preview.lines.len();
    app.preview_rendered_lines = preview.lines.iter().map(|l| l.to_string()).collect();
    let viewport_len = area.height.saturating_sub(2) as usize;
    let max_scroll = app.preview_content_len.saturating_sub(viewport_len);
    app.preview_scroll = app.preview_scroll.min(max_scroll);
    app.preview_header_rows = preview.header_rows.clone();
    app.preview_session_path = app.current_session().map(|s| s.path.clone());
    app.ensure_preview_focus_valid();

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
        .scroll((app.preview_scroll as u16, 0));
    frame.render_widget(para, area);

    let inner_x = area.x.saturating_add(1);
    let inner_y = area.y.saturating_add(1);
    let inner_w = area.width.saturating_sub(2);
    let inner_h = area.height.saturating_sub(2) as usize;
    let scroll = app.preview_scroll;
    for (row, tone) in preview.tone_rows {
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
            block_tone_style(tone),
        );
    }
    if app.focus == Focus::Preview
        && let Some(focused_turn) = app.preview_focus_turn
        && let Some((_, start, end)) = preview
            .block_ranges
            .iter()
            .find(|(turn_idx, _, _)| *turn_idx == focused_turn)
            .copied()
    {
        let header_row = app
            .preview_header_rows
            .iter()
            .find(|(_, turn_idx)| *turn_idx == focused_turn)
            .map(|(row, _)| *row);
        let vis_start = start.max(scroll);
        let vis_end = end.min(scroll + inner_h.saturating_sub(1));
        if vis_start <= vis_end && inner_w >= 2 {
            let left_x = inner_x;
            let right_x = inner_x + inner_w.saturating_sub(1);
            let edge = Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM);
            for row in vis_start..=vis_end {
                let y = inner_y + (row - scroll) as u16;
                // Preserve the expand/collapse marker at column 0 on header row.
                if Some(row) != header_row {
                    frame.buffer_mut().set_string(left_x, y, "│", edge);
                }
                frame.buffer_mut().set_string(right_x, y, "│", edge);
            }
            let top_y = inner_y + (vis_start - scroll) as u16;
            let bottom_y = inner_y + (vis_end - scroll) as u16;
            frame.buffer_mut().set_string(left_x, top_y, "┌", edge);
            frame.buffer_mut().set_string(right_x, top_y, "┐", edge);
            frame.buffer_mut().set_string(left_x, bottom_y, "└", edge);
            frame.buffer_mut().set_string(right_x, bottom_y, "┘", edge);
        }
    }
    if let Some((a, b)) = app.preview_selection {
        let (beg, fin) = if a <= b { (a, b) } else { (b, a) };
        for row in beg.0..=fin.0 {
            if row < scroll || row >= scroll + inner_h {
                continue;
            }
            let line_len = app
                .preview_rendered_lines
                .get(row)
                .map(|l| l.chars().count())
                .unwrap_or(0);
            if line_len == 0 {
                continue;
            }
            let (col_start, col_end_inclusive) = if beg.0 == fin.0 {
                (
                    beg.1.min(line_len.saturating_sub(1)),
                    fin.1.min(line_len.saturating_sub(1)),
                )
            } else if row == beg.0 {
                (
                    beg.1.min(line_len.saturating_sub(1)),
                    line_len.saturating_sub(1),
                )
            } else if row == fin.0 {
                (0, fin.1.min(line_len.saturating_sub(1)))
            } else {
                (0, line_len.saturating_sub(1))
            };
            if col_start > col_end_inclusive {
                continue;
            }
            let x = inner_x.saturating_add(col_start as u16);
            let w = (col_end_inclusive - col_start + 1) as u16;
            let max_w = inner_w.saturating_sub(col_start as u16);
            let width = w.min(max_w);
            if width == 0 {
                continue;
            }
            let screen_y = inner_y + (row - scroll) as u16;
            frame.buffer_mut().set_style(
                ratatui::layout::Rect {
                    x,
                    y: screen_y,
                    width,
                    height: 1,
                },
                Style::default().add_modifier(Modifier::REVERSED),
            );
        }
    }

    render_thin_scrollbar(
        frame,
        area,
        app.preview_scroll,
        app.preview_content_len,
        viewport_len,
    );
}

fn render_thin_scrollbar(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    offset: usize,
    content_len: usize,
    viewport_len: usize,
) {
    if viewport_len == 0 || content_len <= viewport_len {
        return;
    }

    let mut state = ScrollbarState::new(content_len).position(offset.min(content_len - 1));
    let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .thumb_symbol("▐")
        .track_symbol(Some("│"))
        .begin_symbol(None)
        .end_symbol(None)
        .style(Style::default().fg(Color::DarkGray));
    frame.render_stateful_widget(bar, area, &mut state);
}

fn block_tone_style(tone: BlockTone) -> Style {
    if tone == BlockTone::Assistant {
        return Style::default();
    }
    // Similar to edit's approach: blend a cool accent into terminal background.
    // This avoids warm/darker shifts while still creating a distinct user block.
    let (bg, _) = terminal_bg_rgb().unwrap_or(((0, 0, 0), true));
    let accent = ansi_index_to_rgb(12); // BrightBlue from ANSI palette.
    let accented = blend_rgb(bg, accent, 0.16);
    let softened = blend_rgb(bg, accented, 0.55);
    Style::default().bg(Color::Rgb(softened.0, softened.1, softened.2))
}

fn infer_dark_theme_from_env() -> Option<bool> {
    let raw = env::var("COLORFGBG").ok()?;
    let idx = parse_colorfgbg_bg_index(&raw)?;
    let rgb = ansi_index_to_rgb(idx);
    let luma = 0.2126 * rgb.0 as f32 + 0.7152 * rgb.1 as f32 + 0.0722 * rgb.2 as f32;
    Some(luma < 140.0)
}

fn parse_colorfgbg_bg_index(raw: &str) -> Option<u8> {
    raw.split(';').next_back()?.trim().parse::<u8>().ok()
}

fn terminal_bg_rgb() -> Option<((u8, u8, u8), bool)> {
    let raw = env::var("COLORFGBG").ok()?;
    let idx = parse_colorfgbg_bg_index(&raw)?;
    let rgb = ansi_index_to_rgb(idx);
    let dark = infer_dark_theme_from_env().unwrap_or(true);
    Some((rgb, dark))
}

fn blend_rgb(base: (u8, u8, u8), overlay: (u8, u8, u8), alpha: f32) -> (u8, u8, u8) {
    let a = alpha.clamp(0.0, 1.0);
    let mix = |b: u8, o: u8| -> u8 {
        ((b as f32) * (1.0 - a) + (o as f32) * a)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    (
        mix(base.0, overlay.0),
        mix(base.1, overlay.1),
        mix(base.2, overlay.2),
    )
}

fn ansi_index_to_rgb(idx: u8) -> (u8, u8, u8) {
    const BASIC: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 0, 0),
        (0, 205, 0),
        (205, 205, 0),
        (0, 0, 238),
        (205, 0, 205),
        (0, 205, 205),
        (229, 229, 229),
        (127, 127, 127),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (92, 92, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    if idx < 16 {
        return BASIC[idx as usize];
    }
    if (16..=231).contains(&idx) {
        let i = idx - 16;
        let r = i / 36;
        let g = (i % 36) / 6;
        let b = i % 6;
        let step = [0, 95, 135, 175, 215, 255];
        return (step[r as usize], step[g as usize], step[b as usize]);
    }
    let gray = 8 + (idx.saturating_sub(232)) * 10;
    (gray, gray, gray)
}

fn status_button_style(button: StatusButton) -> Style {
    match button {
        StatusButton::Apply
        | StatusButton::Move
        | StatusButton::Copy
        | StatusButton::Fork
        | StatusButton::ProjectRename
        | StatusButton::ProjectCopy => Style::default().fg(Color::Green),
        StatusButton::Delete => Style::default().fg(Color::Red),
        StatusButton::SelectAll | StatusButton::Invert => Style::default().fg(Color::Yellow),
        StatusButton::Cancel | StatusButton::Quit => Style::default().fg(Color::Red),
        StatusButton::Refresh => Style::default().fg(Color::Yellow),
    }
}

fn tab_match_status_style() -> Style {
    if infer_dark_theme_from_env().unwrap_or(true) {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Rgb(220, 228, 242))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::White)
            .bg(Color::Rgb(34, 46, 64))
            .add_modifier(Modifier::BOLD)
    }
}

fn render_status(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &App) {
    let key_line = if app.mode == Mode::Input {
        Line::from(vec![
            Span::styled("tab", Style::default().fg(Color::Cyan)),
            Span::raw(" path-complete  "),
            Span::styled("tab tab", Style::default().fg(Color::Cyan)),
            Span::raw(" list dirs  "),
            Span::styled("enter", Style::default().fg(Color::Green)),
            Span::raw(" apply  "),
            Span::styled("esc", Style::default().fg(Color::Red)),
            Span::raw(" cancel"),
        ])
    } else if app.focus == Focus::Preview && app.mode == Mode::Normal {
        Line::from(vec![
            Span::styled("↑/↓", Style::default().fg(Color::Cyan)),
            Span::raw(" block prev/next  "),
            Span::styled("←/→", Style::default().fg(Color::Cyan)),
            Span::raw(" fold/unfold block  "),
            Span::styled("tab", Style::default().fg(Color::Cyan)),
            Span::raw(" toggle block  "),
            Span::styled("shift+tab", Style::default().fg(Color::Cyan)),
            Span::raw(" toggle all blocks  "),
            Span::styled("drag", Style::default().fg(Color::Cyan)),
            Span::raw(" preview-select+copy  "),
            Span::styled("drag", Style::default().fg(Color::Cyan)),
            Span::raw(" splitter/scrollbar"),
        ])
    } else if app.focus == Focus::Projects && app.mode == Mode::Normal {
        Line::from(vec![
            Span::styled("j/k", Style::default().fg(Color::Cyan)),
            Span::raw(" project nav  "),
            Span::styled("m or r", Style::default().fg(Color::Green)),
            Span::raw(" rename folder sessions  "),
            Span::styled("c or y", Style::default().fg(Color::Green)),
            Span::raw(" copy folder sessions  "),
            Span::styled("/", Style::default().fg(Color::Cyan)),
            Span::raw(" search  "),
            Span::styled("q", Style::default().fg(Color::Red)),
            Span::raw(" quit"),
        ])
    } else if app.focus == Focus::Sessions && app.mode == Mode::Normal {
        Line::from(vec![
            Span::styled("j/k", Style::default().fg(Color::Cyan)),
            Span::raw(" nav  "),
            Span::styled("space", Style::default().fg(Color::Yellow)),
            Span::raw(" toggle-select  "),
            Span::styled("checkbox click", Style::default().fg(Color::Yellow)),
            Span::raw(" toggle  "),
            Span::styled("a", Style::default().fg(Color::Yellow)),
            Span::raw(" select-all  "),
            Span::styled("i", Style::default().fg(Color::Yellow)),
            Span::raw(" invert  "),
            Span::styled("m/c/f/d", Style::default().fg(Color::Green)),
            Span::raw(" move/copy/fork/delete selection  "),
            Span::styled("/", Style::default().fg(Color::Cyan)),
            Span::raw(" search"),
        ])
    } else {
        Line::from(vec![
            Span::styled("tab", Style::default().fg(Color::Cyan)),
            Span::raw(" focus  "),
            Span::styled("j/k", Style::default().fg(Color::Cyan)),
            Span::raw(" nav  "),
            Span::styled("/", Style::default().fg(Color::Cyan)),
            Span::raw(" search  "),
            Span::styled("v", Style::default().fg(Color::Cyan)),
            Span::raw(" preview-mode  "),
            Span::styled("z", Style::default().fg(Color::Cyan)),
            Span::raw(" fold  "),
            Span::styled("h/l", Style::default().fg(Color::Cyan)),
            Span::raw(" resize-pane  "),
            Span::styled("drag", Style::default().fg(Color::Cyan)),
            Span::raw(" splitter  preview-select "),
            Span::styled("m/c/f/d", Style::default().fg(Color::Green)),
            Span::raw(" move/copy/fork/delete  "),
            Span::styled("g", Style::default().fg(Color::Yellow)),
            Span::raw(" refresh  "),
            Span::styled("q", Style::default().fg(Color::Red)),
            Span::raw(" quit"),
        ])
    };
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
        "pane widths p/s/r: {}/{}/{}  preview: {}  mouse: {}",
        app.project_width_pct,
        app.session_width_pct,
        app.preview_width_pct(),
        preview_mode,
        if app.preview_selecting {
            "select"
        } else {
            "ui"
        }
    );
    let meta_line = Line::from(vec![
        Span::styled(search_meta, Style::default().fg(Color::DarkGray)),
        Span::raw("  "),
        Span::styled(pane_meta, Style::default().fg(Color::DarkGray)),
    ]);

    let mut controls_spans = Vec::new();
    let buttons = status_buttons(app);
    for (idx, button) in buttons.iter().enumerate() {
        controls_spans.push(Span::styled(
            status_button_label(*button),
            status_button_style(*button),
        ));
        if idx + 1 < buttons.len() {
            controls_spans.push(Span::raw(" "));
        }
    }
    controls_spans.push(Span::raw(if app.mode == Mode::Input {
        "  (click buttons or press Enter/Esc)"
    } else {
        "  wheel scrolls panes"
    }));
    let mut lines = vec![Line::from(controls_spans)];

    lines.insert(0, meta_line);
    lines.insert(0, key_line);

    if app.mode == Mode::Input {
        let action = match app.pending_action {
            Some(Action::Move) => "MOVE",
            Some(Action::Copy) => "COPY",
            Some(Action::Fork) => "FORK",
            Some(Action::Delete) => "DELETE",
            Some(Action::ProjectRename) => "RENAME FOLDER",
            Some(Action::ProjectCopy) => "COPY FOLDER",
            None => "ACTION",
        };

        let focus_mark = if app.input_focused { "*" } else { " " };
        let blink_on = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| (d.as_millis() / 500) % 2 == 0)
            .unwrap_or(true);
        let cursor = if app.input_focused && blink_on {
            "█"
        } else {
            " "
        };
        lines.push(Line::from(format!(
            "{focus_mark} {action} target> {}{cursor}",
            app.input,
        )));
        if !app.status.trim().is_empty() {
            let status_style = if app.status.starts_with("Matches:") {
                tab_match_status_style()
            } else {
                Style::default().fg(Color::DarkGray)
            };
            lines.push(Line::from(Span::styled(app.status.clone(), status_style)));
        }
    } else {
        lines.push(Line::from(app.status.clone()));
    }

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Status"))
        .wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

#[cfg(test)]
fn build_preview(
    session: &SessionSummary,
    mode: PreviewMode,
    inner_width: usize,
) -> Result<PreviewData> {
    let content = fs::read_to_string(&session.path)
        .with_context(|| format!("failed to read {}", session.path.display()))?;
    let turns = extract_chat_turns(&content);
    let events = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<Value>(line)
                .map(|v| summarize_event_line(&v))
                .unwrap_or_else(|_| String::from("<invalid event>"))
        })
        .collect::<Vec<_>>();
    let cached = CachedPreviewSource {
        mtime: SystemTime::UNIX_EPOCH,
        turns,
        events,
    };
    Ok(build_preview_from_cached(
        session,
        mode,
        inner_width,
        &cached,
        &HashSet::new(),
    ))
}

fn build_preview_from_cached(
    session: &SessionSummary,
    mode: PreviewMode,
    inner_width: usize,
    cached: &CachedPreviewSource,
    folded: &HashSet<usize>,
) -> PreviewData {
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
    let mut tone_rows = Vec::new();
    let mut header_rows = Vec::new();
    let mut block_ranges = Vec::new();

    if mode == PreviewMode::Events {
        lines.push(Line::from(Span::styled(
            "Event Stream",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
        append_event_preview_from_lines(&mut lines, &cached.events);
        return PreviewData {
            lines,
            tone_rows,
            header_rows,
            block_ranges,
        };
    }

    lines.push(Line::from(Span::styled(
        "Conversation",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )));

    if cached.turns.is_empty() {
        lines.push(Line::from(
            "No user/assistant chat messages found in this session.",
        ));
        return PreviewData {
            lines,
            tone_rows,
            header_rows,
            block_ranges,
        };
    }

    let user_count = cached.turns.iter().filter(|t| t.role == "user").count();
    let assistant_count = cached
        .turns
        .iter()
        .filter(|t| t.role == "assistant")
        .count();
    lines.push(Line::from(format!(
        "Turns: user={} assistant={} total={}",
        user_count,
        assistant_count,
        cached.turns.len()
    )));
    if assistant_count == 0 {
        lines.push(Line::from(Span::styled(
            "Warning: no assistant messages detected in this session.",
            Style::default().fg(Color::Yellow),
        )));
    }
    lines.push(Line::from(String::new()));

    for (turn_idx, turn) in cached.turns.iter().enumerate() {
        let tone = if turn.role == "user" {
            BlockTone::User
        } else {
            BlockTone::Assistant
        };
        let role_style = match turn.role.as_str() {
            "user" => Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            "assistant" => Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
            _ => Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        };
        let is_folded = folded.contains(&turn_idx);
        let marker = if is_folded { "▶" } else { "▼" };
        let block_start = lines.len();
        lines.push(Line::from(String::new()));
        tone_rows.push((lines.len().saturating_sub(1), tone));
        lines.push(Line::from(vec![
            Span::styled(format!("{marker} "), Style::default().fg(Color::DarkGray)),
            Span::styled(format!(" {} ", turn.role.to_uppercase()), role_style),
            Span::raw(" "),
            Span::styled(turn.timestamp.clone(), Style::default().fg(Color::DarkGray)),
        ]));
        header_rows.push((lines.len().saturating_sub(1), turn_idx));
        tone_rows.push((lines.len().saturating_sub(1), tone));

        if !is_folded {
            for wrapped in render_markdown_lines(&turn.text, inner_width.saturating_sub(3)) {
                lines.push(Line::from(format!("   {wrapped}")));
                tone_rows.push((lines.len().saturating_sub(1), tone));
            }
        }
        lines.push(Line::from(String::new()));
        tone_rows.push((lines.len().saturating_sub(1), tone));
        let block_end = lines.len().saturating_sub(1);
        block_ranges.push((turn_idx, block_start, block_end));
        if turn_idx + 1 < cached.turns.len() {
            if tone == BlockTone::User {
                // Ensure a terminal-bg hairline gap between USER blocks.
                lines.push(Line::from(String::new()));
            } else {
                let width = inner_width.saturating_sub(1).max(1);
                lines.push(Line::from(Span::styled(
                    "─".repeat(width),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
    }

    PreviewData {
        lines,
        tone_rows,
        header_rows,
        block_ranges,
    }
}

fn append_event_preview_from_lines(lines: &mut Vec<Line<'static>>, all: &[String]) {
    let start = all.len().saturating_sub(220);
    if start > 0 {
        lines.push(Line::from(format!(
            "... showing last {} of {} events ...",
            all.len() - start,
            all.len()
        )));
        lines.push(Line::from(String::new()));
    }
    for entry in all.iter().skip(start) {
        lines.push(Line::from(entry.clone()));
    }
}

fn wrap_text_lines(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut out = Vec::new();
    for raw in text.lines() {
        let mut current = String::new();
        for word in raw.split_whitespace() {
            if current.is_empty() {
                if word.chars().count() <= width {
                    current.push_str(word);
                } else {
                    for chunk in chunk_by_width(word, width) {
                        out.push(chunk);
                    }
                }
                continue;
            }
            let next_len = current.chars().count() + 1 + word.chars().count();
            if next_len <= width {
                current.push(' ');
                current.push_str(word);
            } else {
                out.push(current);
                current = String::new();
                if word.chars().count() <= width {
                    current.push_str(word);
                } else {
                    for chunk in chunk_by_width(word, width) {
                        out.push(chunk);
                    }
                }
            }
        }
        if !current.is_empty() {
            out.push(current);
        } else if raw.trim().is_empty() {
            out.push(String::new());
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn render_markdown_lines(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut options = MdOptions::empty();
    options.insert(MdOptions::ENABLE_STRIKETHROUGH);
    options.insert(MdOptions::ENABLE_TABLES);
    options.insert(MdOptions::ENABLE_TASKLISTS);

    #[derive(Clone, Copy)]
    enum ListKind {
        Bullet,
        Ordered(u64),
    }

    let mut raw_lines = Vec::new();
    let mut line = String::new();
    let mut quote_depth = 0usize;
    let mut list_stack: Vec<ListKind> = Vec::new();
    let mut in_code_block = false;

    let flush_line = |line: &mut String, raw_lines: &mut Vec<String>| {
        if !line.is_empty() {
            raw_lines.push(std::mem::take(line));
        }
    };

    for event in MdParser::new_ext(text, options) {
        if in_code_block {
            match event {
                MdEvent::End(TagEnd::CodeBlock) => {
                    in_code_block = false;
                    raw_lines.push(String::new());
                }
                MdEvent::Text(t) | MdEvent::Code(t) => {
                    for code_line in t.lines() {
                        raw_lines.push(format!("    {code_line}"));
                    }
                }
                MdEvent::SoftBreak | MdEvent::HardBreak => raw_lines.push(String::new()),
                _ => {}
            }
            continue;
        }

        match event {
            MdEvent::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::Heading { .. } => {
                    flush_line(&mut line, &mut raw_lines);
                }
                Tag::BlockQuote(_) => {
                    flush_line(&mut line, &mut raw_lines);
                    quote_depth = quote_depth.saturating_add(1);
                }
                Tag::List(start) => {
                    flush_line(&mut line, &mut raw_lines);
                    match start {
                        Some(n) => list_stack.push(ListKind::Ordered(n)),
                        None => list_stack.push(ListKind::Bullet),
                    }
                }
                Tag::Item => {
                    flush_line(&mut line, &mut raw_lines);
                    for _ in 0..quote_depth {
                        line.push_str("> ");
                    }
                    if let Some(kind) = list_stack.last_mut() {
                        match kind {
                            ListKind::Bullet => line.push_str("- "),
                            ListKind::Ordered(n) => {
                                line.push_str(&format!("{n}. "));
                                *n += 1;
                            }
                        }
                    }
                }
                Tag::CodeBlock(_) => {
                    flush_line(&mut line, &mut raw_lines);
                    in_code_block = true;
                }
                _ => {}
            },
            MdEvent::End(tag_end) => match tag_end {
                TagEnd::Paragraph | TagEnd::Heading(_) => {
                    flush_line(&mut line, &mut raw_lines);
                    raw_lines.push(String::new());
                }
                TagEnd::BlockQuote(_) => {
                    flush_line(&mut line, &mut raw_lines);
                    quote_depth = quote_depth.saturating_sub(1);
                    raw_lines.push(String::new());
                }
                TagEnd::List(_) => {
                    flush_line(&mut line, &mut raw_lines);
                    let _ = list_stack.pop();
                    raw_lines.push(String::new());
                }
                TagEnd::Item => {
                    flush_line(&mut line, &mut raw_lines);
                }
                _ => {}
            },
            MdEvent::Text(t) | MdEvent::Code(t) => line.push_str(&t),
            MdEvent::SoftBreak => line.push(' '),
            MdEvent::HardBreak => flush_line(&mut line, &mut raw_lines),
            MdEvent::Rule => {
                flush_line(&mut line, &mut raw_lines);
                raw_lines.push("─".repeat(width.min(48)));
            }
            MdEvent::Html(_) | MdEvent::InlineHtml(_) => {}
            MdEvent::InlineMath(t) | MdEvent::DisplayMath(t) => line.push_str(&t),
            _ => {}
        }
    }
    flush_line(&mut line, &mut raw_lines);

    while raw_lines.last().is_some_and(|l| l.is_empty()) {
        raw_lines.pop();
    }

    let mut out = Vec::new();
    for raw in raw_lines {
        if raw.is_empty() {
            out.push(String::new());
            continue;
        }
        if let Some(code) = raw.strip_prefix("    ") {
            let chunks = chunk_by_width(code, width.saturating_sub(4).max(1));
            if chunks.is_empty() {
                out.push(String::from("    "));
            } else {
                for chunk in chunks {
                    out.push(format!("    {chunk}"));
                }
            }
            continue;
        }
        let (prefix, body) = split_markdown_prefix(&raw);
        if body.trim().is_empty() {
            out.push(prefix);
            continue;
        }
        let wrapped = wrap_text_lines(body.trim(), width.saturating_sub(prefix.chars().count()));
        for (idx, l) in wrapped.iter().enumerate() {
            if idx == 0 {
                out.push(format!("{prefix}{l}"));
            } else {
                out.push(format!("{}{}", " ".repeat(prefix.chars().count()), l));
            }
        }
    }
    if out.is_empty() {
        vec![String::new()]
    } else {
        out
    }
}

fn split_markdown_prefix(raw: &str) -> (String, &str) {
    let trimmed = raw.trim_start();
    let indent_len = raw.len().saturating_sub(trimmed.len());
    let indent = " ".repeat(indent_len);

    if let Some(rest) = trimmed.strip_prefix("> ") {
        return (format!("{indent}> "), rest);
    }
    if let Some(rest) = trimmed.strip_prefix("- ") {
        return (format!("{indent}- "), rest);
    }
    if let Some(rest) = trimmed.strip_prefix("* ") {
        return (format!("{indent}* "), rest);
    }
    if let Some(rest) = trimmed.strip_prefix("+ ") {
        return (format!("{indent}+ "), rest);
    }
    if let Some((num, rest)) = split_ordered_list(trimmed) {
        return (format!("{indent}{num}. "), rest);
    }
    if trimmed.starts_with('#') {
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        let marker = &trimmed[..hashes];
        let rest = trimmed[hashes..].trim_start();
        return (format!("{indent}{marker} "), rest);
    }
    (indent, trimmed)
}

fn split_ordered_list(s: &str) -> Option<(&str, &str)> {
    let dot = s.find('.')?;
    if dot == 0 || !s[..dot].chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let rest = s[dot + 1..].strip_prefix(' ')?;
    Some((&s[..dot], rest))
}

fn chunk_by_width(input: &str, width: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut buf = String::new();
    for ch in input.chars() {
        buf.push(ch);
        if buf.chars().count() >= width {
            chunks.push(buf);
            buf = String::new();
        }
    }
    if !buf.is_empty() {
        chunks.push(buf);
    }
    chunks
}

fn slice_chars(s: &str, start: usize, end_exclusive: usize) -> String {
    let start = start.min(s.chars().count());
    let end = end_exclusive.min(s.chars().count()).max(start);
    s.chars().skip(start).take(end - start).collect()
}

fn longest_common_prefix(items: &[String]) -> String {
    let Some(first) = items.first() else {
        return String::new();
    };
    let mut prefix = first.clone();
    for item in items.iter().skip(1) {
        let mut next = String::new();
        for (a, b) in prefix.chars().zip(item.chars()) {
            if a != b {
                break;
            }
            next.push(a);
        }
        prefix = next;
        if prefix.is_empty() {
            break;
        }
    }
    prefix
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
    tone_rows: Vec<(usize, BlockTone)>,
    header_rows: Vec<(usize, usize)>,
    block_ranges: Vec<(usize, usize, usize)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockTone {
    User,
    Assistant,
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

fn delete_confirmation_valid(input: &str) -> bool {
    input == "DELETE"
}

fn delete_session_file(path: &Path) -> Result<()> {
    backup_file(path)?;
    fs::remove_file(path).with_context(|| format!("failed deleting {}", path.display()))?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_chat_jsonl() -> String {
        [
            r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":"abc","timestamp":"2026-01-01T00:00:00Z","cwd":"/tmp/x"}}"#,
            r#"{"timestamp":"2026-01-01T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}}"#,
            r#"{"timestamp":"2026-01-01T00:00:02Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"world"}]}}"#,
            r#"{"timestamp":"2026-01-01T00:00:03Z","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"normalized user"}]}}"#,
        ]
        .join("\n")
    }

    fn empty_test_app() -> App {
        App {
            sessions_root: PathBuf::from("/tmp"),
            all_projects: Vec::new(),
            projects: Vec::new(),
            project_idx: 0,
            session_idx: 0,
            selected_sessions: HashSet::new(),
            session_select_anchor: None,
            focus: Focus::Projects,
            mode: Mode::Normal,
            pending_action: None,
            input: String::new(),
            input_focused: false,
            input_tab_last_at: None,
            input_tab_last_query: String::new(),
            search_query: String::new(),
            search_focused: false,
            search_dirty: false,
            preview_mode: PreviewMode::Chat,
            preview_selecting: false,
            preview_mouse_down_pos: None,
            drag_target: None,
            scroll_drag: None,
            status: String::new(),
            panes: PaneLayout::default(),
            project_width_pct: 20,
            session_width_pct: 36,
            project_scroll: 0,
            session_scroll: 0,
            preview_scroll: 0,
            preview_content_len: 0,
            preview_selection: None,
            preview_rendered_lines: Vec::new(),
            preview_focus_turn: None,
            preview_cache: HashMap::new(),
            preview_folded: HashMap::new(),
            preview_header_rows: Vec::new(),
            preview_session_path: None,
        }
    }

    fn sample_session(path: &str, cwd: &str, id: &str) -> SessionSummary {
        SessionSummary {
            path: PathBuf::from(path),
            file_name: format!("{id}.jsonl"),
            id: String::from(id),
            cwd: String::from(cwd),
            started_at: String::from("2026-01-01T00:00:00Z"),
            event_count: 1,
            search_blob: String::new(),
        }
    }

    #[test]
    fn extract_chat_turns_normalizes_developer_role() {
        let turns = extract_chat_turns(&sample_chat_jsonl());
        assert_eq!(turns.len(), 3);
        assert_eq!(turns[0].role, "user");
        assert_eq!(turns[1].role, "assistant");
        assert_eq!(turns[2].role, "user");
    }

    #[test]
    fn fuzzy_score_prefers_compact_match() {
        let a = fuzzy_score("abc", "a_b_c").unwrap_or(i64::MIN);
        let b = fuzzy_score("abc", "alphabet-bucket-code").unwrap_or(i64::MIN);
        assert!(a > b);
    }

    #[test]
    fn wrap_text_lines_reflows_for_smaller_width() {
        let text = "this is a long sentence for wrapping";
        let wide = wrap_text_lines(text, 40);
        let narrow = wrap_text_lines(text, 10);
        assert_eq!(wide.len(), 1);
        assert!(narrow.len() > 1);
        assert!(narrow.iter().all(|line| line.chars().count() <= 10));
    }

    #[test]
    fn parse_colorfgbg_bg_index_works() {
        assert_eq!(parse_colorfgbg_bg_index("15;0"), Some(0));
        assert_eq!(parse_colorfgbg_bg_index("0;15"), Some(15));
        assert_eq!(parse_colorfgbg_bg_index("bad"), None);
    }

    #[test]
    fn blend_rgb_halfway_mixes_channels() {
        let out = blend_rgb((0, 0, 0), (200, 100, 50), 0.5);
        assert_eq!(out, (100, 50, 25));
    }

    #[test]
    fn longest_common_prefix_finds_shared_prefix() {
        let items = vec![
            String::from("alpha"),
            String::from("alpine"),
            String::from("alps"),
        ];
        assert_eq!(longest_common_prefix(&items), "alp");
    }

    #[test]
    fn tab_complete_path_single_match_appends_slash() {
        let base = std::env::temp_dir().join(format!("cse-tab-{}", Uuid::new_v4()));
        fs::create_dir_all(base.join("alpha")).expect("mkdir alpha");
        fs::create_dir_all(base.join("beta")).expect("mkdir beta");

        let base_s = base.to_string_lossy().replace('\\', "/");
        let mut app = empty_test_app();
        app.mode = Mode::Input;
        app.input_focused = true;
        app.input = format!("{base_s}/al");

        app.tab_complete_input_path();
        assert_eq!(app.input, format!("{base_s}/alpha/"));
    }

    #[test]
    fn tab_complete_path_double_tab_lists_matches() {
        let base = std::env::temp_dir().join(format!("cse-tab-list-{}", Uuid::new_v4()));
        fs::create_dir_all(base.join("alpha")).expect("mkdir alpha");
        fs::create_dir_all(base.join("alto")).expect("mkdir alto");
        fs::create_dir_all(base.join("alps")).expect("mkdir alps");

        let base_s = base.to_string_lossy().replace('\\', "/");
        let mut app = empty_test_app();
        app.mode = Mode::Input;
        app.input_focused = true;
        app.input = format!("{base_s}/al");

        app.tab_complete_input_path();
        assert!(app.status.contains("Tab again to list"));
        app.tab_complete_input_path();
        assert!(app.status.starts_with("Matches: "));
        assert!(app.status.contains("alpha"));
        assert!(app.status.contains("alto"));
        assert!(app.status.contains("alps"));
    }

    #[test]
    fn toggle_current_session_selection_tracks_current_project() {
        let mut app = empty_test_app();
        app.projects = vec![ProjectBucket {
            cwd: String::from("/repo"),
            sessions: vec![
                sample_session("/tmp/a.jsonl", "/repo", "a"),
                sample_session("/tmp/b.jsonl", "/repo", "b"),
            ],
        }];
        app.focus = Focus::Sessions;
        app.session_idx = 1;

        app.toggle_current_session_selection();
        assert_eq!(app.selected_count_current_project(), 1);
        assert!(
            app.selected_sessions
                .contains(&PathBuf::from("/tmp/b.jsonl"))
        );

        app.toggle_current_session_selection();
        assert_eq!(app.selected_count_current_project(), 0);
    }

    #[test]
    fn select_all_and_invert_sessions_work() {
        let mut app = empty_test_app();
        app.projects = vec![ProjectBucket {
            cwd: String::from("/repo"),
            sessions: vec![
                sample_session("/tmp/a.jsonl", "/repo", "a"),
                sample_session("/tmp/b.jsonl", "/repo", "b"),
                sample_session("/tmp/c.jsonl", "/repo", "c"),
            ],
        }];
        app.focus = Focus::Sessions;
        app.session_idx = 1;

        app.select_all_sessions_current_project();
        assert_eq!(app.selected_count_current_project(), 3);

        app.invert_sessions_selection_current_project();
        assert_eq!(app.selected_count_current_project(), 0);
    }

    #[test]
    fn action_targets_prefers_selected_sessions() {
        let mut app = empty_test_app();
        app.focus = Focus::Sessions;
        app.projects = vec![ProjectBucket {
            cwd: String::from("/repo"),
            sessions: vec![
                sample_session("/tmp/a.jsonl", "/repo", "a"),
                sample_session("/tmp/b.jsonl", "/repo", "b"),
            ],
        }];
        app.session_idx = 0;
        app.selected_sessions.insert(PathBuf::from("/tmp/b.jsonl"));

        let targets = app.action_targets(Action::Move);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].id, "b");
    }

    #[test]
    fn delete_targets_prefers_selected_sessions() {
        let mut app = empty_test_app();
        app.focus = Focus::Sessions;
        app.projects = vec![ProjectBucket {
            cwd: String::from("/repo"),
            sessions: vec![
                sample_session("/tmp/a.jsonl", "/repo", "a"),
                sample_session("/tmp/b.jsonl", "/repo", "b"),
            ],
        }];
        app.session_idx = 0;
        app.selected_sessions.insert(PathBuf::from("/tmp/b.jsonl"));

        let targets = app.action_targets(Action::Delete);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].id, "b");
    }

    #[test]
    fn delete_confirmation_is_strict() {
        assert!(delete_confirmation_valid("DELETE"));
        assert!(!delete_confirmation_valid("delete"));
        assert!(!delete_confirmation_valid(" DELETE "));
    }

    #[test]
    fn project_actions_target_all_project_sessions() {
        let mut app = empty_test_app();
        app.focus = Focus::Projects;
        app.projects = vec![ProjectBucket {
            cwd: String::from("/repo"),
            sessions: vec![
                sample_session("/tmp/a.jsonl", "/repo", "a"),
                sample_session("/tmp/b.jsonl", "/repo", "b"),
            ],
        }];
        let targets = app.action_targets(Action::ProjectCopy);
        assert_eq!(targets.len(), 2);
    }

    #[test]
    fn preview_selected_text_uses_character_bounds() {
        let app = App {
            sessions_root: PathBuf::from("/tmp"),
            all_projects: Vec::new(),
            projects: Vec::new(),
            project_idx: 0,
            session_idx: 0,
            selected_sessions: HashSet::new(),
            session_select_anchor: None,
            focus: Focus::Preview,
            mode: Mode::Normal,
            pending_action: None,
            input: String::new(),
            input_focused: false,
            input_tab_last_at: None,
            input_tab_last_query: String::new(),
            search_query: String::new(),
            search_focused: false,
            search_dirty: false,
            preview_mode: PreviewMode::Chat,
            preview_selecting: false,
            preview_mouse_down_pos: None,
            drag_target: None,
            scroll_drag: None,
            status: String::new(),
            panes: PaneLayout::default(),
            project_width_pct: 20,
            session_width_pct: 36,
            project_scroll: 0,
            session_scroll: 0,
            preview_scroll: 0,
            preview_content_len: 2,
            preview_selection: None,
            preview_rendered_lines: vec![String::from("abcde"), String::from("vwxyz")],
            preview_focus_turn: None,
            preview_cache: HashMap::new(),
            preview_folded: HashMap::new(),
            preview_header_rows: Vec::new(),
            preview_session_path: None,
        };
        let text = app
            .preview_selected_text((0, 1), (1, 2))
            .expect("selection text");
        assert_eq!(text, "bcde\nvwx");
    }

    #[test]
    fn scroll_offset_from_mouse_maps_top_and_bottom() {
        let pane = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 12,
        };
        let top = scroll_offset_from_mouse_row(1, pane, 200, 10);
        let bottom = scroll_offset_from_mouse_row(10, pane, 200, 10);
        assert_eq!(top, 0);
        assert!(bottom >= 185);
    }

    #[test]
    fn session_item_lines_are_two_line_pretty_format() {
        let s = SessionSummary {
            path: PathBuf::from("/tmp/a.jsonl"),
            file_name: String::from("rollout-a.jsonl"),
            id: String::from("123456789abcdef"),
            cwd: String::from("/tmp"),
            started_at: String::from("2026-01-01T00:00:00Z"),
            event_count: 42,
            search_blob: String::new(),
        };
        let (a, b) = format_session_item_lines(&s);
        assert!(a.contains("| 42 events"));
        assert!(b.starts_with("12345678 | "));
    }

    #[test]
    fn build_preview_marks_toned_rows() {
        let dir = std::env::temp_dir().join(format!("cse-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("sample.jsonl");
        fs::write(&path, sample_chat_jsonl()).expect("write");

        let session = SessionSummary {
            path: path.clone(),
            file_name: String::from("sample.jsonl"),
            id: String::from("abc"),
            cwd: String::from("/tmp/x"),
            started_at: String::from("2026-01-01T00:00:00Z"),
            event_count: 4,
            search_blob: String::from("hello world normalized user"),
        };
        let preview = build_preview(&session, PreviewMode::Chat, 80).expect("preview");

        assert!(!preview.tone_rows.is_empty());
        assert!(preview.tone_rows.windows(2).all(|w| w[0].0 < w[1].0));
        let user_tone = preview
            .tone_rows
            .iter()
            .filter(|(_, tone)| *tone == BlockTone::User)
            .count();
        let assistant_tone = preview
            .tone_rows
            .iter()
            .filter(|(_, tone)| *tone == BlockTone::Assistant)
            .count();
        assert!(user_tone > 0);
        assert!(assistant_tone > 0);
    }

    #[test]
    fn build_preview_wraps_long_message_to_width() {
        let dir = std::env::temp_dir().join(format!("cse-wrap-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("w.jsonl");
        let data = [
            r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":"x","timestamp":"2026-01-01T00:00:00Z","cwd":"/tmp"}}"#,
            r#"{"timestamp":"2026-01-01T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"alpha beta gamma delta epsilon zeta eta theta iota kappa"}]}}"#,
        ]
        .join("\n");
        fs::write(&path, data).expect("write");
        let s = SessionSummary {
            path,
            file_name: String::from("w.jsonl"),
            id: String::from("x"),
            cwd: String::from("/tmp"),
            started_at: String::from("2026-01-01T00:00:00Z"),
            event_count: 2,
            search_blob: String::new(),
        };
        let preview = build_preview(&s, PreviewMode::Chat, 24).expect("preview");
        let joined = preview
            .lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("alpha beta gamma"));
        assert!(joined.contains("kappa"));
        assert!(preview.tone_rows.len() >= 4);
    }

    #[test]
    fn build_preview_does_not_truncate_turns() {
        let mut lines = Vec::new();
        lines.push(r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":"x","timestamp":"2026-01-01T00:00:00Z","cwd":"/tmp"}}"#.to_string());
        for i in 0..140 {
            lines.push(format!(
                r#"{{"timestamp":"2026-01-01T00:00:{i:02}Z","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"turn {i}"}}]}}}}"#
            ));
        }
        let content = lines.join("\n");
        let dir = std::env::temp_dir().join(format!("cse-all-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("all.jsonl");
        fs::write(&path, content).expect("write");
        let s = SessionSummary {
            path,
            file_name: String::from("all.jsonl"),
            id: String::from("x"),
            cwd: String::from("/tmp"),
            started_at: String::from("2026-01-01T00:00:00Z"),
            event_count: 141,
            search_blob: String::new(),
        };
        let preview = build_preview(&s, PreviewMode::Chat, 60).expect("preview");
        assert_eq!(preview.header_rows.len(), 140);
    }

    #[test]
    fn folded_turn_hides_body_lines() {
        let cached = CachedPreviewSource {
            mtime: SystemTime::UNIX_EPOCH,
            turns: vec![ChatTurn {
                role: String::from("user"),
                timestamp: String::from("2026-01-01T00:00:00Z"),
                text: String::from("line one line two"),
            }],
            events: Vec::new(),
        };
        let s = SessionSummary {
            path: PathBuf::from("/tmp/fold.jsonl"),
            file_name: String::from("fold.jsonl"),
            id: String::from("x"),
            cwd: String::from("/tmp"),
            started_at: String::from("2026-01-01T00:00:00Z"),
            event_count: 2,
            search_blob: String::new(),
        };
        let mut folded = HashSet::new();
        folded.insert(0usize);
        let preview = build_preview_from_cached(&s, PreviewMode::Chat, 40, &cached, &folded);
        let all = preview
            .lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("▶"));
        assert!(!all.contains("line one"));
    }

    #[test]
    fn preview_includes_assistant_count_line() {
        let cached = CachedPreviewSource {
            mtime: SystemTime::UNIX_EPOCH,
            turns: vec![
                ChatTurn {
                    role: String::from("user"),
                    timestamp: String::from("t1"),
                    text: String::from("u"),
                },
                ChatTurn {
                    role: String::from("assistant"),
                    timestamp: String::from("t2"),
                    text: String::from("a"),
                },
            ],
            events: Vec::new(),
        };
        let s = SessionSummary {
            path: PathBuf::from("/tmp/c.jsonl"),
            file_name: String::from("c.jsonl"),
            id: String::from("x"),
            cwd: String::from("/tmp"),
            started_at: String::from("t0"),
            event_count: 2,
            search_blob: String::new(),
        };
        let preview =
            build_preview_from_cached(&s, PreviewMode::Chat, 40, &cached, &HashSet::new());
        let joined = preview
            .lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("assistant=1"));
    }

    #[test]
    fn render_markdown_lines_preserves_code_fence_and_list_marker() {
        let md = "## Header\n\n- one two three four five\n\n```rust\nlet a = 1;\n```";
        let rendered = render_markdown_lines(md, 20);
        let joined = rendered.join("\n");
        assert!(joined.contains("Header"));
        assert!(joined.contains("- one two three"));
        assert!(joined.contains("let a = 1;"));
        assert!(!joined.contains("```"));
    }

    #[test]
    fn assistant_blocks_have_hairline_separator() {
        let cached = CachedPreviewSource {
            mtime: SystemTime::UNIX_EPOCH,
            turns: vec![
                ChatTurn {
                    role: String::from("assistant"),
                    timestamp: String::from("t1"),
                    text: String::from("a1"),
                },
                ChatTurn {
                    role: String::from("assistant"),
                    timestamp: String::from("t2"),
                    text: String::from("a2"),
                },
            ],
            events: Vec::new(),
        };
        let s = SessionSummary {
            path: PathBuf::from("/tmp/sep.jsonl"),
            file_name: String::from("sep.jsonl"),
            id: String::from("x"),
            cwd: String::from("/tmp"),
            started_at: String::from("t0"),
            event_count: 2,
            search_blob: String::new(),
        };
        let preview =
            build_preview_from_cached(&s, PreviewMode::Chat, 30, &cached, &HashSet::new());
        let rows = preview
            .lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>();
        let sep_idx = rows
            .iter()
            .position(|r| r.contains('─'))
            .expect("assistant separator");
        assert!(!preview.tone_rows.iter().any(|(idx, _)| *idx == sep_idx));
    }

    #[test]
    fn apply_search_filter_reduces_to_matching_sessions() {
        let s1 = SessionSummary {
            path: PathBuf::from("/tmp/a.jsonl"),
            file_name: String::from("a.jsonl"),
            id: String::from("a"),
            cwd: String::from("/repo/a"),
            started_at: String::from("2026-01-01T00:00:00Z"),
            event_count: 1,
            search_blob: String::from("deploy fix alpha"),
        };
        let s2 = SessionSummary {
            path: PathBuf::from("/tmp/b.jsonl"),
            file_name: String::from("b.jsonl"),
            id: String::from("b"),
            cwd: String::from("/repo/b"),
            started_at: String::from("2026-01-01T00:00:00Z"),
            event_count: 1,
            search_blob: String::from("unrelated text"),
        };

        let mut app = App {
            sessions_root: PathBuf::from("/tmp"),
            all_projects: vec![
                ProjectBucket {
                    cwd: String::from("/repo/a"),
                    sessions: vec![s1],
                },
                ProjectBucket {
                    cwd: String::from("/repo/b"),
                    sessions: vec![s2],
                },
            ],
            projects: Vec::new(),
            project_idx: 0,
            session_idx: 0,
            selected_sessions: HashSet::new(),
            session_select_anchor: None,
            focus: Focus::Projects,
            mode: Mode::Normal,
            pending_action: None,
            input: String::new(),
            input_focused: false,
            input_tab_last_at: None,
            input_tab_last_query: String::new(),
            search_query: String::from("alpha"),
            search_focused: true,
            search_dirty: true,
            preview_mode: PreviewMode::Chat,
            preview_selecting: false,
            preview_mouse_down_pos: None,
            drag_target: None,
            scroll_drag: None,
            status: String::new(),
            panes: PaneLayout::default(),
            project_width_pct: 28,
            session_width_pct: 38,
            project_scroll: 0,
            session_scroll: 0,
            preview_scroll: 0,
            preview_content_len: 0,
            preview_selection: None,
            preview_rendered_lines: Vec::new(),
            preview_focus_turn: None,
            preview_cache: HashMap::new(),
            preview_folded: HashMap::new(),
            preview_header_rows: Vec::new(),
            preview_session_path: None,
        };

        app.apply_search_filter();
        assert_eq!(app.projects.len(), 1);
        assert_eq!(app.projects[0].cwd, "/repo/a");
    }

    #[test]
    fn preview_toggle_all_folds_collapses_and_expands() {
        let mut app = App {
            sessions_root: PathBuf::from("/tmp"),
            all_projects: Vec::new(),
            projects: Vec::new(),
            project_idx: 0,
            session_idx: 0,
            selected_sessions: HashSet::new(),
            session_select_anchor: None,
            focus: Focus::Preview,
            mode: Mode::Normal,
            pending_action: None,
            input: String::new(),
            input_focused: false,
            input_tab_last_at: None,
            input_tab_last_query: String::new(),
            search_query: String::new(),
            search_focused: false,
            search_dirty: false,
            preview_mode: PreviewMode::Chat,
            preview_selecting: false,
            preview_mouse_down_pos: None,
            drag_target: None,
            scroll_drag: None,
            status: String::new(),
            panes: PaneLayout::default(),
            project_width_pct: 28,
            session_width_pct: 38,
            project_scroll: 0,
            session_scroll: 0,
            preview_scroll: 0,
            preview_content_len: 0,
            preview_selection: None,
            preview_rendered_lines: Vec::new(),
            preview_focus_turn: None,
            preview_cache: HashMap::new(),
            preview_folded: HashMap::new(),
            preview_header_rows: vec![(10, 0), (20, 1), (30, 2)],
            preview_session_path: Some(PathBuf::from("/tmp/x.jsonl")),
        };

        app.toggle_fold_all_preview_turns();
        let folded = app
            .preview_folded
            .get(&PathBuf::from("/tmp/x.jsonl"))
            .expect("folded set");
        assert!(folded.contains(&0) && folded.contains(&1) && folded.contains(&2));

        app.toggle_fold_all_preview_turns();
        let folded2 = app
            .preview_folded
            .get(&PathBuf::from("/tmp/x.jsonl"))
            .expect("folded set");
        assert!(folded2.is_empty());
    }
}
