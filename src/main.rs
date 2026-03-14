use std::collections::HashSet;
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs;
use std::io::{self, Stdout, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
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
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

fn main() -> Result<()> {
    let mut app = App::load()?;
    let mut tui = Tui::new()?;

    let run_result = run_app(&mut tui, &mut app);
    let restore_result = tui.restore();
    let launch = app.launch_codex_after_exit.clone();

    run_result?;
    restore_result?;
    if let Some(spec) = launch {
        launch_codex_resume(&spec)?;
    }
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
                app.panes.browser,
                app.panes.preview,
            ) {
                app.drag_target = Some(DragTarget::LeftSplitter);
                return;
            }

            if point_in_rect(mouse.column, mouse.row, app.panes.search) {
                app.search_focused = true;
                app.input_focused = false;
            } else if point_in_rect(mouse.column, mouse.row, app.panes.browser) {
                app.search_focused = false;
                if app.mode == Mode::Input {
                    app.input_focused = false;
                }
                app.focus = Focus::Projects;
                let rows = app.browser_rows();
                let idx = app.project_scroll + mouse_row_to_index(mouse.row, app.panes.browser);
                if let Some(row) = rows.get(idx).cloned() {
                    let is_double_click = app.register_browser_click(row.clone(), Instant::now());
                    if is_browser_toggle_hit(mouse.column, app.panes.browser, &row) {
                        match row.kind {
                            BrowserRowKind::Group { path } => {
                                app.browser_cursor = BrowserCursor::Group;
                                app.selected_group_path = Some(path);
                                app.ensure_selection_visible();
                                app.toggle_selected_group_collapsed_manual();
                            }
                            BrowserRowKind::Project { project_idx } => {
                                app.project_idx = project_idx;
                                app.browser_cursor = BrowserCursor::Project;
                                app.selected_group_path = None;
                                app.ensure_selection_visible();
                                app.toggle_current_project_collapsed_manual();
                            }
                            BrowserRowKind::Session { .. } => {}
                        }
                    } else {
                        app.set_browser_row(row.clone());
                        match row.kind {
                            BrowserRowKind::Group { .. } => {
                                if is_double_click {
                                    app.toggle_selected_group_collapsed_manual();
                                }
                            }
                            BrowserRowKind::Project { .. } => {
                                if is_double_click {
                                    app.toggle_current_project_collapsed_manual();
                                }
                            }
                            BrowserRowKind::Session { session_idx, .. } => {
                                let checkbox_hit = is_sessions_checkbox_hit(
                                    mouse.column,
                                    mouse.row,
                                    app.panes.browser,
                                );
                                if checkbox_hit {
                                    app.toggle_current_session_selection();
                                } else if is_double_click {
                                    app.focus = Focus::Preview;
                                } else {
                                    app.session_select_anchor = Some(session_idx);
                                }
                                app.preview_scroll = 0;
                            }
                        }
                    }
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
            if point_in_rect(mouse.column, mouse.row, app.panes.browser) {
                app.focus = Focus::Projects;
                app.move_up();
            } else if point_in_rect(mouse.column, mouse.row, app.panes.preview) {
                app.focus = Focus::Preview;
                app.move_up();
            }
        }
        MouseEventKind::ScrollDown => {
            if point_in_rect(mouse.column, mouse.row, app.panes.browser) {
                app.focus = Focus::Projects;
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
    Export,
    Delete,
    ProjectRename,
    ProjectCopy,
    AddRemote,
    Refresh,
    Quit,
}

fn status_buttons(app: &App) -> Vec<StatusButton> {
    if app.mode == Mode::Input {
        return vec![StatusButton::Apply, StatusButton::Cancel];
    }
    if app.focus == Focus::Projects
        && matches!(
            app.browser_cursor,
            BrowserCursor::Project | BrowserCursor::Group
        )
    {
        return vec![
            StatusButton::ProjectRename,
            StatusButton::ProjectCopy,
            StatusButton::AddRemote,
            StatusButton::Refresh,
            StatusButton::Quit,
        ];
    }
    if app.focus == Focus::Projects && app.browser_cursor == BrowserCursor::Session {
        return vec![
            StatusButton::SelectAll,
            StatusButton::Invert,
            StatusButton::Move,
            StatusButton::Copy,
            StatusButton::Fork,
            StatusButton::Export,
            StatusButton::Delete,
            StatusButton::Refresh,
            StatusButton::Quit,
        ];
    }
    vec![
        StatusButton::Move,
        StatusButton::Copy,
        StatusButton::Fork,
        StatusButton::Export,
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
        StatusButton::Export => "[Export]",
        StatusButton::Delete => "[Delete]",
        StatusButton::ProjectRename => "[Rename Folder]",
        StatusButton::ProjectCopy => "[Copy Folder]",
        StatusButton::AddRemote => "[Connect Remote]",
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
        StatusButton::Export => app.start_action(Action::Export),
        StatusButton::Delete => app.start_action(Action::Delete),
        StatusButton::ProjectRename => app.start_action(Action::ProjectRename),
        StatusButton::ProjectCopy => app.start_action(Action::ProjectCopy),
        StatusButton::AddRemote => app.start_action(Action::AddRemote),
        StatusButton::Refresh => {
            let _ = app.reload(true);
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
    if is_on_scrollbar(x, y, app.panes.browser) {
        return Some(ScrollTarget::Projects);
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
            let rows = app.browser_rows();
            let viewport = App::visible_rows(app.panes.browser.height, 1);
            let off = scroll_offset_from_mouse_row(y, app.panes.browser, rows.len(), viewport);
            app.project_scroll = off;
            app.focus = Focus::Projects;
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

fn is_sessions_checkbox_hit(x: u16, _y: u16, pane: ratatui::layout::Rect) -> bool {
    let col = mouse_col_to_index(x, pane);
    // Browser rows are single-line; session checkbox sits near the left gutter.
    col <= 7
}

fn is_browser_toggle_hit(x: u16, pane: ratatui::layout::Rect, row: &BrowserRow) -> bool {
    let col = mouse_col_to_index(x, pane);
    let indent = row.depth * 2;
    col >= indent && col <= indent + 3
}

fn copy_to_clipboard_osc52(text: &str) -> Result<()> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let mut out = io::stdout();
    write!(out, "\x1b]52;c;{b64}\x1b\\").context("failed OSC52 write")?;
    out.flush().context("failed stdout flush")?;
    Ok(())
}

fn launch_codex_resume(spec: &CodexLaunchSpec) -> Result<()> {
    let status = if let Some(ssh_target) = &spec.ssh_target {
        let inner = format!(
            "cd {} && codex resume {}",
            sh_single_quote(&path_to_string(&spec.cwd)),
            sh_single_quote(&spec.session_id)
        );
        let remote_cmd = wrap_remote_exec(spec.exec_prefix.as_deref(), &inner);
        let mut cmd = Command::new("ssh");
        add_ssh_options(&mut cmd, false);
        cmd.arg("-t")
            .arg(ssh_target)
            .arg(remote_cmd)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .with_context(|| format!("failed to launch remote codex resume via {}", ssh_target))?
    } else {
        Command::new("codex")
            .arg("resume")
            .arg(&spec.session_id)
            .current_dir(&spec.cwd)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .context("failed to launch codex resume")?
    };
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("codex resume exited with status {status}"))
    }
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
            KeyCode::Tab => {
                app.search_focused = false;
                app.next_focus();
            }
            KeyCode::BackTab => {
                app.search_focused = false;
                app.prev_focus();
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

    if key.modifiers.contains(KeyModifiers::CONTROL) && app.focus == Focus::Projects {
        match key.code {
            KeyCode::Up => {
                app.jump_project(-1);
                return Ok(false);
            }
            KeyCode::Down => {
                app.jump_project(1);
                return Ok(false);
            }
            KeyCode::Left => {
                app.collapse_all_projects_except_current();
                return Ok(false);
            }
            KeyCode::Right => {
                app.expand_all_projects();
                return Ok(false);
            }
            KeyCode::Char('c') => {
                app.copy_browser_selection(BrowserClipboardMode::Copy);
                return Ok(false);
            }
            KeyCode::Char('x') => {
                app.copy_browser_selection(BrowserClipboardMode::Cut);
                return Ok(false);
            }
            KeyCode::Char('v') => {
                app.paste_browser_clipboard()?;
                return Ok(false);
            }
            KeyCode::Char('r') => {
                app.reload(true)?;
                return Ok(false);
            }
            _ => {}
        }
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
            if app.focus == Focus::Projects && app.browser_cursor == BrowserCursor::Session {
                app.toggle_current_session_selection();
            }
        }
        KeyCode::Char('a') => {
            if app.focus == Focus::Projects && app.browser_cursor == BrowserCursor::Session {
                app.select_all_sessions_current_project();
            }
        }
        KeyCode::Char('i') => {
            if app.focus == Focus::Projects && app.browser_cursor == BrowserCursor::Session {
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
        KeyCode::Enter => {
            if app.mode == Mode::Normal {
                app.browser_enter();
            }
        }
        KeyCode::Esc => {
            if app.focus == Focus::Preview {
                app.focus = Focus::Projects;
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
        KeyCode::PageUp => {
            if app.focus == Focus::Preview {
                app.page_preview(-1);
            }
        }
        KeyCode::PageDown => {
            if app.focus == Focus::Preview {
                app.page_preview(1);
            }
        }
        KeyCode::Home => {
            if app.focus == Focus::Preview {
                app.jump_preview_to_edge(true);
            }
        }
        KeyCode::End => {
            if app.focus == Focus::Preview {
                app.jump_preview_to_edge(false);
            }
        }
        KeyCode::Left => {
            if app.focus == Focus::Preview {
                app.fold_focused_preview_turn();
            } else if app.focus == Focus::Projects {
                match app.browser_cursor {
                    BrowserCursor::Session => app.browser_cursor = BrowserCursor::Project,
                    BrowserCursor::Project | BrowserCursor::Group => app.collapse_current_project(),
                }
            }
        }
        KeyCode::Right => {
            if app.focus == Focus::Preview {
                app.unfold_focused_preview_turn();
            } else if app.focus == Focus::Projects {
                match app.browser_cursor {
                    BrowserCursor::Group => app.expand_current_project(),
                    BrowserCursor::Project => {
                        if app.current_project_collapsed() {
                            app.expand_current_project();
                        } else if app
                            .current_project()
                            .is_some_and(|project| !project.sessions.is_empty())
                        {
                            app.browser_cursor = BrowserCursor::Session;
                            app.ensure_selection_visible();
                        }
                    }
                    BrowserCursor::Session => {}
                }
            }
        }
        KeyCode::Char('g') | KeyCode::F(5) => app.reload(true)?,
        KeyCode::Char('m') => {
            if app.focus == Focus::Projects && app.browser_cursor == BrowserCursor::Project {
                app.start_action(Action::ProjectRename);
            } else if app.current_session().is_some() {
                app.start_action(Action::Move);
            }
        }
        KeyCode::Char('c') => {
            if app.focus == Focus::Projects && app.browser_cursor == BrowserCursor::Project {
                app.start_action(Action::ProjectCopy);
            } else if app.current_session().is_some() {
                app.start_action(Action::Copy);
            }
        }
        KeyCode::Char('f') => {
            if app.focus == Focus::Projects && app.browser_cursor == BrowserCursor::Project {
                app.status = String::from("Project scope supports rename/copy");
            } else if app.current_session().is_some() {
                app.start_action(Action::Fork);
            }
        }
        KeyCode::Char('e') => {
            if app.current_session().is_some() {
                app.start_action(Action::Export);
            }
        }
        KeyCode::Char('n') => {
            if app.focus == Focus::Preview {
                app.focus_next_preview_search_match();
            }
        }
        KeyCode::Char('N') => {
            if app.focus == Focus::Preview {
                app.focus_prev_preview_search_match();
            }
        }
        KeyCode::Char('o') => {
            if app.current_session().is_some() {
                app.plan_open_current_session_in_codex();
                return Ok(true);
            }
        }
        KeyCode::Char('d') | KeyCode::Delete => {
            if app.current_session().is_some() {
                app.start_action(Action::Delete);
            }
        }
        KeyCode::Char('r') => {
            if app.focus == Focus::Projects && app.browser_cursor == BrowserCursor::Project {
                app.start_action(Action::ProjectRename);
            }
        }
        KeyCode::Char('R') => {
            if app.focus == Focus::Projects {
                app.start_action(Action::AddRemote);
            }
        }
        KeyCode::Char('y') => {
            if app.focus == Focus::Projects && app.browser_cursor == BrowserCursor::Project {
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
            if let Err(err) = app.submit_input() {
                app.status = format!("{err:#}");
            }
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
                    Constraint::Percentage(app.project_width_pct + app.session_width_pct),
                    Constraint::Percentage(app.preview_width_pct()),
                ])
                .split(root[1]);

            app.panes = PaneLayout {
                search: root[0],
                browser: panes[0],
                preview: panes[1],
                status: root[2],
            };
            app.ensure_selection_visible();
            if app.search_visible() {
                render_search(frame, root[0], app);
            }
            render_browser(frame, app.panes.browser, app);
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Focus {
    Projects,
    Preview,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BrowserCursor {
    Group,
    Project,
    Session,
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum BrowserRowKind {
    Group {
        path: String,
    },
    Project {
        project_idx: usize,
    },
    Session {
        project_idx: usize,
        session_idx: usize,
    },
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct BrowserRow {
    kind: BrowserRowKind,
    depth: usize,
    label: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    Move,
    Copy,
    Fork,
    Export,
    Delete,
    ProjectRename,
    ProjectCopy,
    AddRemote,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BrowserClipboardMode {
    Copy,
    Cut,
}

#[derive(Clone)]
struct BrowserClipboard {
    mode: BrowserClipboardMode,
    targets: Vec<SessionSummary>,
    source_label: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScrollTarget {
    Projects,
    Preview,
}

#[derive(Clone)]
struct SessionSummary {
    path: PathBuf,
    storage_path: String,
    file_name: String,
    id: String,
    cwd: String,
    machine_name: String,
    machine_target: Option<String>,
    #[allow(dead_code)]
    machine_codex_home: Option<String>,
    machine_exec_prefix: Option<String>,
    started_at: String,
    modified_epoch: i64,
    #[allow(dead_code)]
    event_count: usize,
    user_message_count: usize,
    assistant_message_count: usize,
    search_blob: String,
}

#[derive(Clone)]
struct ProjectBucket {
    machine_name: String,
    machine_target: Option<String>,
    machine_codex_home: Option<String>,
    machine_exec_prefix: Option<String>,
    cwd: String,
    sessions: Vec<SessionSummary>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct ConfigMachine {
    name: String,
    ssh_target: String,
    #[serde(default)]
    exec_prefix: Option<String>,
    #[serde(default)]
    codex_home: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
struct AppConfig {
    #[serde(default)]
    machines: Vec<ConfigMachine>,
}

#[derive(Clone, Copy, Default)]
struct PaneLayout {
    search: ratatui::layout::Rect,
    browser: ratatui::layout::Rect,
    preview: ratatui::layout::Rect,
    status: ratatui::layout::Rect,
}

const REMOTE_SCAN_CACHE_TTL: Duration = Duration::from_secs(15);

struct App {
    config_path: PathBuf,
    config: AppConfig,
    sessions_root: PathBuf,
    state_db_path: Option<PathBuf>,
    all_projects: Vec<ProjectBucket>,
    projects: Vec<ProjectBucket>,
    project_idx: usize,
    session_idx: usize,
    browser_cursor: BrowserCursor,
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
    rendered_preview_cache: HashMap<PathBuf, RenderedPreviewCache>,
    preview_folded: HashMap<PathBuf, HashSet<usize>>,
    collapsed_projects: HashSet<String>,
    collapsed_groups: HashSet<String>,
    pinned_open_projects: HashSet<String>,
    selected_group_path: Option<String>,
    preview_header_rows: Vec<(usize, usize)>,
    preview_session_path: Option<PathBuf>,
    preview_search_matches: Vec<PreviewMatch>,
    preview_search_index: Option<usize>,
    last_browser_nav_at: Option<Instant>,
    pending_preview_search_jump: Option<(PathBuf, String)>,
    browser_clipboard: Option<BrowserClipboard>,
    last_browser_click: Option<(BrowserRow, Instant)>,
    launch_codex_after_exit: Option<CodexLaunchSpec>,
    remote_states: BTreeMap<String, RemoteMachineState>,
}

#[derive(Clone)]
struct CachedPreviewSource {
    mtime: SystemTime,
    turns: Vec<ChatTurn>,
    events: Vec<String>,
}

#[derive(Clone)]
struct RenderedPreviewCache {
    mode: PreviewMode,
    width: usize,
    folded: HashSet<usize>,
    data: Arc<PreviewData>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct PreviewMatch {
    row: usize,
    col_start: usize,
    col_end: usize,
    is_primary: bool,
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct CodexLaunchSpec {
    cwd: PathBuf,
    session_id: String,
    ssh_target: Option<String>,
    exec_prefix: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MachineTargetSpec {
    name: String,
    ssh_target: Option<String>,
    codex_home: String,
    cwd: String,
    exec_prefix: Option<String>,
}

#[derive(Clone, Default)]
struct RemoteMachineState {
    status: RemoteMachineStatus,
    last_error: Option<String>,
    cached_projects: Vec<ProjectBucket>,
    last_scan_at: Option<Instant>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RemoteMachineStatus {
    #[default]
    Unknown,
    Healthy,
    Cached,
    Error,
}

impl App {
    fn load() -> Result<Self> {
        let codex_home = resolve_codex_home()?;
        let config_path = resolve_config_path()?;
        let config = load_app_config(&config_path)?;
        let sessions_root = codex_home.join("sessions");
        let state_db_path = resolve_state_db_path(&codex_home);
        let cwd_base = env::current_dir().context("failed to resolve current directory")?;
        let repaired_count = repair_session_cwds(&sessions_root, &cwd_base)?;

        let mut app = Self {
            config_path,
            config,
            sessions_root,
            state_db_path,
            all_projects: Vec::new(),
            projects: Vec::new(),
            project_idx: 0,
            session_idx: 0,
            browser_cursor: BrowserCursor::Project,
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
            session_width_pct: 0,
            project_scroll: 0,
            session_scroll: 0,
            preview_scroll: 0,
            preview_content_len: 0,
            preview_selection: None,
            preview_rendered_lines: Vec::new(),
            preview_focus_turn: None,
            preview_cache: HashMap::new(),
            rendered_preview_cache: HashMap::new(),
            preview_folded: HashMap::new(),
            collapsed_projects: HashSet::new(),
            collapsed_groups: HashSet::new(),
            pinned_open_projects: HashSet::new(),
            selected_group_path: None,
            preview_header_rows: Vec::new(),
            preview_session_path: None,
            preview_search_matches: Vec::new(),
            preview_search_index: None,
            last_browser_nav_at: None,
            pending_preview_search_jump: None,
            browser_clipboard: None,
            last_browser_click: None,
            launch_codex_after_exit: None,
            remote_states: BTreeMap::new(),
        };

        app.reload(true)?;
        let synced_threads = app.sync_state_index()?;
        if repaired_count > 0 || synced_threads > 0 {
            app.status = format!(
                "Loaded {} projects, repaired {} session file(s), synced {} thread row(s)",
                app.projects.len(),
                repaired_count,
                synced_threads
            );
        }
        Ok(app)
    }

    fn reload(&mut self, force_remote_scan: bool) -> Result<()> {
        self.all_projects = self.scan_all_projects(force_remote_scan)?;
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
            self.browser_cursor = BrowserCursor::Project;
        }

        self.status = format!("Loaded {} projects", self.projects.len());
        if let Some(summary) = self.remote_health_summary() {
            self.status.push_str(&format!("  {summary}"));
        }
        self.ensure_selection_visible();
        Ok(())
    }

    fn scan_all_projects(&mut self, force_remote_scan: bool) -> Result<Vec<ProjectBucket>> {
        let mut all_projects = scan_local_sessions(&self.sessions_root)?;
        let mut states = BTreeMap::new();
        for machine in &self.config.machines {
            let previous = self
                .remote_states
                .get(&machine.name)
                .cloned()
                .unwrap_or_default();
            let next = self.scan_remote_machine(machine, &previous, force_remote_scan);
            all_projects.extend(next.cached_projects.iter().cloned());
            states.insert(machine.name.clone(), next);
        }
        self.remote_states = states;
        all_projects.sort_by(|a, b| {
            a.machine_name
                .cmp(&b.machine_name)
                .then_with(|| a.cwd.cmp(&b.cwd))
        });
        Ok(all_projects)
    }

    fn scan_remote_machine(
        &self,
        machine: &ConfigMachine,
        previous: &RemoteMachineState,
        force_remote_scan: bool,
    ) -> RemoteMachineState {
        let now = Instant::now();
        if !force_remote_scan
            && previous
                .last_scan_at
                .is_some_and(|last| now.duration_since(last) < REMOTE_SCAN_CACHE_TTL)
        {
            return previous.clone();
        }
        match scan_remote_sessions(machine) {
            Ok(projects) => RemoteMachineState {
                status: RemoteMachineStatus::Healthy,
                last_error: None,
                cached_projects: projects,
                last_scan_at: Some(now),
            },
            Err(err) if !previous.cached_projects.is_empty() => RemoteMachineState {
                status: RemoteMachineStatus::Cached,
                last_error: Some(err.to_string()),
                cached_projects: previous.cached_projects.clone(),
                last_scan_at: previous.last_scan_at.or(Some(now)),
            },
            Err(err) => RemoteMachineState {
                status: RemoteMachineStatus::Error,
                last_error: Some(err.to_string()),
                cached_projects: Vec::new(),
                last_scan_at: Some(now),
            },
        }
    }

    fn remote_status_for_machine(&self, machine_name: &str) -> RemoteMachineStatus {
        if machine_name == "local" {
            return RemoteMachineStatus::Healthy;
        }
        self.remote_states
            .get(machine_name)
            .map(|state| state.status)
            .unwrap_or(RemoteMachineStatus::Unknown)
    }

    fn remote_health_summary(&self) -> Option<String> {
        let mut healthy = 0usize;
        let mut cached = 0usize;
        let mut down = 0usize;
        for state in self.remote_states.values() {
            match state.status {
                RemoteMachineStatus::Healthy => healthy += 1,
                RemoteMachineStatus::Cached => cached += 1,
                RemoteMachineStatus::Error => down += 1,
                RemoteMachineStatus::Unknown => {}
            }
        }
        if healthy == 0 && cached == 0 && down == 0 {
            None
        } else {
            Some(format!("remotes ok={healthy} cached={cached} down={down}"))
        }
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
            Focus::Projects => Focus::Preview,
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
                self.move_browser_row(-1);
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
                self.move_browser_row(1);
            }
            Focus::Preview => {
                self.preview_scroll = self.preview_scroll.saturating_add(1);
            }
        }
    }

    fn page_preview(&mut self, direction: isize) {
        let viewport = self.panes.preview.height.saturating_sub(2) as usize;
        let step = viewport.saturating_sub(1).max(1);
        let max_scroll = self.preview_content_len.saturating_sub(viewport);
        if direction >= 0 {
            self.preview_scroll = (self.preview_scroll + step).min(max_scroll);
        } else {
            self.preview_scroll = self.preview_scroll.saturating_sub(step);
        }
    }

    fn jump_preview_to_edge(&mut self, to_top: bool) {
        if to_top {
            self.preview_scroll = 0;
            return;
        }
        let viewport = self.panes.preview.height.saturating_sub(2) as usize;
        self.preview_scroll = self.preview_content_len.saturating_sub(viewport);
    }

    fn move_browser_row(&mut self, delta: isize) {
        let rows = self.browser_rows();
        if rows.is_empty() {
            return;
        }
        let current = self.current_browser_row_index() as isize;
        let next = (current + delta).clamp(0, rows.len().saturating_sub(1) as isize) as usize;
        self.set_browser_row(rows[next].clone());
        self.session_select_anchor = None;
    }

    fn current_project_collapsed(&self) -> bool {
        self.current_project()
            .is_some_and(|project| project_set_contains(&self.collapsed_projects, project))
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

    fn browser_rows(&self) -> Vec<BrowserRow> {
        self.browser_render_rows()
    }

    fn browser_machine_roots(&self) -> Vec<String> {
        let mut roots = vec![String::from("local")];
        roots.extend(self.config.machines.iter().map(|m| m.name.clone()));
        roots.sort();
        roots.dedup();
        roots
    }

    fn browser_render_rows(&self) -> Vec<BrowserRow> {
        build_browser_rows(
            &self.projects,
            &self.browser_machine_roots(),
            &self.collapsed_groups,
            &self.collapsed_projects,
            &self.selected_sessions,
        )
    }

    fn current_browser_row_index(&self) -> usize {
        let rows = self.browser_rows();
        if let Some(idx) = rows
            .iter()
            .position(|row| match (&self.browser_cursor, &row.kind) {
                (BrowserCursor::Group, BrowserRowKind::Group { path }) => {
                    self.selected_group_path.as_deref() == Some(path.as_str())
                }
                (BrowserCursor::Project, BrowserRowKind::Project { project_idx }) => {
                    *project_idx == self.project_idx
                }
                (
                    BrowserCursor::Session,
                    BrowserRowKind::Session {
                        project_idx,
                        session_idx,
                    },
                ) => *project_idx == self.project_idx && *session_idx == self.session_idx,
                _ => false,
            })
        {
            return idx;
        }
        rows.iter()
            .position(|row| {
                matches!(
                    row.kind,
                    BrowserRowKind::Project { project_idx } if project_idx == self.project_idx
                )
            })
            .unwrap_or(0)
    }

    fn set_browser_row(&mut self, row: BrowserRow) {
        match row.kind {
            BrowserRowKind::Group { path } => {
                self.browser_cursor = BrowserCursor::Group;
                self.selected_group_path = Some(path.clone());
                self.project_idx = first_project_index_for_group(&self.projects, &path)
                    .unwrap_or(self.project_idx.min(self.projects.len().saturating_sub(1)));
                if let Some(state) = self.remote_states.get(&path) {
                    match state.status {
                        RemoteMachineStatus::Cached | RemoteMachineStatus::Error => {
                            let detail = state.last_error.as_deref().unwrap_or("unreachable");
                            self.status = format!(
                                "{} {} {}",
                                path,
                                machine_status_suffix(state.status),
                                detail
                            );
                        }
                        RemoteMachineStatus::Healthy | RemoteMachineStatus::Unknown => {}
                    }
                }
            }
            BrowserRowKind::Project { project_idx } => {
                self.project_idx = project_idx;
                self.browser_cursor = BrowserCursor::Project;
                self.selected_group_path = None;
                self.auto_manage_project_expansion();
            }
            BrowserRowKind::Session {
                project_idx,
                session_idx,
            } => {
                self.project_idx = project_idx;
                self.browser_cursor = BrowserCursor::Session;
                self.session_idx = session_idx;
                self.selected_group_path = None;
                self.auto_manage_project_expansion();
            }
        }
        self.clamp_session_idx();
        self.note_browser_navigation();
        self.ensure_selection_visible();
    }

    fn toggle_current_project_collapsed_manual(&mut self) {
        let Some(cwd) = self.current_project().map(|project| project.cwd.clone()) else {
            return;
        };
        let Some(project) = self.current_project().cloned() else {
            return;
        };
        let key = project_bucket_key(&project);
        if project_set_contains(&self.collapsed_projects, &project) {
            self.collapsed_projects.remove(&key);
            self.collapsed_projects.remove(&cwd);
            self.pinned_open_projects.insert(key.clone());
            self.pinned_open_projects.insert(cwd.clone());
            self.status = format!("Expanded {}", browser_display_path(&cwd));
        } else {
            self.collapsed_projects.insert(key.clone());
            self.collapsed_projects.insert(cwd.clone());
            self.pinned_open_projects.remove(&key);
            self.pinned_open_projects.remove(&cwd);
            self.browser_cursor = BrowserCursor::Project;
            self.status = format!("Collapsed {}", browser_display_path(&cwd));
        }
        self.note_browser_navigation();
        self.ensure_selection_visible();
    }

    fn toggle_selected_group_collapsed_manual(&mut self) {
        let Some(path) = self.selected_group_path.clone() else {
            return;
        };
        if self.collapsed_groups.contains(&path) {
            self.collapsed_groups.remove(&path);
            self.status = format!("Expanded {path}");
        } else {
            self.collapsed_groups.insert(path.clone());
            self.status = format!("Collapsed {path}");
        }
        self.note_browser_navigation();
        self.ensure_selection_visible();
    }

    fn collapse_current_project(&mut self) {
        if self.browser_cursor == BrowserCursor::Group {
            if let Some(path) = self.selected_group_path.clone() {
                self.collapsed_groups.insert(path.clone());
                self.status = format!("Collapsed {path}");
            }
            return;
        }
        if self.projects.is_empty() || self.current_project_collapsed() {
            return;
        }
        self.toggle_current_project_collapsed_manual();
    }

    fn expand_current_project(&mut self) {
        if self.browser_cursor == BrowserCursor::Group {
            if let Some(path) = self.selected_group_path.clone() {
                self.collapsed_groups.remove(&path);
                self.status = format!("Expanded {path}");
            }
            return;
        }
        if self.projects.is_empty() || !self.current_project_collapsed() {
            return;
        }
        self.toggle_current_project_collapsed_manual();
    }

    fn auto_manage_project_expansion(&mut self) {
        let Some(current_project) = self.current_project().cloned() else {
            return;
        };
        let current_key = project_bucket_key(&current_project);
        for project in &self.projects {
            let key = project_bucket_key(project);
            if key != current_key && !pinned_set_contains(&self.pinned_open_projects, project) {
                self.collapsed_projects.insert(key);
                self.collapsed_projects.insert(project.cwd.clone());
            }
        }
        self.collapsed_projects.remove(&current_key);
        self.collapsed_projects.remove(&current_project.cwd);
    }

    fn collapse_all_projects_except_current(&mut self) {
        let current_key = self.current_project().map(project_bucket_key);
        self.collapsed_projects.clear();
        self.pinned_open_projects.clear();
        self.collapsed_groups.clear();
        for project in &self.projects {
            let key = project_bucket_key(project);
            if Some(key.as_str()) != current_key.as_deref() {
                self.collapsed_projects.insert(key);
                self.collapsed_projects.insert(project.cwd.clone());
            } else {
                self.pinned_open_projects.insert(key);
                self.pinned_open_projects.insert(project.cwd.clone());
            }
        }
        self.browser_cursor = BrowserCursor::Project;
        self.ensure_selection_visible();
        self.status = String::from("Collapsed all folders except current");
    }

    fn expand_all_projects(&mut self) {
        self.collapsed_projects.clear();
        self.collapsed_groups.clear();
        self.pinned_open_projects = self
            .projects
            .iter()
            .flat_map(|project| [project_bucket_key(project), project.cwd.clone()])
            .collect();
        self.ensure_selection_visible();
        self.status = String::from("Expanded all folders");
    }

    fn collapse_all_projects(&mut self) {
        self.collapsed_projects = self
            .projects
            .iter()
            .flat_map(|project| [project_bucket_key(project), project.cwd.clone()])
            .collect();
        self.collapsed_groups = default_collapsed_group_paths(&self.projects);
        self.expand_initial_browser_groups();
        self.pinned_open_projects.clear();
        if let Some(first_root) = self.browser_machine_roots().first().cloned() {
            self.browser_cursor = BrowserCursor::Group;
            self.selected_group_path = Some(first_root);
        } else {
            self.browser_cursor = BrowserCursor::Project;
            self.selected_group_path = None;
        }
        self.project_scroll = 0;
        self.session_scroll = 0;
        self.preview_scroll = 0;
    }

    fn expand_initial_browser_groups(&mut self) {
        let Some(first_project) = self.projects.first() else {
            return;
        };
        let segments = browser_tree_segments_for_project(first_project);
        if segments.is_empty() {
            return;
        }
        let mut current = String::new();
        for (idx, segment) in segments.iter().enumerate().take(2) {
            if idx == 0 {
                current = segment.clone();
            } else if current == "/" {
                current = format!("/{segment}");
            } else {
                current = format!("{current}/{segment}");
            }
            self.collapsed_groups.remove(&current);
        }
    }

    fn jump_project(&mut self, delta: isize) {
        if self.projects.is_empty() {
            return;
        }
        let current = self.project_idx as isize;
        let next = (current + delta).clamp(0, self.projects.len().saturating_sub(1) as isize);
        self.project_idx = next as usize;
        self.browser_cursor = BrowserCursor::Project;
        self.selected_group_path = None;
        self.auto_manage_project_expansion();
        self.session_select_anchor = None;
        self.note_browser_navigation();
        self.ensure_selection_visible();
        self.status = String::from("Jumped to project");
    }

    fn browser_enter(&mut self) {
        if self.focus != Focus::Projects {
            return;
        }

        match self.browser_cursor {
            BrowserCursor::Group => {
                self.toggle_selected_group_collapsed_manual();
            }
            BrowserCursor::Project => {
                self.toggle_current_project_collapsed_manual();
            }
            BrowserCursor::Session => {
                self.focus = Focus::Preview;
            }
        }

        self.ensure_selection_visible();
    }

    fn note_browser_navigation(&mut self) {
        self.last_browser_nav_at = Some(Instant::now());
    }

    fn current_preview_session(&self) -> Option<SessionSummary> {
        self.current_preview_session_at(Instant::now())
    }

    fn current_preview_session_at(&self, now: Instant) -> Option<SessionSummary> {
        let current = self.current_session().cloned();
        if self.focus == Focus::Preview {
            return current;
        }
        if let Some((path, _)) = &self.pending_preview_search_jump {
            return self.find_session_by_path(path).or(current);
        }

        let should_defer = self
            .last_browser_nav_at
            .is_some_and(|last| now.duration_since(last) < Duration::from_millis(180));
        if !should_defer {
            return current;
        }

        self.preview_session_path
            .as_ref()
            .and_then(|path| self.find_session_by_path(path))
            .or(current)
    }

    fn find_session_by_path(&self, path: &Path) -> Option<SessionSummary> {
        self.projects
            .iter()
            .flat_map(|project| project.sessions.iter())
            .find(|session| session.path == path)
            .cloned()
            .or_else(|| {
                self.all_projects
                    .iter()
                    .flat_map(|project| project.sessions.iter())
                    .find(|session| session.path == path)
                    .cloned()
            })
    }

    fn ensure_selection_visible(&mut self) {
        let visible = Self::visible_rows(self.panes.browser.height, 1);
        let current = self.current_browser_row_index();
        if current < self.project_scroll {
            self.project_scroll = current;
        } else if current >= self.project_scroll + visible {
            self.project_scroll = current + 1 - visible;
        }
    }

    fn apply_search_filter(&mut self) {
        if self.search_query.trim().is_empty() {
            self.projects = self.all_projects.clone();
            self.project_idx = self.project_idx.min(self.projects.len().saturating_sub(1));
            self.clamp_session_idx();
            self.collapse_all_projects();
            self.preview_search_matches.clear();
            self.preview_search_index = None;
            self.pending_preview_search_jump = None;
            self.search_dirty = false;
            return;
        }

        let query = self.search_query.to_lowercase();
        let mut filtered = Vec::new();
        let mut total_matches = 0usize;

        for project in &self.all_projects {
            let mut scored: Vec<(i64, SessionSummary)> = Vec::new();
            for session in &project.sessions {
                if let Some(score) = search_score(
                    &query,
                    &session.search_blob,
                    &project.cwd,
                    &session.file_name,
                    &session.id,
                ) {
                    scored.push((score, session.clone()));
                }
            }

            if !scored.is_empty() {
                total_matches += scored.len();
                scored.sort_by(|a, b| {
                    b.0.cmp(&a.0)
                        .then_with(|| b.1.started_at.cmp(&a.1.started_at))
                });
                let best_score = scored.first().map(|(score, _)| *score).unwrap_or(i64::MIN);
                filtered.push((
                    best_score,
                    ProjectBucket {
                        machine_name: project.machine_name.clone(),
                        machine_target: project.machine_target.clone(),
                        machine_codex_home: project.machine_codex_home.clone(),
                        machine_exec_prefix: project.machine_exec_prefix.clone(),
                        cwd: project.cwd.clone(),
                        sessions: scored.into_iter().map(|(_, s)| s).collect(),
                    },
                ));
            }
        }

        filtered.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cwd.cmp(&b.1.cwd)));
        self.projects = filtered.into_iter().map(|(_, project)| project).collect();
        self.project_idx = 0;
        self.session_idx = 0;
        self.browser_cursor = BrowserCursor::Project;
        self.selected_group_path = None;
        self.collapsed_groups = default_collapsed_group_paths(&self.projects);
        if let Some(first_project) = self.projects.first() {
            if let Some(first_session) = first_project.sessions.first() {
                self.browser_cursor = BrowserCursor::Session;
                self.collapsed_projects
                    .remove(&project_bucket_key(first_project));
                self.collapsed_projects.remove(&first_project.cwd);
                expand_group_ancestors_for_project(
                    &self.projects,
                    &mut self.collapsed_groups,
                    &first_project.cwd,
                );
                self.pending_preview_search_jump =
                    Some((first_session.path.clone(), self.search_query.clone()));
            } else {
                self.pending_preview_search_jump = None;
            }
        } else {
            self.pending_preview_search_jump = None;
        }
        self.project_scroll = 0;
        self.session_scroll = 0;
        self.preview_scroll = 0;
        self.note_browser_navigation();
        self.ensure_selection_visible();
        self.status = format!(
            "Search '{}' matched {} session(s) in {} project(s)",
            self.search_query,
            total_matches,
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
        let mut r = 100i16 - p;

        match self.focus {
            Focus::Projects => {
                p += delta;
                r -= delta;
            }
            Focus::Preview => {
                r += delta;
                p -= delta;
            }
        }

        if p < min || r < min {
            return;
        }

        self.project_width_pct = p as u16;
        self.session_width_pct = 0;
    }

    fn resize_from_mouse(&mut self, target: DragTarget, mouse_x: u16) {
        let total_width = self
            .panes
            .browser
            .width
            .saturating_add(self.panes.preview.width);
        if total_width == 0 {
            return;
        }

        let x0 = self.panes.browser.x;
        let right = x0.saturating_add(total_width);

        let split = match target {
            DragTarget::LeftSplitter => {
                mouse_x.clamp(x0.saturating_add(12), right.saturating_sub(12))
            }
        };

        let p = split.saturating_sub(x0) as f32 / total_width as f32 * 100.0;
        let mut p_pct = p.round() as i16;
        let mut s_pct = 100 - p_pct;
        let min = 15i16;
        if p_pct < min {
            p_pct = min;
            s_pct = 100 - p_pct;
        }
        if s_pct < min {
            s_pct = min;
            p_pct = 100 - s_pct;
        }
        if p_pct >= min && s_pct >= min {
            self.project_width_pct = p_pct as u16;
            self.session_width_pct = 0;
        }
    }

    fn preview_for_session(
        &mut self,
        session: &SessionSummary,
        mode: PreviewMode,
        inner_width: usize,
    ) -> Result<Arc<PreviewData>> {
        let (mtime, content, stale) = if session.machine_target.is_none() {
            let meta = fs::metadata(&session.storage_path)
                .with_context(|| format!("failed metadata {}", session.storage_path))?;
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let stale = self
                .preview_cache
                .get(&session.path)
                .is_none_or(|cached| cached.mtime < mtime);
            let content = if stale {
                Some(
                    fs::read_to_string(&session.storage_path)
                        .with_context(|| format!("failed to read {}", session.storage_path))?,
                )
            } else {
                None
            };
            (mtime, content, stale)
        } else {
            let stale = !self.preview_cache.contains_key(&session.path);
            let content = if stale {
                Some(fetch_remote_session_content(session)?)
            } else {
                None
            };
            (SystemTime::UNIX_EPOCH, content, stale)
        };

        if stale {
            let content = content.unwrap_or_default();
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
            .unwrap_or_else(|| default_folded_turns(&coalesce_chat_turns(&cached.turns)));

        if let Some(rendered) = self.rendered_preview_cache.get(&session.path)
            && rendered.mode == mode
            && rendered.width == inner_width
            && rendered.folded == folded
        {
            return Ok(Arc::clone(&rendered.data));
        }

        let data = Arc::new(build_preview_from_cached(
            session,
            mode,
            inner_width,
            cached,
            &folded,
        ));
        self.rendered_preview_cache.insert(
            session.path.clone(),
            RenderedPreviewCache {
                mode,
                width: inner_width,
                folded,
                data: Arc::clone(&data),
            },
        );
        Ok(data)
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

    fn focus_next_preview_search_match(&mut self) {
        if self.preview_search_matches.is_empty() {
            return;
        }
        let next = match self.preview_search_index {
            Some(idx) => (idx + 1).min(self.preview_search_matches.len().saturating_sub(1)),
            None => 0,
        };
        self.preview_search_index = Some(next);
        self.scroll_preview_match_into_view(next);
    }

    fn focus_prev_preview_search_match(&mut self) {
        if self.preview_search_matches.is_empty() {
            return;
        }
        let prev = self.preview_search_index.unwrap_or(0).saturating_sub(1);
        self.preview_search_index = Some(prev);
        self.scroll_preview_match_into_view(prev);
    }

    fn scroll_preview_match_into_view(&mut self, match_idx: usize) {
        let Some(found) = self.preview_search_matches.get(match_idx) else {
            return;
        };
        let viewport = self.panes.preview.height.saturating_sub(2) as usize;
        self.preview_scroll = found.row.saturating_sub(viewport / 3);
        self.preview_focus_turn =
            preview_turn_at_or_before_row(&self.preview_header_rows, found.row);
    }

    fn plan_open_current_session_in_codex(&mut self) -> Option<CodexLaunchSpec> {
        let session = self.current_session()?.clone();
        let launch = CodexLaunchSpec {
            cwd: PathBuf::from(&session.cwd),
            session_id: session.id.clone(),
            ssh_target: session.machine_target.clone(),
            exec_prefix: session.machine_exec_prefix.clone(),
        };
        self.launch_codex_after_exit = Some(launch.clone());
        Some(launch)
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
        if self.browser_cursor != BrowserCursor::Session {
            return None;
        }
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

    fn machine_specs(&self) -> Vec<(String, Option<String>, String, Option<String>)> {
        let mut out = vec![(
            String::from("local"),
            None,
            path_to_string(
                self.sessions_root
                    .parent()
                    .unwrap_or_else(|| Path::new("/")),
            ),
            None,
        )];
        for machine in &self.config.machines {
            out.push((
                machine.name.clone(),
                Some(machine.ssh_target.clone()),
                machine
                    .codex_home
                    .clone()
                    .unwrap_or_else(|| String::from("~/.codex")),
                machine.exec_prefix.clone(),
            ));
        }
        out
    }

    fn resolve_machine_target(&self, raw: &str) -> Result<MachineTargetSpec> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(anyhow!("Target path is empty"));
        }

        if let Some(colon_idx) = trimmed.find(':') {
            let prefix = trimmed[..colon_idx].trim();
            let rest = trimmed[colon_idx + 1..].trim();
            if !prefix.is_empty() {
                if let Some((name, ssh_target, codex_home, exec_prefix)) = self
                    .machine_specs()
                    .into_iter()
                    .find(|(name, _, _, _)| name == prefix)
                {
                    let cwd = if ssh_target.is_none() {
                        normalize_local_target_cwd(
                            rest,
                            &env::current_dir().context("failed to resolve current directory")?,
                        )?
                    } else {
                        rest.to_string()
                    };
                    return Ok(MachineTargetSpec {
                        name,
                        ssh_target,
                        codex_home,
                        cwd,
                        exec_prefix,
                    });
                }
            }
        }

        Ok(MachineTargetSpec {
            name: String::from("local"),
            ssh_target: None,
            codex_home: path_to_string(
                self.sessions_root
                    .parent()
                    .unwrap_or_else(|| Path::new("/")),
            ),
            cwd: normalize_local_target_cwd(
                trimmed,
                &env::current_dir().context("failed to resolve current directory")?,
            )?,
            exec_prefix: None,
        })
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
            Action::AddRemote => Vec::new(),
            Action::Move | Action::Copy | Action::Fork | Action::Export | Action::Delete => {
                let selected = self.selected_sessions_in_current_project();
                if !selected.is_empty() {
                    selected
                } else {
                    self.current_session().cloned().into_iter().collect()
                }
            }
        }
    }

    fn browser_copy_targets(&self) -> Vec<SessionSummary> {
        match self.browser_cursor {
            BrowserCursor::Group => Vec::new(),
            BrowserCursor::Project => self
                .current_project()
                .map(|project| project.sessions.clone())
                .unwrap_or_default(),
            BrowserCursor::Session => self.action_targets(Action::Copy),
        }
    }

    fn copy_browser_selection(&mut self, mode: BrowserClipboardMode) {
        let targets = self.browser_copy_targets();
        if targets.is_empty() {
            self.status = match self.browser_cursor {
                BrowserCursor::Group => String::from("Select a project folder or session"),
                BrowserCursor::Project => String::from("No sessions in selected folder"),
                BrowserCursor::Session => String::from("No session selected"),
            };
            return;
        }

        let source_label = match self.browser_cursor {
            BrowserCursor::Group => self
                .selected_group_path
                .clone()
                .unwrap_or_else(|| String::from("<group>")),
            BrowserCursor::Project => self
                .current_project()
                .map(|project| browser_display_path(&project.cwd))
                .unwrap_or_else(|| String::from("<unknown>")),
            BrowserCursor::Session => targets
                .first()
                .map(|session| browser_display_path(&session.cwd))
                .unwrap_or_else(|| String::from("<unknown>")),
        };

        self.browser_clipboard = Some(BrowserClipboard {
            mode,
            targets: targets.clone(),
            source_label: source_label.clone(),
        });
        self.status = match mode {
            BrowserClipboardMode::Copy => format!(
                "Copied {} session(s) from {}. Select target folder and press Ctrl+V",
                targets.len(),
                source_label
            ),
            BrowserClipboardMode::Cut => format!(
                "Cut {} session(s) from {}. Select target folder and press Ctrl+V",
                targets.len(),
                source_label
            ),
        };
    }

    fn paste_browser_clipboard(&mut self) -> Result<()> {
        let Some(clipboard) = self.browser_clipboard.clone() else {
            self.status = String::from("Clipboard empty");
            return Ok(());
        };
        let Some(target_project) = self.current_project() else {
            self.status = String::from("No target folder selected");
            return Ok(());
        };
        let target = MachineTargetSpec {
            name: target_project.machine_name.clone(),
            ssh_target: target_project.machine_target.clone(),
            codex_home: target_project
                .machine_codex_home
                .clone()
                .unwrap_or_else(|| {
                    path_to_string(
                        self.sessions_root
                            .parent()
                            .unwrap_or_else(|| Path::new("/")),
                    )
                }),
            exec_prefix: target_project.machine_exec_prefix.clone(),
            cwd: target_project.cwd.clone(),
        };
        let mut ok = 0usize;
        let mut skipped = 0usize;
        let mut failures = Vec::new();

        for session in &clipboard.targets {
            let result = match clipboard.mode {
                BrowserClipboardMode::Copy => {
                    self.apply_session_action_to_target(Action::Copy, session, &target)
                }
                BrowserClipboardMode::Cut => {
                    if session.machine_target == target.ssh_target && session.cwd == target.cwd {
                        skipped += 1;
                        Ok(())
                    } else {
                        self.apply_session_action_to_target(Action::Move, session, &target)
                    }
                }
            };
            match result {
                Ok(()) => ok += 1,
                Err(err) => failures.push(format!("{}: {}", session.file_name, err)),
            }
        }

        if ok > 0 || skipped > 0 {
            self.reload(false)?;
        }
        self.selected_sessions.clear();
        self.session_select_anchor = None;

        if clipboard.mode == BrowserClipboardMode::Cut && failures.is_empty() {
            self.browser_clipboard = None;
        }

        let verb = match clipboard.mode {
            BrowserClipboardMode::Copy => "Pasted",
            BrowserClipboardMode::Cut => "Moved",
        };
        self.status = if failures.is_empty() {
            if skipped > 0 {
                format!(
                    "{verb} {ok} session(s) from {} into {} (skipped {skipped})",
                    clipboard.source_label,
                    format!("{}:{}", target.name, browser_display_path(&target.cwd))
                )
            } else {
                format!(
                    "{verb} {ok} session(s) from {} into {}",
                    clipboard.source_label,
                    format!("{}:{}", target.name, browser_display_path(&target.cwd))
                )
            }
        } else {
            let first = failures
                .first()
                .cloned()
                .unwrap_or_else(|| String::from("unknown error"));
            format!(
                "{verb} {ok} session(s), {} failed, skipped {skipped}. First error: {first}",
                failures.len()
            )
        };
        Ok(())
    }

    fn register_browser_click(&mut self, row: BrowserRow, now: Instant) -> bool {
        const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(450);
        let is_double = self
            .last_browser_click
            .clone()
            .is_some_and(|(last_row, last_at)| {
                last_row == row && now.duration_since(last_at) <= DOUBLE_CLICK_WINDOW
            });
        self.last_browser_click = Some((row, now));
        is_double
    }

    fn start_action(&mut self, action: Action) {
        let targets = self.action_targets(action);
        if action != Action::AddRemote && targets.is_empty() {
            self.status = match action {
                Action::ProjectRename | Action::ProjectCopy => String::from("No project selected"),
                Action::AddRemote => String::from("Enter remote connection details"),
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
                "Move {} session(s): enter target path (`/path` or `machine:/path`) and press Enter",
                targets.len()
            ),
            Action::Copy => format!(
                "Copy {} session(s): enter target path (`/path` or `machine:/path`) and press Enter",
                targets.len()
            ),
            Action::Fork => format!(
                "Fork {} session(s): enter target path (`/path` or `machine:/path`) and press Enter",
                targets.len()
            ),
            Action::Export => format!(
                "Export {} session(s): enter user@host:/remote/project/path and press Enter",
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
                "Copy folder sessions ({}) to target path (`/path` or `machine:/path`) and press Enter",
                targets.len()
            ),
            Action::AddRemote => String::from(
                "Add remote: enter user@host, name=user@host, name=user@host:/remote/.codex, or name=user@host|exec-prefix|/remote/.codex and press Enter",
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
        if action != Action::AddRemote && targets.is_empty() {
            self.status = String::from("No applicable sessions for this action");
            return Ok(());
        }
        let target_display = self.input.trim().to_string();
        let target_str = match action {
            Action::Delete => {
                if !delete_confirmation_valid(&self.input) {
                    self.status = String::from("Delete cancelled: type DELETE to confirm");
                    return Ok(());
                }
                String::new()
            }
            Action::Export => self.input.trim().to_string(),
            Action::AddRemote => self.input.trim().to_string(),
            _ => self.resolve_machine_target(&self.input)?.cwd,
        };
        let mut ok = 0usize;
        let mut skipped = 0usize;
        let mut failures = Vec::new();

        match action {
            Action::AddRemote => {
                let machine = parse_config_machine_input(&self.input)?;
                upsert_config_machine(&mut self.config, machine);
                save_app_config(&self.config_path, &self.config)?;
                ok = 1;
                self.reload(true)?;
            }
            _ => {
                let target_machine = if matches!(
                    action,
                    Action::Move
                        | Action::Copy
                        | Action::Fork
                        | Action::ProjectRename
                        | Action::ProjectCopy
                ) {
                    Some(self.resolve_machine_target(&self.input)?)
                } else {
                    None
                };
                for session in &targets {
                    let result = match action {
                        Action::Move
                        | Action::ProjectRename
                        | Action::Copy
                        | Action::ProjectCopy
                        | Action::Fork => {
                            let target = target_machine.as_ref().expect("target machine");
                            if session.machine_target == target.ssh_target
                                && session.cwd == target.cwd
                            {
                                skipped += 1;
                                Ok(())
                            } else {
                                self.apply_session_action_to_target(action, session, target)
                            }
                        }
                        Action::Export => export_session_via_ssh(session, &target_str),
                        Action::Delete => self.apply_delete_action(session),
                        Action::AddRemote => Ok(()),
                    };
                    match result {
                        Ok(()) => ok += 1,
                        Err(err) => failures.push(format!("{}: {}", session.file_name, err)),
                    }
                }
            }
        }

        self.mode = Mode::Normal;
        self.pending_action = None;
        self.input.clear();
        self.input_focused = false;
        self.clear_input_completion_cycle();

        if !matches!(action, Action::Export | Action::AddRemote) && (ok > 0 || skipped > 0) {
            self.reload(false)?;
        }
        self.selected_sessions.clear();
        self.session_select_anchor = None;

        let action_name = match action {
            Action::Move => "moved",
            Action::Copy => "copied",
            Action::Fork => "forked",
            Action::Export => "exported",
            Action::Delete => "deleted",
            Action::ProjectRename => "renamed",
            Action::ProjectCopy => "copied",
            Action::AddRemote => "connected",
        };
        self.status = if failures.is_empty() {
            if action == Action::Delete {
                format!("{action_name} {ok} session(s)")
            } else if skipped > 0 {
                format!(
                    "{action_name} {ok} session(s), skipped {skipped} unchanged -> {target_display}"
                )
            } else {
                format!("{action_name} {ok} session(s) -> {target_display}")
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

    fn sync_state_index(&self) -> Result<usize> {
        let Some(db_path) = self.state_db_path.as_deref() else {
            return Ok(0);
        };
        sync_threads_db_from_projects(db_path, &self.all_projects)
    }

    fn sync_state_thread(&self, session: &SessionSummary, target_cwd: &str) -> Result<bool> {
        let Some(db_path) = self.state_db_path.as_deref() else {
            return Ok(false);
        };
        sync_thread_record(
            db_path,
            &session.id,
            &session.path,
            target_cwd,
            &session.path,
        )
    }

    fn apply_session_action_to_target(
        &self,
        action: Action,
        session: &SessionSummary,
        target: &MachineTargetSpec,
    ) -> Result<()> {
        match action {
            Action::Move | Action::ProjectRename => {
                if session.machine_target == target.ssh_target && session.cwd == target.cwd {
                    return Ok(());
                }
                if session.machine_target == target.ssh_target {
                    if session.machine_target.is_none() {
                        rewrite_session_file(Path::new(&session.storage_path), &target.cwd, false)?;
                        self.sync_state_thread(session, &target.cwd)?;
                    } else {
                        rewrite_remote_session_file(session, &target.cwd, false)?;
                    }
                    return Ok(());
                }
                self.apply_session_action_to_target(Action::Copy, session, target)?;
                self.apply_delete_action(session)?;
                Ok(())
            }
            Action::Copy | Action::ProjectCopy | Action::Fork | Action::Export => {
                let fork = matches!(action, Action::Fork);
                let (out, session_id, _) = duplicate_session_content(session, &target.cwd, fork)?;
                if let Some(ssh_target) = &target.ssh_target {
                    let remote_path = write_new_remote_session(
                        ssh_target,
                        target.exec_prefix.as_deref(),
                        &target.codex_home,
                        &session_id,
                        &out,
                    )?;
                    let sync_session = SessionSummary {
                        id: session_id,
                        cwd: target.cwd.clone(),
                        storage_path: remote_path.clone(),
                        machine_exec_prefix: target.exec_prefix.clone(),
                        ..session.clone()
                    };
                    sync_remote_thread_index(
                        ssh_target,
                        target.exec_prefix.as_deref(),
                        &remote_path,
                        &target.cwd,
                        &sync_session,
                    )?;
                } else {
                    let new_path = write_new_local_session(&self.sessions_root, &session_id, &out)?;
                    let _ = self.sync_state_thread(
                        &SessionSummary {
                            id: session_id,
                            cwd: target.cwd.clone(),
                            storage_path: path_to_string(&new_path),
                            path: new_path.clone(),
                            machine_name: String::from("local"),
                            machine_target: None,
                            machine_codex_home: None,
                            machine_exec_prefix: None,
                            ..session.clone()
                        },
                        &target.cwd,
                    )?;
                }
                Ok(())
            }
            Action::Delete => self.apply_delete_action(session),
            Action::AddRemote => Ok(()),
        }
    }

    fn apply_delete_action(&self, session: &SessionSummary) -> Result<()> {
        if session.machine_target.is_none() {
            delete_session_file(Path::new(&session.storage_path))
        } else {
            delete_remote_session_file(session)
        }
    }
}

fn render_search(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &App) {
    let focus_style = if app.search_focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };

    let query_prefix = if app.search_focused { ">" } else { " " };
    let cursor = if app.search_focused { "█" } else { " " };
    let content = format!("{query_prefix} {}{cursor}", app.search_query);

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

fn render_browser(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &App) {
    let rows = app.browser_rows();
    let items: Vec<ListItem> = rows
        .iter()
        .map(|row| {
            let indent = "  ".repeat(row.depth);
            match &row.kind {
                BrowserRowKind::Session {
                    project_idx,
                    session_idx,
                } => {
                    let session = &app.projects[*project_idx].sessions[*session_idx];
                    let selected = app.selected_sessions.contains(&session.path);
                    let mark = if selected { "◉" } else { "◌" };
                    let line = format!("{indent}  {mark} 🗨 {}", row.label);
                    let base = if selected {
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    ListItem::new(Line::from(prepend_style(
                        highlight_spans(&line, &app.search_query),
                        base,
                    )))
                }
                BrowserRowKind::Group { path } => {
                    let collapsed = app.collapsed_groups.contains(path);
                    let icon = if row.depth == 0 { "🖥" } else { "📁" };
                    let group_label = if row.depth == 0 {
                        format!(
                            "{} {}",
                            row.label,
                            machine_status_suffix(app.remote_status_for_machine(&row.label))
                        )
                    } else {
                        row.label.clone()
                    };
                    let label = format!(
                        "{indent}{} {} {}",
                        if collapsed { "▶" } else { "▼" },
                        icon,
                        group_label
                    );
                    ListItem::new(Line::from(prepend_style(
                        highlight_spans(&label, &app.search_query),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    )))
                }
                BrowserRowKind::Project { project_idx } => {
                    let project = &app.projects[*project_idx];
                    let collapsed = project_set_contains(&app.collapsed_projects, project);
                    let label = format!(
                        "{indent}{} 📁 {} ({})",
                        if collapsed { "▶" } else { "▼" },
                        row.label,
                        project.sessions.len()
                    );
                    ListItem::new(Line::from(prepend_style(
                        highlight_spans(&label, &app.search_query),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    )))
                }
            }
        })
        .collect();

    let mut state = ListState::default();
    if !rows.is_empty() {
        state.select(Some(app.current_browser_row_index()));
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
                .title(format!(
                    "Browser [{} selected] (folder+sessions)",
                    app.selected_count_current_project()
                ))
                .borders(Borders::ALL)
                .border_style(focus_style)
                .style(Style::default().add_modifier(Modifier::DIM)),
        )
        .highlight_style(browser_highlight_style())
        .highlight_symbol(" > ");

    frame.render_stateful_widget(list, area, &mut state);
    render_thin_scrollbar(
        frame,
        area,
        app.project_scroll,
        rows.len(),
        App::visible_rows(area.height, 1),
    );
}

fn browser_highlight_style() -> Style {
    Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
}

fn machine_status_suffix(status: RemoteMachineStatus) -> &'static str {
    match status {
        RemoteMachineStatus::Healthy => "[ok]",
        RemoteMachineStatus::Cached => "[cached]",
        RemoteMachineStatus::Error => "[offline]",
        RemoteMachineStatus::Unknown => "[unknown]",
    }
}

fn format_session_browser_line(session: &SessionSummary) -> String {
    let chars = session.id.chars().collect::<Vec<_>>();
    let start = chars.len().saturating_sub(7);
    let mut out = chars[start..].iter().collect::<String>();
    if is_user_only_session(session) {
        out.push_str(" !");
    }
    out
}

fn browser_display_path(path: &str) -> String {
    if path == "/" {
        return String::from("/");
    }
    if path == "/root" {
        return String::from("/root");
    }
    path.strip_prefix("/root/")
        .map(|rest| format!("/{rest}"))
        .unwrap_or_else(|| path.to_string())
}

fn project_bucket_key(project: &ProjectBucket) -> String {
    format!("{}::{}", project.machine_name, project.cwd)
}

fn project_set_contains(set: &HashSet<String>, project: &ProjectBucket) -> bool {
    set.contains(&project_bucket_key(project)) || set.contains(&project.cwd)
}

fn pinned_set_contains(set: &HashSet<String>, project: &ProjectBucket) -> bool {
    project_set_contains(set, project)
}

fn is_user_only_session(session: &SessionSummary) -> bool {
    session.user_message_count > 0 && session.assistant_message_count == 0
}

fn render_preview(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &mut App) {
    let preview_session = app.current_preview_session();
    let preview = if let Some(session) = preview_session.clone() {
        let inner_width = area.width.saturating_sub(2) as usize;
        match app.preview_for_session(&session, app.preview_mode, inner_width) {
            Ok(preview) => preview,
            Err(err) => Arc::new(PreviewData {
                lines: vec![Line::from(format!("Preview error: {err:#}"))],
                tone_rows: Vec::new(),
                header_rows: Vec::new(),
                block_ranges: Vec::new(),
            }),
        }
    } else {
        Arc::new(PreviewData {
            lines: vec![Line::from("No session selected")],
            tone_rows: Vec::new(),
            header_rows: Vec::new(),
            block_ranges: Vec::new(),
        })
    };
    let session_title = preview_session
        .as_ref()
        .map(|s| {
            let warning = if is_user_only_session(s) {
                "  [user-only; may not resume in codex]"
            } else {
                ""
            };
            format!(
                "{}  [{}]  {}  user={} assistant={}{}",
                s.id,
                s.machine_name,
                format_human_timestamp(&s.started_at),
                s.user_message_count,
                s.assistant_message_count,
                warning
            )
        })
        .unwrap_or_else(|| String::from("No session selected"));
    app.preview_content_len = preview.lines.len();
    let viewport_len = area.height.saturating_sub(2) as usize;
    let max_scroll = app.preview_content_len.saturating_sub(viewport_len);
    let session_changed =
        app.preview_session_path.as_ref() != preview_session.as_ref().map(|s| &s.path);
    let content_len_changed = app.preview_rendered_lines.len() != preview.lines.len();
    if session_changed {
        app.preview_scroll = default_preview_scroll(app.preview_content_len, viewport_len);
        app.preview_focus_turn = preview.header_rows.last().map(|(_, turn_idx)| *turn_idx);
    } else {
        app.preview_scroll = app.preview_scroll.min(max_scroll);
    }
    let search_matches = if app.search_query.trim().is_empty() {
        Vec::new()
    } else {
        preview_match_positions(&preview, &app.search_query)
    };
    let search_changed = search_matches != app.preview_search_matches;
    app.preview_search_matches = search_matches;
    if app.preview_search_matches.is_empty() {
        app.preview_search_index = None;
    } else if session_changed || search_changed || app.preview_search_index.is_none() {
        app.preview_search_index = Some(0);
    }
    if let Some((path, query)) = app.pending_preview_search_jump.clone()
        && preview_session
            .as_ref()
            .is_some_and(|session| session.path == path)
    {
        if let Some(found) = preview_match_positions(&preview, &query).first().cloned() {
            app.preview_scroll = found.row.saturating_sub(viewport_len / 3);
            app.preview_focus_turn = preview_turn_at_or_before_row(&preview.header_rows, found.row);
            app.preview_search_index = Some(0);
        }
        app.pending_preview_search_jump = None;
    }
    if session_changed
        || content_len_changed
        || app.preview_selection.is_some()
        || app.preview_mouse_down_pos.is_some()
    {
        app.preview_rendered_lines = preview.lines.iter().map(|l| l.to_string()).collect();
    }
    app.preview_header_rows = preview.header_rows.clone();
    app.preview_session_path = preview_session.as_ref().map(|s| s.path.clone());
    if let Some(session) = preview_session.as_ref()
        && is_user_only_session(session)
    {
        app.status = String::from(
            "Selected session has user messages but no assistant reply; codex may not resume it",
        );
    }
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
        .title(format!("Preview ({mode_name}) {session_title}"))
        .borders(Borders::ALL)
        .border_style(focus_style);
    let (visible_start, visible_end) =
        preview_window_bounds(app.preview_content_len, app.preview_scroll, viewport_len);
    let para = Paragraph::new(preview.lines[visible_start..visible_end].to_vec()).block(block);
    frame.render_widget(para, area);

    let inner_x = area.x.saturating_add(1);
    let inner_y = area.y.saturating_add(1);
    let inner_w = area.width.saturating_sub(2);
    let inner_h = area.height.saturating_sub(2) as usize;
    let scroll = app.preview_scroll;
    for &(row, tone) in &preview.tone_rows {
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
    if !app.search_query.trim().is_empty() {
        for (match_idx, found) in app.preview_search_matches.iter().enumerate() {
            let row = found.row;
            if row < visible_start || row >= visible_end {
                continue;
            }
            let screen_y = inner_y + (row - scroll) as u16;
            let x = inner_x.saturating_add(found.col_start as u16);
            let width = (found.col_end.saturating_sub(found.col_start)) as u16;
            let max_w = inner_w.saturating_sub(found.col_start as u16);
            if width == 0 || max_w == 0 {
                continue;
            }
            let style = if Some(match_idx) == app.preview_search_index || found.is_primary {
                Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            };
            frame.buffer_mut().set_style(
                ratatui::layout::Rect {
                    x,
                    y: screen_y,
                    width: width.min(max_w),
                    height: 1,
                },
                style,
            );
        }
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

fn default_preview_scroll(content_len: usize, viewport_len: usize) -> usize {
    content_len.saturating_sub(viewport_len)
}

fn preview_window_bounds(content_len: usize, scroll: usize, viewport_len: usize) -> (usize, usize) {
    if viewport_len == 0 || content_len == 0 {
        return (0, 0);
    }
    let start = scroll.min(content_len);
    let end = (start + viewport_len).min(content_len);
    (start, end)
}

#[cfg(test)]
fn preview_match_row(preview: &PreviewData, query: &str) -> Option<usize> {
    preview_match_positions(preview, query)
        .first()
        .map(|found| found.row)
}

fn preview_match_positions(preview: &PreviewData, query: &str) -> Vec<PreviewMatch> {
    let tokens = search_tokens(query);
    if tokens.is_empty() {
        return Vec::new();
    }

    let mut matches = Vec::new();
    for (row, line) in preview.lines.iter().enumerate() {
        let line_text = line.to_string();
        let ranges = highlight_ranges(&line_text, query);
        for (col_start, col_end) in ranges {
            matches.push(PreviewMatch {
                row,
                col_start,
                col_end,
                is_primary: false,
            });
        }
    }
    if let Some(first) = matches.first_mut() {
        first.is_primary = true;
    }
    matches
}

fn preview_turn_at_or_before_row(header_rows: &[(usize, usize)], row: usize) -> Option<usize> {
    header_rows
        .iter()
        .filter(|(header_row, _)| *header_row <= row)
        .max_by_key(|(header_row, _)| *header_row)
        .map(|(_, turn_idx)| *turn_idx)
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
        | StatusButton::Export
        | StatusButton::ProjectRename
        | StatusButton::ProjectCopy
        | StatusButton::AddRemote => Style::default().fg(Color::Green),
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
    } else if app.search_focused {
        Line::from(vec![
            Span::styled("type", Style::default().fg(Color::Cyan)),
            Span::raw(" text/path/hash  "),
            Span::styled("enter", Style::default().fg(Color::Green)),
            Span::raw(" keep results  "),
            Span::styled("esc", Style::default().fg(Color::Red)),
            Span::raw(" close search  "),
            Span::styled("tab", Style::default().fg(Color::Cyan)),
            Span::raw(" next pane  "),
            Span::styled("shift+tab", Style::default().fg(Color::Cyan)),
            Span::raw(" prev pane"),
        ])
    } else if app.focus == Focus::Preview && app.mode == Mode::Normal {
        Line::from(vec![
            Span::styled("esc", Style::default().fg(Color::Red)),
            Span::raw(" browser  "),
            Span::styled("↑/↓", Style::default().fg(Color::Cyan)),
            Span::raw(" block prev/next  "),
            Span::styled("pgup/pgdn", Style::default().fg(Color::Cyan)),
            Span::raw(" page  "),
            Span::styled("home/end", Style::default().fg(Color::Cyan)),
            Span::raw(" top/bottom  "),
            Span::styled("←/→", Style::default().fg(Color::Cyan)),
            Span::raw(" fold/unfold block  "),
            Span::styled("n/N", Style::default().fg(Color::Cyan)),
            Span::raw(" next/prev match  "),
            Span::styled("tab", Style::default().fg(Color::Cyan)),
            Span::raw(" toggle block  "),
            Span::styled("shift+tab", Style::default().fg(Color::Cyan)),
            Span::raw(" toggle all blocks  "),
            Span::styled("o", Style::default().fg(Color::Green)),
            Span::raw(" open in codex  "),
            Span::styled("drag", Style::default().fg(Color::Cyan)),
            Span::raw(" preview-select+copy  "),
            Span::styled("drag", Style::default().fg(Color::Cyan)),
            Span::raw(" splitter/scrollbar"),
        ])
    } else if app.focus == Focus::Projects
        && app.mode == Mode::Normal
        && matches!(
            app.browser_cursor,
            BrowserCursor::Project | BrowserCursor::Group
        )
    {
        Line::from(vec![
            Span::styled("j/k", Style::default().fg(Color::Cyan)),
            Span::raw(" folder nav  "),
            Span::styled("ctrl+↑/↓", Style::default().fg(Color::Cyan)),
            Span::raw(" project jump  "),
            Span::styled("←/→", Style::default().fg(Color::Cyan)),
            Span::raw(" collapse/expand  "),
            Span::styled("ctrl+←", Style::default().fg(Color::Cyan)),
            Span::raw(" collapse others  "),
            Span::styled("ctrl+→", Style::default().fg(Color::Cyan)),
            Span::raw(" expand all  "),
            Span::styled("ctrl+c", Style::default().fg(Color::Green)),
            Span::raw(" copy folder  "),
            Span::styled("ctrl+x", Style::default().fg(Color::Yellow)),
            Span::raw(" cut folder  "),
            Span::styled("ctrl+v", Style::default().fg(Color::Green)),
            Span::raw(" paste into folder  "),
            Span::styled("dblclick", Style::default().fg(Color::Cyan)),
            Span::raw(" toggle folder  "),
            Span::styled("m or r", Style::default().fg(Color::Green)),
            Span::raw(" rename folder sessions  "),
            Span::styled("c or y", Style::default().fg(Color::Green)),
            Span::raw(" copy folder sessions  "),
            Span::styled("R", Style::default().fg(Color::Green)),
            Span::raw(" connect remote  "),
            Span::styled("/", Style::default().fg(Color::Cyan)),
            Span::raw(" search  "),
            Span::styled("f5/ctrl+r", Style::default().fg(Color::Yellow)),
            Span::raw(" refresh  "),
            Span::styled("q", Style::default().fg(Color::Red)),
            Span::raw(" quit"),
        ])
    } else if app.focus == Focus::Projects
        && app.mode == Mode::Normal
        && app.browser_cursor == BrowserCursor::Session
    {
        Line::from(vec![
            Span::styled("j/k", Style::default().fg(Color::Cyan)),
            Span::raw(" nav  "),
            Span::styled("ctrl+↑/↓", Style::default().fg(Color::Cyan)),
            Span::raw(" project jump  "),
            Span::styled("←", Style::default().fg(Color::Cyan)),
            Span::raw(" folder row  "),
            Span::styled("→", Style::default().fg(Color::Cyan)),
            Span::raw(" open preview  "),
            Span::styled("space", Style::default().fg(Color::Yellow)),
            Span::raw(" toggle-select  "),
            Span::styled("checkbox click", Style::default().fg(Color::Yellow)),
            Span::raw(" toggle  "),
            Span::styled("a", Style::default().fg(Color::Yellow)),
            Span::raw(" select-all  "),
            Span::styled("i", Style::default().fg(Color::Yellow)),
            Span::raw(" invert  "),
            Span::styled("ctrl+c/x/v", Style::default().fg(Color::Green)),
            Span::raw(" copy/cut/paste  "),
            Span::styled("del", Style::default().fg(Color::Red)),
            Span::raw(" delete  "),
            Span::styled("dblclick", Style::default().fg(Color::Cyan)),
            Span::raw(" open  "),
            Span::styled("m/c/f/d", Style::default().fg(Color::Green)),
            Span::raw(" move/copy/fork/delete selection  "),
            Span::styled("target", Style::default().fg(Color::Cyan)),
            Span::raw(" /path or machine:/path  "),
            Span::styled("e", Style::default().fg(Color::Green)),
            Span::raw(" export ssh  "),
            Span::styled("/", Style::default().fg(Color::Cyan)),
            Span::raw(" search  "),
            Span::styled("f5/ctrl+r", Style::default().fg(Color::Yellow)),
            Span::raw(" refresh"),
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
            Span::styled("R", Style::default().fg(Color::Green)),
            Span::raw(" connect remote  "),
            Span::styled("e", Style::default().fg(Color::Green)),
            Span::raw(" export ssh  "),
            Span::styled("g/f5/ctrl+r", Style::default().fg(Color::Yellow)),
            Span::raw(" refresh  "),
            Span::styled("q", Style::default().fg(Color::Red)),
            Span::raw(" quit"),
        ])
    };
    let matched_sessions = app
        .projects
        .iter()
        .map(|project| project.sessions.len())
        .sum::<usize>();
    let search_meta = if app.search_query.trim().is_empty() {
        String::from("search: <none>")
    } else {
        format!(
            "search: '{}' ({} sessions, {} projects, {} focus)",
            app.search_query,
            matched_sessions,
            app.projects.len(),
            if app.search_focused { "active" } else { "kept" }
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
            Some(Action::Export) => "EXPORT",
            Some(Action::Delete) => "DELETE",
            Some(Action::ProjectRename) => "RENAME FOLDER",
            Some(Action::ProjectCopy) => "COPY FOLDER",
            Some(Action::AddRemote) => "CONNECT REMOTE",
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

    let turns = coalesce_chat_turns(&cached.turns);

    if turns.is_empty() {
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

    let assistant_count = turns.iter().filter(|t| t.role == "assistant").count();
    if assistant_count == 0 {
        lines.push(Line::from(Span::styled(
            "Warning: no assistant messages detected in this session.",
            Style::default().fg(Color::Yellow),
        )));
    }
    lines.push(Line::from(String::new()));

    for (turn_idx, turn) in turns.iter().enumerate() {
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
            Span::styled(
                format_human_timestamp(&turn.timestamp),
                Style::default().fg(Color::DarkGray),
            ),
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
        if turn_idx + 1 < turns.len() {
            if tone == BlockTone::User {
                // Ensure a terminal-bg hairline gap between USER blocks.
                lines.push(Line::from(Span::styled(
                    "─".repeat(inner_width.saturating_sub(1).max(1)),
                    Style::default().fg(Color::DarkGray),
                )));
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

fn search_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in query.chars() {
        match ch {
            '"' => {
                if in_quotes {
                    let token = current.trim().to_lowercase();
                    if !token.is_empty() {
                        tokens.push(token);
                    }
                    current.clear();
                    in_quotes = false;
                } else {
                    let token = current.trim().to_lowercase();
                    if !token.is_empty() {
                        tokens.push(token);
                    }
                    current.clear();
                    in_quotes = true;
                }
            }
            c if c.is_whitespace() && !in_quotes => {
                let token = current.trim().to_lowercase();
                if !token.is_empty() {
                    tokens.push(token);
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    let token = current.trim().to_lowercase();
    if !token.is_empty() {
        tokens.push(token);
    }

    tokens
}

fn search_score(
    query: &str,
    search_blob: &str,
    cwd: &str,
    file_name: &str,
    session_id: &str,
) -> Option<i64> {
    let tokens = search_tokens(query);
    if tokens.is_empty() {
        return Some(0);
    }

    let mut total = 0i64;
    let search_blob_l = search_blob.to_lowercase();
    let cwd_l = cwd.to_lowercase();
    let file_name_l = file_name.to_lowercase();
    let session_id_l = session_id.to_lowercase();
    let haystacks = [
        (search_blob_l.as_str(), 120i64),
        (cwd_l.as_str(), 90i64),
        (session_id_l.as_str(), 80i64),
        (file_name_l.as_str(), 70i64),
    ];

    for token in &tokens {
        let mut best = None;
        for (hay, weight) in &haystacks {
            if let Some(score) = literal_search_score(token, hay, *weight) {
                best = Some(best.unwrap_or(i64::MIN).max(score));
            }
        }
        let Some(best) = best else {
            return None;
        };
        total += best;
    }

    let query_l = query.to_lowercase();
    if search_blob_l.contains(&query_l) {
        total += 40;
    } else if cwd_l.contains(&query_l) {
        total += 30;
    } else if session_id_l.contains(&query_l) || file_name_l.contains(&query_l) {
        total += 25;
    }
    Some(total)
}

fn literal_search_score(token: &str, haystack: &str, weight: i64) -> Option<i64> {
    let pos = haystack.find(token)? as i64;
    let mut score = weight;
    score += (40 - pos.min(40)).max(0);
    if pos == 0 {
        score += 25;
    }
    if haystack == token {
        score += 30;
    }
    if haystack
        .split(|c: char| !c.is_alphanumeric())
        .any(|part| part == token)
    {
        score += 20;
    }
    Some(score)
}

fn highlight_spans(text: &str, query: &str) -> Vec<Span<'static>> {
    let ranges = highlight_ranges(text, query);
    if ranges.is_empty() {
        return vec![Span::raw(text.to_string())];
    }

    let chars = text.chars().collect::<Vec<_>>();
    let mut spans = Vec::new();
    let mut cursor = 0usize;
    for (start, end) in ranges {
        if cursor < start {
            spans.push(Span::raw(chars[cursor..start].iter().collect::<String>()));
        }
        spans.push(Span::styled(
            chars[start..end].iter().collect::<String>(),
            Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        ));
        cursor = end;
    }
    if cursor < chars.len() {
        spans.push(Span::raw(chars[cursor..].iter().collect::<String>()));
    }
    spans
}

fn highlight_ranges(text: &str, query: &str) -> Vec<(usize, usize)> {
    let tokens = search_tokens(query);
    if tokens.is_empty() {
        return Vec::new();
    }

    let lower = text.to_lowercase();
    let mut byte_ranges = Vec::<(usize, usize)>::new();
    for token in tokens {
        let mut start_at = 0usize;
        while let Some(rel) = lower[start_at..].find(&token) {
            let start = start_at + rel;
            let end = start + token.len();
            byte_ranges.push((start, end));
            start_at = end;
            if start_at >= lower.len() {
                break;
            }
        }
    }
    if byte_ranges.is_empty() {
        return Vec::new();
    }
    byte_ranges.sort_unstable();
    let mut merged = Vec::<(usize, usize)>::new();
    for (start, end) in byte_ranges {
        if let Some(last) = merged.last_mut()
            && start <= last.1
        {
            last.1 = last.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    merged
        .into_iter()
        .map(|(start_b, end_b)| {
            (
                text[..start_b].chars().count(),
                text[..end_b].chars().count(),
            )
        })
        .collect()
}

fn prepend_style(spans: Vec<Span<'static>>, base: Style) -> Vec<Span<'static>> {
    spans
        .into_iter()
        .map(|span| {
            let style = base.patch(span.style);
            Span::styled(span.content.into_owned(), style)
        })
        .collect()
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

#[derive(Default)]
struct BrowserTreeNode {
    name: String,
    full_path: String,
    project_idx: Option<usize>,
    children: BTreeMap<String, BrowserTreeNode>,
}

fn build_browser_rows(
    projects: &[ProjectBucket],
    machine_roots: &[String],
    collapsed_groups: &HashSet<String>,
    collapsed_projects: &HashSet<String>,
    _selected_sessions: &HashSet<PathBuf>,
) -> Vec<BrowserRow> {
    let tree = build_browser_tree(projects, machine_roots);
    let mut rows = Vec::new();
    for node in tree.values() {
        append_browser_rows(
            node,
            projects,
            collapsed_groups,
            collapsed_projects,
            &mut rows,
            0,
        );
    }
    rows
}

fn build_browser_tree(
    projects: &[ProjectBucket],
    machine_roots: &[String],
) -> BTreeMap<String, BrowserTreeNode> {
    let mut roots = BTreeMap::<String, BrowserTreeNode>::new();
    for root_name in machine_roots {
        roots.entry(root_name.clone()).or_insert_with(|| BrowserTreeNode {
            name: root_name.clone(),
            full_path: root_name.clone(),
            ..BrowserTreeNode::default()
        });
    }
    for (project_idx, project) in projects.iter().enumerate() {
        let segments = browser_tree_segments_for_project(project);
        let Some((root_name, rest)) = segments.split_first() else {
            continue;
        };
        let root = roots
            .entry(root_name.clone())
            .or_insert_with(|| BrowserTreeNode {
                name: root_name.clone(),
                full_path: root_name.clone(),
                ..BrowserTreeNode::default()
            });
        insert_browser_tree_path(root, rest, project_idx);
    }
    compress_browser_tree_children(&mut roots);
    roots
}

fn insert_browser_tree_path(node: &mut BrowserTreeNode, segments: &[String], project_idx: usize) {
    if segments.is_empty() {
        node.project_idx = Some(project_idx);
        return;
    }
    let name = &segments[0];
    let child_path = if node.full_path == "/" {
        format!("/{name}")
    } else if name == "/" {
        format!("{}/", node.full_path)
    } else {
        format!("{}/{}", node.full_path, name)
    };
    let child = node
        .children
        .entry(name.clone())
        .or_insert_with(|| BrowserTreeNode {
            name: name.clone(),
            full_path: child_path,
            ..BrowserTreeNode::default()
        });
    insert_browser_tree_path(child, &segments[1..], project_idx);
}

fn compress_browser_tree_children(nodes: &mut BTreeMap<String, BrowserTreeNode>) {
    let keys = nodes.keys().cloned().collect::<Vec<_>>();
    for key in keys {
        if let Some(node) = nodes.get_mut(&key) {
            compress_browser_tree_node(node, false);
        }
    }
}

fn compress_browser_tree_node(node: &mut BrowserTreeNode, can_compress_self: bool) {
    let child_keys = node.children.keys().cloned().collect::<Vec<_>>();
    for key in child_keys {
        if let Some(child) = node.children.get_mut(&key) {
            compress_browser_tree_node(child, true);
        }
    }
    if !can_compress_self {
        return;
    }
    while node.project_idx.is_none() && node.children.len() == 1 {
        let (_, child) = node.children.pop_first().expect("child exists");
        node.name = format!("{}/{}", node.name, child.name);
        node.full_path = child.full_path;
        node.project_idx = child.project_idx;
        node.children = child.children;
    }
}

fn append_browser_rows(
    node: &BrowserTreeNode,
    projects: &[ProjectBucket],
    collapsed_groups: &HashSet<String>,
    collapsed_projects: &HashSet<String>,
    rows: &mut Vec<BrowserRow>,
    depth: usize,
) {
    let group_only = node.project_idx.is_none();
    rows.push(BrowserRow {
        kind: if let Some(project_idx) = node.project_idx {
            BrowserRowKind::Project { project_idx }
        } else {
            BrowserRowKind::Group {
                path: node.full_path.clone(),
            }
        },
        depth,
        label: node.name.clone(),
    });

    let collapsed = if group_only {
        collapsed_groups.contains(&node.full_path)
    } else {
        let project_idx = node.project_idx.expect("project idx");
        project_set_contains(collapsed_projects, &projects[project_idx])
    };
    if collapsed {
        return;
    }

    if let Some(project_idx) = node.project_idx {
        for session_idx in 0..projects[project_idx].sessions.len() {
            rows.push(BrowserRow {
                kind: BrowserRowKind::Session {
                    project_idx,
                    session_idx,
                },
                depth: depth + 1,
                label: format_session_browser_line(&projects[project_idx].sessions[session_idx]),
            });
        }
    }
    for child in node.children.values() {
        append_browser_rows(
            child,
            projects,
            collapsed_groups,
            collapsed_projects,
            rows,
            depth + 1,
        );
    }
}

fn browser_tree_segments(cwd: &str) -> Vec<String> {
    let normalized = if cwd == "/" {
        String::from("/")
    } else if cwd.starts_with('/') {
        format!("/{}", cwd.trim_start_matches('/'))
    } else {
        cwd.to_string()
    };
    let cwd = normalized.as_str();

    if cwd == "/" {
        return vec![String::from("/")];
    }
    if cwd == "/root" {
        return vec![String::from("/root")];
    }
    if let Some(rest) = cwd.strip_prefix("/root/") {
        let mut parts = vec![String::from("/root")];
        parts.extend(rest.split('/').map(|s| s.to_string()));
        return parts;
    }
    let mut parts = vec![String::from("/")];
    parts.extend(
        cwd.trim_start_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
    );
    parts
}

fn browser_tree_segments_for_project(project: &ProjectBucket) -> Vec<String> {
    let mut parts = vec![project.machine_name.clone()];
    parts.extend(browser_tree_segments(&project.cwd));
    parts
}

fn default_collapsed_group_paths(projects: &[ProjectBucket]) -> HashSet<String> {
    let mut collapsed = HashSet::new();
    let mut machine_roots = vec![String::from("local")];
    machine_roots.extend(projects.iter().map(|p| p.machine_name.clone()));
    machine_roots.sort();
    machine_roots.dedup();
    for node in build_browser_tree(projects, &machine_roots).values() {
        collect_group_paths(node, &mut collapsed);
    }
    collapsed
}

fn collect_group_paths(node: &BrowserTreeNode, out: &mut HashSet<String>) {
    if node.project_idx.is_none() {
        out.insert(node.full_path.clone());
    }
    for child in node.children.values() {
        collect_group_paths(child, out);
    }
}

fn first_project_index_for_group(projects: &[ProjectBucket], group_path: &str) -> Option<usize> {
    let prefix = if group_path == "/" {
        String::from("/")
    } else {
        format!("{group_path}/")
    };
    projects.iter().position(|project| {
        let display = browser_display_path(&project.cwd);
        display == group_path || display.starts_with(&prefix)
    })
}

fn expand_group_ancestors_for_project(
    projects: &[ProjectBucket],
    collapsed_groups: &mut HashSet<String>,
    cwd: &str,
) {
    let Some(project) = projects.iter().find(|project| project.cwd == cwd) else {
        return;
    };
    let segments = browser_tree_segments_for_project(project);
    if segments.is_empty() {
        return;
    }
    let mut current = String::new();
    for (idx, segment) in segments.iter().enumerate() {
        if idx == 0 {
            current = segment.clone();
        } else if current == "/" {
            current = format!("/{segment}");
        } else {
            current = format!("{current}/{segment}");
        }
        collapsed_groups.remove(&current);
    }
    let _ = projects;
}

#[cfg(test)]
fn project_label(projects: &[ProjectBucket], idx: usize) -> String {
    let cwd = projects
        .get(idx)
        .map(|p| p.cwd.as_str())
        .unwrap_or("<unknown>");
    let display = browser_display_path(cwd);
    if display == "/" || display == "/root" {
        return display;
    }
    let cwd = display.as_str();
    let common = shared_path_prefix(projects);
    let parts = path_components(cwd);
    let base_len = nearest_project_ancestor_len(projects, idx, &parts).unwrap_or(common.len());
    let rel = &parts[base_len.min(parts.len())..];
    if rel.is_empty() {
        cwd.to_string()
    } else {
        rel.join("/")
    }
}

#[cfg(test)]
fn project_indent(projects: &[ProjectBucket], idx: usize) -> String {
    let cwd = projects
        .get(idx)
        .map(|p| p.cwd.as_str())
        .unwrap_or("<unknown>");
    let display = browser_display_path(cwd);
    let cwd = display.as_str();
    let common = shared_path_prefix(projects);
    let parts = path_components(cwd);
    let rel_depth = project_ancestor_depth(projects, idx, &parts, common.len()).min(6);
    "  ".repeat(rel_depth)
}

#[cfg(test)]
fn nearest_project_ancestor_len(
    projects: &[ProjectBucket],
    idx: usize,
    parts: &[String],
) -> Option<usize> {
    for candidate_len in (1..parts.len()).rev() {
        if project_exists_with_parts(projects, idx, &parts[..candidate_len]) {
            return Some(candidate_len);
        }
    }
    None
}

#[cfg(test)]
fn project_ancestor_depth(
    projects: &[ProjectBucket],
    idx: usize,
    parts: &[String],
    shared_len: usize,
) -> usize {
    ((shared_len + 1)..parts.len())
        .filter(|candidate_len| project_exists_with_parts(projects, idx, &parts[..*candidate_len]))
        .count()
}

#[cfg(test)]
fn project_exists_with_parts(
    projects: &[ProjectBucket],
    skip_idx: usize,
    wanted_parts: &[String],
) -> bool {
    projects.iter().enumerate().any(|(candidate_idx, project)| {
        candidate_idx != skip_idx
            && path_components(&browser_display_path(&project.cwd)) == wanted_parts
    })
}

#[cfg(test)]
fn shared_path_prefix(projects: &[ProjectBucket]) -> Vec<String> {
    let mut iter = projects.iter();
    let Some(first) = iter.next() else {
        return Vec::new();
    };
    let mut prefix = path_components(&browser_display_path(&first.cwd));
    for project in iter {
        let parts = path_components(&browser_display_path(&project.cwd));
        let keep = prefix
            .iter()
            .zip(parts.iter())
            .take_while(|(a, b)| a == b)
            .count();
        prefix.truncate(keep);
        if prefix.is_empty() {
            break;
        }
    }
    prefix
}

#[cfg(test)]
fn path_components(path: &str) -> Vec<String> {
    Path::new(path)
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(v) => Some(v.to_string_lossy().to_string()),
            _ => None,
        })
        .collect()
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

fn coalesce_chat_turns(turns: &[ChatTurn]) -> Vec<ChatTurn> {
    let mut out: Vec<ChatTurn> = Vec::new();
    for turn in turns {
        if let Some(last) = out.last_mut()
            && last.role == turn.role
        {
            if !last.text.is_empty() && !turn.text.is_empty() {
                last.text.push_str("\n\n");
            }
            last.text.push_str(&turn.text);
            last.timestamp = turn.timestamp.clone();
            continue;
        }
        out.push(turn.clone());
    }
    out
}

fn default_folded_turns(turns: &[ChatTurn]) -> HashSet<usize> {
    let mut folded = HashSet::new();
    for (idx, turn) in turns.iter().enumerate() {
        let should_fold = turn.role == "assistant" || (turn.role == "user" && idx == 0);
        if should_fold {
            folded.insert(idx);
        }
    }
    folded
}

fn format_human_timestamp(raw: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|dt| {
            dt.with_timezone(&Utc)
                .format("%B %-d, %Y %-I:%M%p")
                .to_string()
        })
        .unwrap_or_else(|_| raw.to_string())
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

#[cfg(test)]
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

#[allow(dead_code)]
fn scan_sessions(root: &Path, config: &AppConfig) -> Result<Vec<ProjectBucket>> {
    let mut all_projects = scan_local_sessions(root)?;
    for machine in &config.machines {
        match scan_remote_sessions(machine) {
            Ok(mut projects) => all_projects.append(&mut projects),
            Err(err) => {
                eprintln!("remote scan failed for {}: {err:#}", machine.name);
            }
        }
    }
    all_projects.sort_by(|a, b| {
        a.machine_name
            .cmp(&b.machine_name)
            .then_with(|| a.cwd.cmp(&b.cwd))
    });
    Ok(all_projects)
}

fn scan_local_sessions(root: &Path) -> Result<Vec<ProjectBucket>> {
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    collect_jsonl_files(root, &mut files)?;

    let mut projects: HashMap<String, Vec<SessionSummary>> = HashMap::new();
    for path in files {
        if let Ok(summary) = parse_local_session_summary(&path) {
            projects
                .entry(summary.cwd.clone())
                .or_default()
                .push(summary);
        }
    }

    let mut sorted_projects = BTreeMap::new();
    for (cwd, mut sessions) in projects {
        sessions.sort_by(|a, b| {
            b.modified_epoch
                .cmp(&a.modified_epoch)
                .then_with(|| b.started_at.cmp(&a.started_at))
        });
        sorted_projects.insert(cwd, sessions);
    }

    Ok(sorted_projects
        .into_iter()
        .map(|(cwd, sessions)| ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd,
            sessions,
        })
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

fn parse_local_session_summary(path: &Path) -> Result<SessionSummary> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let metadata =
        fs::metadata(path).with_context(|| format!("failed metadata {}", path.display()))?;
    let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let modified_dt: DateTime<Utc> = modified.into();

    let mut session_id = String::from("unknown");
    let mut cwd = String::from("<unknown>");
    let mut started_at = String::from("unknown");
    let mut event_count = 0usize;
    let mut user_message_count = 0usize;
    let mut assistant_message_count = 0usize;
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
                    match payload.get("role").and_then(Value::as_str) {
                        Some("user") | Some("developer") => user_message_count += 1,
                        Some("assistant") => assistant_message_count += 1,
                        _ => {}
                    }
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
        storage_path: path_to_string(path),
        file_name,
        id: session_id,
        cwd,
        machine_name: String::from("local"),
        machine_target: None,
        machine_codex_home: None,
        machine_exec_prefix: None,
        started_at,
        modified_epoch: modified_dt.timestamp(),
        event_count,
        user_message_count,
        assistant_message_count,
        search_blob: search_parts.join("\n"),
    })
}

fn parse_remote_session_summary_line(
    machine: &ConfigMachine,
    line: &str,
) -> Result<SessionSummary> {
    let value: Value = serde_json::from_str(line).context("invalid remote summary line")?;
    let storage_path = value
        .get("rollout_path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("remote summary missing rollout_path"))?;
    let file_name = value
        .get("file_name")
        .and_then(Value::as_str)
        .unwrap_or("rollout.jsonl");
    let id = value.get("id").and_then(Value::as_str).unwrap_or("unknown");
    let cwd = value
        .get("cwd")
        .and_then(Value::as_str)
        .unwrap_or("<unknown>");
    let started_at = value
        .get("started_at")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let modified_epoch = value
        .get("modified_epoch")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let event_count = value
        .get("event_count")
        .and_then(Value::as_u64)
        .unwrap_or_default() as usize;
    let user_message_count = value
        .get("user_message_count")
        .and_then(Value::as_u64)
        .unwrap_or_default() as usize;
    let assistant_message_count = value
        .get("assistant_message_count")
        .and_then(Value::as_u64)
        .unwrap_or_default() as usize;
    let search_blob = value
        .get("search_blob")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Ok(SessionSummary {
        path: PathBuf::from(format!("ssh://{}/{}", machine.name, storage_path)),
        storage_path: storage_path.to_string(),
        file_name: file_name.to_string(),
        id: id.to_string(),
        cwd: cwd.to_string(),
        machine_name: machine.name.clone(),
        machine_target: Some(machine.ssh_target.clone()),
        machine_codex_home: machine.codex_home.clone(),
        machine_exec_prefix: machine.exec_prefix.clone(),
        started_at: started_at.to_string(),
        modified_epoch,
        event_count,
        user_message_count,
        assistant_message_count,
        search_blob: search_blob.to_string(),
    })
}

fn scan_remote_sessions(machine: &ConfigMachine) -> Result<Vec<ProjectBucket>> {
    let lines = run_remote_python_lines(
        &machine.ssh_target,
        machine.exec_prefix.as_deref(),
        REMOTE_SCAN_SCRIPT,
        &[machine
            .codex_home
            .clone()
            .unwrap_or_else(|| String::from("~/.codex"))],
        true,
    )?;

    let mut projects: HashMap<String, Vec<SessionSummary>> = HashMap::new();
    for line in lines {
        let summary = parse_remote_session_summary_line(machine, &line)?;
        projects
            .entry(summary.cwd.clone())
            .or_default()
            .push(summary);
    }

    let mut sorted_projects = BTreeMap::new();
    for (cwd, mut sessions) in projects {
        sessions.sort_by(|a, b| {
            b.modified_epoch
                .cmp(&a.modified_epoch)
                .then_with(|| b.started_at.cmp(&a.started_at))
        });
        sorted_projects.insert(cwd, sessions);
    }

    Ok(sorted_projects
        .into_iter()
        .map(|(cwd, sessions)| ProjectBucket {
            machine_name: machine.name.clone(),
            machine_target: Some(machine.ssh_target.clone()),
            machine_codex_home: machine.codex_home.clone(),
            machine_exec_prefix: machine.exec_prefix.clone(),
            cwd,
            sessions,
        })
        .collect())
}

fn rewrite_session_file(path: &Path, target_cwd: &str, rewrite_id: bool) -> Result<()> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;

    let new_id = if rewrite_id {
        Some(Uuid::new_v4().to_string())
    } else {
        None
    };

    let out = rewrite_session_content(
        &content,
        target_cwd,
        new_id.as_deref(),
        false,
        path.display().to_string().as_str(),
    )?;

    backup_file(path)?;
    atomic_write(path, &out)?;
    Ok(())
}

#[allow(dead_code)]
fn rewrite_session_file_content_local(path: &Path, out: &str) -> Result<()> {
    backup_file(path)?;
    atomic_write(path, out)
}

fn repair_session_file_cwds(path: &Path, cwd_base: &Path) -> Result<bool> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let repaired = rewrite_session_content_with_normalized_cwds(&content, cwd_base)?;
    if repaired == content {
        return Ok(false);
    }

    backup_file(path)?;
    atomic_write(path, &repaired)?;
    Ok(true)
}

#[allow(dead_code)]
fn duplicate_session_file(
    sessions_root: &Path,
    source: &SessionSummary,
    target_cwd: &str,
    fork: bool,
) -> Result<PathBuf> {
    let content = read_session_content(source)?;

    let new_id = if fork {
        Some(Uuid::new_v4().to_string())
    } else {
        None
    };

    let out = rewrite_session_content(
        &content,
        target_cwd,
        new_id.as_deref(),
        fork,
        source.path.display().to_string().as_str(),
    )?;

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

fn duplicate_session_content(
    source: &SessionSummary,
    target_cwd: &str,
    fork: bool,
) -> Result<(String, String, bool)> {
    let content = read_session_content(source)?;
    let new_id = if fork {
        Some(Uuid::new_v4().to_string())
    } else {
        None
    };
    let out = rewrite_session_content(
        &content,
        target_cwd,
        new_id.as_deref(),
        fork,
        &source.storage_path,
    )?;
    Ok((out, new_id.unwrap_or_else(|| source.id.clone()), fork))
}

fn write_new_local_session(sessions_root: &Path, session_id: &str, out: &str) -> Result<PathBuf> {
    let now = Utc::now();
    let mut target_path = sessions_root
        .join(now.format("%Y").to_string())
        .join(now.format("%m").to_string())
        .join(now.format("%d").to_string());
    fs::create_dir_all(&target_path)
        .with_context(|| format!("failed to create {}", target_path.display()))?;
    target_path.push(format!(
        "rollout-{}-{}.jsonl",
        now.format("%Y-%m-%dT%H-%M-%S"),
        session_id
    ));
    let final_path = unique_path(target_path);
    atomic_write(&final_path, out)?;
    Ok(final_path)
}

fn rewrite_session_content(
    content: &str,
    target_cwd: &str,
    new_id: Option<&str>,
    rewrite_start_timestamp: bool,
    source_label: &str,
) -> Result<String> {
    let mut out = String::with_capacity(content.len() + 1024);
    for line in content.lines() {
        if line.trim().is_empty() {
            out.push('\n');
            continue;
        }

        let mut value: Value = serde_json::from_str(line)
            .with_context(|| format!("invalid JSON line in {source_label}"))?;

        rewrite_cwd_fields(&mut value, target_cwd);
        if let Some(id) = new_id {
            rewrite_session_id(&mut value, id);
            if rewrite_start_timestamp {
                rewrite_session_start_timestamp(&mut value);
            }
        }

        out.push_str(&serde_json::to_string(&value)?);
        out.push('\n');
    }
    Ok(out)
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

fn rewrite_cwd_fields_normalized(value: &mut Value, cwd_base: &Path) {
    match value {
        Value::Object(obj) => {
            for (key, val) in obj.iter_mut() {
                if key == "cwd" {
                    if let Some(cwd) = val.as_str()
                        && let Some(normalized) = normalize_local_cwd(cwd, cwd_base)
                        && normalized != cwd
                    {
                        *val = Value::String(normalized);
                    }
                } else {
                    rewrite_cwd_fields_normalized(val, cwd_base);
                }
            }
        }
        Value::Array(arr) => {
            for item in arr {
                rewrite_cwd_fields_normalized(item, cwd_base);
            }
        }
        _ => {}
    }
}

fn rewrite_session_content_with_normalized_cwds(content: &str, cwd_base: &Path) -> Result<String> {
    let mut out = String::with_capacity(content.len() + 64);
    for line in content.lines() {
        if line.trim().is_empty() {
            out.push('\n');
            continue;
        }

        let mut value: Value =
            serde_json::from_str(line).context("invalid JSON line while repairing cwd")?;
        rewrite_cwd_fields_normalized(&mut value, cwd_base);
        out.push_str(&serde_json::to_string(&value)?);
        out.push('\n');
    }
    Ok(out)
}

fn normalize_local_target_cwd(input: &str, cwd_base: &Path) -> Result<String> {
    let expanded = expand_tilde(input.trim());
    if expanded.as_os_str().is_empty() {
        return Err(anyhow!("Target path is empty"));
    }
    normalize_local_cwd_path(&expanded, cwd_base)
        .map(|path| path_to_string(&path))
        .ok_or_else(|| anyhow!("Target path is empty"))
}

fn normalize_local_cwd(input: &str, cwd_base: &Path) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let expanded = expand_tilde(trimmed);
    normalize_local_cwd_path(&expanded, cwd_base).map(|path| path_to_string(&path))
}

fn normalize_local_cwd_path(path: &Path, cwd_base: &Path) -> Option<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd_base.join(path)
    };

    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        use std::path::Component;
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(Path::new("/"));
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }

    if normalized.as_os_str().is_empty() {
        return Some(PathBuf::from("/"));
    }
    Some(normalized)
}

fn path_to_string(path: &Path) -> String {
    let s = path.to_string_lossy().to_string();
    if s.len() > 1 {
        s.trim_end_matches('/').to_string()
    } else {
        s
    }
}

fn repair_session_cwds(root: &Path, cwd_base: &Path) -> Result<usize> {
    if !root.exists() {
        return Ok(0);
    }

    let mut files = Vec::new();
    collect_jsonl_files(root, &mut files)?;
    let mut repaired = 0usize;
    for path in files {
        if repair_session_file_cwds(&path, cwd_base)? {
            repaired += 1;
        }
    }
    Ok(repaired)
}

fn resolve_state_db_path(codex_home: &Path) -> Option<PathBuf> {
    let mut candidates = fs::read_dir(codex_home)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|name| {
                    name.starts_with("state_")
                        && name.ends_with(".sqlite")
                        && !name.ends_with(".sqlite-shm")
                        && !name.ends_with(".sqlite-wal")
                })
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|path| state_db_sort_key(path));
    candidates.pop()
}

fn state_db_sort_key(path: &Path) -> i64 {
    path.file_stem()
        .and_then(|s| s.to_str())
        .and_then(|stem| stem.strip_prefix("state_"))
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(i64::MIN)
}

fn sync_threads_db_from_projects(db_path: &Path, projects: &[ProjectBucket]) -> Result<usize> {
    if !db_path.exists() {
        return Ok(0);
    }

    let conn = Connection::open(db_path)
        .with_context(|| format!("failed opening {}", db_path.display()))?;
    let tx = conn
        .unchecked_transaction()
        .with_context(|| format!("failed starting transaction on {}", db_path.display()))?;

    let mut synced = 0usize;
    for session in projects.iter().flat_map(|project| project.sessions.iter()) {
        if session.machine_target.is_some() {
            continue;
        }
        if sync_thread_record_tx(&tx, &session.id, &session.path, &session.cwd, &session.path)? {
            synced += 1;
        }
    }

    tx.commit()
        .with_context(|| format!("failed committing {}", db_path.display()))?;
    Ok(synced)
}

fn sync_thread_record(
    db_path: &Path,
    session_id: &str,
    rollout_path: &Path,
    target_cwd: &str,
    target_rollout_path: &Path,
) -> Result<bool> {
    if !db_path.exists() {
        return Ok(false);
    }

    let conn = Connection::open(db_path)
        .with_context(|| format!("failed opening {}", db_path.display()))?;
    let tx = conn
        .unchecked_transaction()
        .with_context(|| format!("failed starting transaction on {}", db_path.display()))?;
    let changed = sync_thread_record_tx(
        &tx,
        session_id,
        rollout_path,
        target_cwd,
        target_rollout_path,
    )?;
    tx.commit()
        .with_context(|| format!("failed committing {}", db_path.display()))?;
    Ok(changed)
}

fn sync_thread_record_tx(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    rollout_path: &Path,
    target_cwd: &str,
    target_rollout_path: &Path,
) -> Result<bool> {
    let rollout_path_s = path_to_string(rollout_path);
    let target_rollout_path_s = path_to_string(target_rollout_path);

    let mut stmt = tx.prepare(
        "SELECT id, cwd, rollout_path
         FROM threads
         WHERE id = ?1 OR rollout_path = ?2
         LIMIT 1",
    )?;
    let existing = stmt
        .query_row(params![session_id, rollout_path_s], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .optional()?;
    drop(stmt);

    let Some((row_id, current_cwd, current_rollout_path)) = existing else {
        return Ok(false);
    };

    if current_cwd == target_cwd && current_rollout_path == target_rollout_path_s {
        return Ok(false);
    }

    tx.execute(
        "UPDATE threads SET cwd = ?1, rollout_path = ?2 WHERE id = ?3",
        params![target_cwd, target_rollout_path_s, row_id],
    )?;
    Ok(true)
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

#[derive(Clone, PartialEq, Eq, Debug)]
struct RemoteExportTarget {
    ssh_target: String,
    remote_cwd: String,
}

const REMOTE_SCAN_SCRIPT: &str = r#"
import json, os, sys
from pathlib import Path

def summarize(path):
    session_id = "unknown"
    cwd = "<unknown>"
    started_at = "unknown"
    event_count = 0
    user_count = 0
    assistant_count = 0
    search_parts = []
    try:
        stat = path.stat()
        modified_epoch = int(stat.st_mtime)
        with path.open("r", encoding="utf-8", errors="replace") as fh:
            for raw in fh:
                raw = raw.strip()
                if not raw:
                    continue
                event_count += 1
                try:
                    value = json.loads(raw)
                except Exception:
                    continue
                ty = value.get("type")
                if ty == "session_meta":
                    payload = value.get("payload") or {}
                    session_id = payload.get("id") or session_id
                    cwd = payload.get("cwd") or cwd
                    started_at = payload.get("timestamp") or started_at
                elif ty == "response_item":
                    payload = value.get("payload") or {}
                    if payload.get("type") == "message":
                        role = payload.get("role")
                        if role in ("user", "developer"):
                            user_count += 1
                        elif role == "assistant":
                            assistant_count += 1
                        for item in payload.get("content") or []:
                            text = item.get("text") or item.get("input_text") or item.get("output_text")
                            if text:
                                search_parts.append(str(text).lower())
                elif ty == "event_msg":
                    payload = value.get("payload") or {}
                    if payload.get("type") == "user_message" and payload.get("message"):
                        search_parts.append(str(payload["message"]).lower())
    except Exception:
        return None
    return {
        "rollout_path": str(path),
        "file_name": path.name,
        "id": session_id,
        "cwd": cwd,
        "started_at": started_at,
        "modified_epoch": modified_epoch,
        "event_count": event_count,
        "user_message_count": user_count,
        "assistant_message_count": assistant_count,
        "search_blob": "\n".join(search_parts),
    }

codex_home = os.path.expanduser(sys.argv[1] if len(sys.argv) > 1 else "~/.codex")
root = Path(codex_home) / "sessions"
if root.exists():
    for path in root.rglob("*.jsonl"):
        data = summarize(path)
        if data:
            print(json.dumps(data, ensure_ascii=False))
"#;

const REMOTE_READ_FILE_SCRIPT: &str = r#"
import sys
with open(sys.argv[1], "r", encoding="utf-8", errors="replace") as fh:
    sys.stdout.write(fh.read())
"#;

fn parse_remote_export_target(input: &str) -> Result<RemoteExportTarget> {
    let trimmed = input.trim();
    let Some(colon_idx) = trimmed.rfind(':') else {
        return Err(anyhow!(
            "remote target must look like user@host:/remote/project/path"
        ));
    };
    let ssh_target = trimmed[..colon_idx].trim();
    let remote_cwd = trimmed[colon_idx + 1..].trim();
    if ssh_target.is_empty() || remote_cwd.is_empty() {
        return Err(anyhow!(
            "remote target must look like user@host:/remote/project/path"
        ));
    }
    Ok(RemoteExportTarget {
        ssh_target: ssh_target.to_string(),
        remote_cwd: remote_cwd.to_string(),
    })
}

fn sh_single_quote(input: &str) -> String {
    format!("'{}'", input.replace('\'', "'\"'\"'"))
}

fn remote_join_path(dir: &str, file_name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{file_name}")
    } else {
        format!("{dir}/{file_name}")
    }
}

fn remote_session_dir(codex_home: &str, now: DateTime<Utc>) -> String {
    format!(
        "{}/sessions/{}/{}/{}",
        codex_home.trim_end_matches('/'),
        now.format("%Y"),
        now.format("%m"),
        now.format("%d")
    )
}

fn remote_session_path(codex_home: &str, now: DateTime<Utc>, file_name: &str) -> String {
    remote_join_path(&remote_session_dir(codex_home, now), file_name)
}

fn add_ssh_options(cmd: &mut Command, batch_mode: bool) {
    cmd.arg("-o").arg("ConnectTimeout=5");
    if batch_mode {
        cmd.arg("-o").arg("BatchMode=yes");
    }
}

fn wrap_remote_exec(exec_prefix: Option<&str>, command: &str) -> String {
    match exec_prefix
        .map(str::trim)
        .filter(|prefix| !prefix.is_empty())
    {
        Some(prefix) => format!("{prefix} sh -lc {}", sh_single_quote(command)),
        None => command.to_string(),
    }
}

fn run_ssh_output(
    ssh_target: &str,
    exec_prefix: Option<&str>,
    script: &str,
    batch_mode: bool,
) -> Result<String> {
    let remote = wrap_remote_exec(exec_prefix, script);
    let mut cmd = Command::new("ssh");
    add_ssh_options(&mut cmd, batch_mode);
    let output = cmd
        .arg(ssh_target)
        .arg(remote)
        .output()
        .with_context(|| format!("failed to start ssh for {ssh_target}"))?;
    if !output.status.success() {
        return Err(anyhow!(
            "ssh command failed for {ssh_target} with status {}",
            output.status
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn run_ssh_status(
    ssh_target: &str,
    exec_prefix: Option<&str>,
    script: &str,
    batch_mode: bool,
) -> Result<()> {
    let remote = wrap_remote_exec(exec_prefix, script);
    let mut cmd = Command::new("ssh");
    add_ssh_options(&mut cmd, batch_mode);
    let status = cmd
        .arg(ssh_target)
        .arg(remote)
        .status()
        .with_context(|| format!("failed to start ssh for {ssh_target}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "ssh command failed for {ssh_target} with status {status}"
        ))
    }
}

fn run_remote_python_lines(
    ssh_target: &str,
    exec_prefix: Option<&str>,
    script: &str,
    args: &[String],
    batch_mode: bool,
) -> Result<Vec<String>> {
    let mut cmd = Command::new("ssh");
    add_ssh_options(&mut cmd, batch_mode);
    cmd.arg(ssh_target);
    if let Some(prefix) = exec_prefix
        .map(str::trim)
        .filter(|prefix| !prefix.is_empty())
    {
        let mut inner = String::from("python3 -");
        for arg in args {
            inner.push(' ');
            inner.push_str(&sh_single_quote(arg));
        }
        cmd.arg("sh")
            .arg("-lc")
            .arg(format!("{prefix} sh -lc {}", sh_single_quote(&inner)));
    } else {
        cmd.arg("python3").arg("-");
        for arg in args {
            cmd.arg(arg);
        }
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to start ssh python for {ssh_target}"))?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(script.as_bytes())
            .with_context(|| format!("failed to send script to {ssh_target}"))?;
    }
    let output = child
        .wait_with_output()
        .with_context(|| format!("failed waiting for ssh python on {ssh_target}"))?;
    if !output.status.success() {
        return Err(anyhow!(
            "remote command failed for {}: {}",
            ssh_target,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect())
}

fn run_remote_python_text(
    ssh_target: &str,
    exec_prefix: Option<&str>,
    script: &str,
    args: &[String],
    batch_mode: bool,
) -> Result<String> {
    Ok(run_remote_python_lines(ssh_target, exec_prefix, script, args, batch_mode)?.join("\n"))
}

fn read_session_content(session: &SessionSummary) -> Result<String> {
    if session.machine_target.is_none() {
        fs::read_to_string(&session.storage_path)
            .with_context(|| format!("failed to read {}", session.storage_path))
    } else {
        fetch_remote_session_content(session)
    }
}

fn fetch_remote_session_content(session: &SessionSummary) -> Result<String> {
    let ssh_target = session
        .machine_target
        .as_deref()
        .ok_or_else(|| anyhow!("remote session missing ssh target"))?;
    run_remote_python_text(
        ssh_target,
        session.machine_exec_prefix.as_deref(),
        REMOTE_READ_FILE_SCRIPT,
        std::slice::from_ref(&session.storage_path),
        true,
    )
}

fn upload_remote_file(
    ssh_target: &str,
    exec_prefix: Option<&str>,
    remote_file: &str,
    content: &str,
) -> Result<()> {
    let remote_file_q = sh_single_quote(remote_file);
    let remote_cmd = wrap_remote_exec(exec_prefix, &format!("cat > {remote_file_q}"));
    let mut cmd = Command::new("ssh");
    add_ssh_options(&mut cmd, false);
    let mut child = cmd
        .arg(ssh_target)
        .arg(remote_cmd)
        .stdin(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start ssh upload for {}", ssh_target))?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(content.as_bytes())
            .with_context(|| format!("failed to stream remote file {}", remote_file))?;
    } else {
        return Err(anyhow!("ssh stdin unavailable for upload"));
    }
    let status = child
        .wait()
        .with_context(|| format!("failed waiting for ssh upload to {}", ssh_target))?;
    if !status.success() {
        return Err(anyhow!(
            "ssh upload failed for {}:{} with status {}",
            ssh_target,
            remote_file,
            status
        ));
    }
    Ok(())
}

fn write_new_remote_session(
    ssh_target: &str,
    exec_prefix: Option<&str>,
    codex_home: &str,
    session_id: &str,
    out: &str,
) -> Result<String> {
    let now = Utc::now();
    let remote_dir = remote_session_dir(codex_home, now);
    run_ssh_status(
        ssh_target,
        exec_prefix,
        &format!("mkdir -p -- {}", sh_single_quote(&remote_dir)),
        false,
    )?;
    let remote_file = remote_session_path(
        codex_home,
        now,
        &format!(
            "rollout-{}-{}.jsonl",
            now.format("%Y-%m-%dT%H-%M-%S"),
            session_id
        ),
    );
    run_ssh_status(
        ssh_target,
        exec_prefix,
        &format!("test ! -e {}", sh_single_quote(&remote_file)),
        false,
    )
    .with_context(|| format!("remote file already exists: {}:{}", ssh_target, remote_file))?;
    upload_remote_file(ssh_target, exec_prefix, &remote_file, out)?;
    Ok(remote_file)
}

fn rewrite_remote_session_file(
    session: &SessionSummary,
    target_cwd: &str,
    rewrite_id: bool,
) -> Result<()> {
    let ssh_target = session
        .machine_target
        .as_deref()
        .ok_or_else(|| anyhow!("remote session missing ssh target"))?;
    let content = read_session_content(session)?;
    let new_id = if rewrite_id {
        Some(Uuid::new_v4().to_string())
    } else {
        None
    };
    let rewritten = rewrite_session_content(
        &content,
        target_cwd,
        new_id.as_deref(),
        false,
        &session.storage_path,
    )?;
    run_ssh_status(
        ssh_target,
        session.machine_exec_prefix.as_deref(),
        &format!(
            "cp -- {} {}",
            sh_single_quote(&session.storage_path),
            sh_single_quote(&format!("{}.bak", session.storage_path))
        ),
        false,
    )?;
    upload_remote_file(
        ssh_target,
        session.machine_exec_prefix.as_deref(),
        &session.storage_path,
        &rewritten,
    )?;
    let sync_session = SessionSummary {
        id: new_id.unwrap_or_else(|| session.id.clone()),
        cwd: target_cwd.to_string(),
        ..session.clone()
    };
    sync_remote_thread_index(
        ssh_target,
        session.machine_exec_prefix.as_deref(),
        &session.storage_path,
        target_cwd,
        &sync_session,
    )?;
    Ok(())
}

fn delete_remote_session_file(session: &SessionSummary) -> Result<()> {
    let ssh_target = session
        .machine_target
        .as_deref()
        .ok_or_else(|| anyhow!("remote session missing ssh target"))?;
    run_ssh_status(
        ssh_target,
        session.machine_exec_prefix.as_deref(),
        &format!("rm -f -- {}", sh_single_quote(&session.storage_path)),
        false,
    )
}

fn export_session_via_ssh(session: &SessionSummary, target: &str) -> Result<()> {
    let remote = parse_remote_export_target(target)?;
    let remote_codex_home = run_ssh_output(
        &remote.ssh_target,
        None,
        "python3 -c 'import os; print(os.environ.get(\"CODEX_HOME\") or os.path.expanduser(\"~/.codex\"))'",
        false,
    )?;
    let now = Utc::now();
    let remote_dir = remote_session_dir(&remote_codex_home, now);
    let remote_dir_q = sh_single_quote(&remote_dir);
    run_ssh_status(
        &remote.ssh_target,
        None,
        &format!("mkdir -p -- {remote_dir_q}"),
        false,
    )?;

    let remote_file = remote_session_path(&remote_codex_home, now, &session.file_name);
    let remote_file_q = sh_single_quote(&remote_file);
    run_ssh_status(
        &remote.ssh_target,
        None,
        &format!("test ! -e {remote_file_q}"),
        false,
    )
    .with_context(|| {
        format!(
            "remote file already exists: {}:{}",
            remote.ssh_target, remote_file
        )
    })?;

    let content = read_session_content(session)?;
    let rewritten = rewrite_session_content(
        &content,
        &remote.remote_cwd,
        None,
        false,
        &session.storage_path,
    )?;
    upload_remote_file(&remote.ssh_target, None, &remote_file, &rewritten)?;
    sync_remote_thread_index(
        &remote.ssh_target,
        None,
        &remote_file,
        &remote.remote_cwd,
        session,
    )?;
    Ok(())
}

fn sync_remote_thread_index(
    ssh_target: &str,
    exec_prefix: Option<&str>,
    remote_rollout_path: &str,
    remote_cwd: &str,
    session: &SessionSummary,
) -> Result<()> {
    let mut cmd = Command::new("ssh");
    add_ssh_options(&mut cmd, false);
    let remote_cmd = wrap_remote_exec(exec_prefix, "python3 -");
    let mut child = cmd
        .arg(ssh_target)
        .arg(remote_cmd)
        .stdin(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start remote index sync for {ssh_target}"))?;
    if let Some(stdin) = child.stdin.as_mut() {
        let script = format!(
            r#"import glob, json, os, sqlite3, sys
from datetime import datetime, timezone

remote_rollout_path = {rollout_path}
remote_cwd = {remote_cwd}
session_id_hint = {session_id}
session_title_hint = ""

def parse_ts(raw):
    try:
        return int(datetime.fromisoformat(raw.replace("Z", "+00:00")).timestamp())
    except Exception:
        return 0

codex_home = os.environ.get("CODEX_HOME") or os.path.expanduser("~/.codex")
dbs = sorted(glob.glob(os.path.join(codex_home, "state_*.sqlite")))
if not dbs:
    sys.exit(0)
db_path = dbs[-1]

session_id = session_id_hint
created_at = 0
updated_at = 0
source = "cli"
model_provider = "openai"
cli_version = ""
title = session_title_hint
first_user_message = session_title_hint
sandbox_policy = "{{}}"
approval_mode = ""
memory_mode = "enabled"

with open(remote_rollout_path, "r", encoding="utf-8") as f:
    for raw in f:
        raw = raw.strip()
        if not raw:
            continue
        obj = json.loads(raw)
        ts = parse_ts(obj.get("timestamp", ""))
        updated_at = max(updated_at, ts)
        if obj.get("type") == "session_meta":
            payload = obj.get("payload") or {{}}
            session_id = payload.get("id") or session_id
            created_at = parse_ts(payload.get("timestamp", "")) or created_at
            source = payload.get("source") or source
            model_provider = payload.get("model_provider") or model_provider
            cli_version = payload.get("cli_version") or cli_version
        elif obj.get("type") == "turn_context":
            payload = obj.get("payload") or {{}}
            if payload.get("sandbox_policy") is not None:
                sandbox_policy = json.dumps(payload.get("sandbox_policy"))
            approval_mode = payload.get("approval_policy") or approval_mode
            collab = payload.get("collaboration_mode") or {{}}
            memory_mode = collab.get("memory_mode") or payload.get("memory_mode") or memory_mode
        elif obj.get("type") == "response_item":
            payload = obj.get("payload") or {{}}
            if payload.get("type") == "message" and payload.get("role") in ("user", "developer") and not first_user_message:
                for item in payload.get("content") or []:
                    text = item.get("text") or item.get("input_text") or item.get("output_text")
                    if text:
                        first_user_message = text
                        title = text
                        break
        elif obj.get("type") == "event_msg":
            payload = obj.get("payload") or {{}}
            if payload.get("type") == "user_message" and not first_user_message:
                text = payload.get("message") or ""
                if text:
                    first_user_message = text
                    title = text

if not title:
    title = first_user_message or session_id
if not first_user_message:
    first_user_message = title
if not created_at:
    created_at = updated_at

con = sqlite3.connect(db_path)
cur = con.cursor()
cur.execute(
    "SELECT id FROM threads WHERE id = ? OR rollout_path = ? LIMIT 1",
    (session_id, remote_rollout_path),
)
row = cur.fetchone()
if row:
    cur.execute(
        "UPDATE threads SET cwd = ?, rollout_path = ?, updated_at = ?, title = ?, first_user_message = ?, source = ?, model_provider = ?, cli_version = ? WHERE id = ?",
        (remote_cwd, remote_rollout_path, updated_at, title, first_user_message, source, model_provider, cli_version, row[0]),
    )
else:
    cur.execute(
        "INSERT INTO threads (id, rollout_path, created_at, updated_at, source, model_provider, cwd, title, sandbox_policy, approval_mode, tokens_used, has_user_event, archived, cli_version, first_user_message, memory_mode) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, 0, 0, ?, ?, ?)",
        (session_id, remote_rollout_path, created_at, updated_at, source, model_provider, remote_cwd, title, sandbox_policy, approval_mode, cli_version, first_user_message, memory_mode),
    )
con.commit()
"#,
            rollout_path = serde_json::to_string(remote_rollout_path)?,
            remote_cwd = serde_json::to_string(remote_cwd)?,
            session_id = serde_json::to_string(&session.id)?,
        );
        stdin
            .write_all(script.as_bytes())
            .context("failed to stream remote index sync script")?;
    } else {
        return Err(anyhow!("ssh stdin unavailable for remote index sync"));
    }
    let status = child
        .wait()
        .with_context(|| format!("failed waiting for remote index sync on {ssh_target}"))?;
    if !status.success() {
        return Err(anyhow!(
            "remote index sync failed for {}:{} with status {}",
            ssh_target,
            remote_rollout_path,
            status
        ));
    }
    Ok(())
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

fn resolve_config_path() -> Result<PathBuf> {
    let cwd = env::current_dir().context("failed to resolve current directory")?;
    let local = cwd.join(".codex-session-tui.toml");
    if local.exists() {
        return Ok(local);
    }
    let home = env::var("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("codex-session-tui.toml"))
}

fn load_app_config(path: &Path) -> Result<AppConfig> {
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("invalid config {}", path.display()))
}

fn save_app_config(path: &Path, config: &AppConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let body = toml::to_string_pretty(config).context("failed to serialize config")?;
    atomic_write(path, &body)
}

fn infer_machine_name_from_ssh_target(target: &str) -> String {
    let trimmed = target.trim();
    let host_part = trimmed
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(trimmed);
    host_part
        .split(':')
        .next()
        .unwrap_or(host_part)
        .trim()
        .to_string()
}

fn parse_config_machine_input(input: &str) -> Result<ConfigMachine> {
    let trimmed = input.trim();
    let (name, rest) = if let Some((name, rest)) = trimmed.split_once('=') {
        let name = name.trim();
        let rest = rest.trim();
        if name.is_empty() || rest.is_empty() {
            return Err(anyhow!(
                "remote must look like user@host, name=user@host, name=user@host:/remote/.codex, or name=user@host|exec-prefix|/remote/.codex"
            ));
        }
        (name.to_string(), rest.to_string())
    } else {
        if trimmed.is_empty() {
            return Err(anyhow!(
                "remote must look like user@host, name=user@host, name=user@host:/remote/.codex, or name=user@host|exec-prefix|/remote/.codex"
            ));
        }
        (
            infer_machine_name_from_ssh_target(trimmed),
            trimmed.to_string(),
        )
    };

    if rest.contains('|') {
        let parts = rest.split('|').map(str::trim).collect::<Vec<_>>();
        if !(2..=3).contains(&parts.len()) {
            return Err(anyhow!(
                "remote with container/command prefix must look like user@host|exec-prefix, name=user@host|exec-prefix, or name=user@host|exec-prefix|/remote/.codex"
            ));
        }
        let ssh_target = parts[0];
        let exec_prefix = parts[1];
        let codex_home = parts.get(2).copied();
        if ssh_target.is_empty() {
            return Err(anyhow!("remote ssh target is empty"));
        }
        if exec_prefix.is_empty() {
            return Err(anyhow!("remote exec prefix is empty"));
        }
        if let Some(codex_home) = codex_home
            && (codex_home.is_empty() || !codex_home.starts_with('/'))
        {
            return Err(anyhow!("remote codex home must be an absolute path"));
        }
        return Ok(ConfigMachine {
            name,
            ssh_target: ssh_target.to_string(),
            exec_prefix: Some(exec_prefix.to_string()),
            codex_home: codex_home.map(str::to_string),
        });
    }

    if let Some((ssh_target, codex_home)) = rest.rsplit_once(":/")
        && !ssh_target.is_empty()
    {
        let codex_home = format!("/{}", codex_home);
        return Ok(ConfigMachine {
            name,
            ssh_target: ssh_target.trim().to_string(),
            exec_prefix: None,
            codex_home: Some(codex_home),
        });
    }
    Ok(ConfigMachine {
        name,
        ssh_target: rest.to_string(),
        codex_home: None,
        exec_prefix: None,
    })
}

fn upsert_config_machine(config: &mut AppConfig, machine: ConfigMachine) {
    if let Some(existing) = config.machines.iter_mut().find(|m| m.name == machine.name) {
        *existing = machine;
    } else if let Some(existing) = config.machines.iter_mut().find(|m| {
        m.ssh_target == machine.ssh_target
            && m.exec_prefix == machine.exec_prefix
            && m.codex_home == machine.codex_home
    }) {
        *existing = machine;
    } else {
        config.machines.push(machine);
        config.machines.sort_by(|a, b| a.name.cmp(&b.name));
    }
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
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn sample_chat_jsonl() -> String {
        [
            r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":"abc","timestamp":"2026-01-01T00:00:00Z","cwd":"/tmp/x"}}"#,
            r#"{"timestamp":"2026-01-01T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}}"#,
            r#"{"timestamp":"2026-01-01T00:00:02Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"world"}]}}"#,
            r#"{"timestamp":"2026-01-01T00:00:03Z","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"normalized user"}]}}"#,
        ]
        .join("\n")
    }

    fn buffer_lines(backend: &TestBackend) -> Vec<String> {
        let area = backend.buffer().area;
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| backend.buffer()[(x, y)].symbol().to_string())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .collect()
    }

    fn buffer_contains(backend: &TestBackend, needle: &str) -> bool {
        buffer_lines(backend)
            .iter()
            .any(|line| line.contains(needle))
    }

    fn write_test_session(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        fs::write(path, body).expect("write session");
    }

    fn empty_test_app() -> App {
        App {
            config_path: PathBuf::from("/tmp/codex-session-tui.toml"),
            config: AppConfig::default(),
            sessions_root: PathBuf::from("/tmp"),
            state_db_path: None,
            all_projects: Vec::new(),
            projects: Vec::new(),
            project_idx: 0,
            session_idx: 0,
            browser_cursor: BrowserCursor::Project,
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
            session_width_pct: 0,
            project_scroll: 0,
            session_scroll: 0,
            preview_scroll: 0,
            preview_content_len: 0,
            preview_selection: None,
            preview_rendered_lines: Vec::new(),
            preview_focus_turn: None,
            preview_cache: HashMap::new(),
            rendered_preview_cache: HashMap::new(),
            preview_folded: HashMap::new(),
            collapsed_projects: HashSet::new(),
            collapsed_groups: HashSet::new(),
            pinned_open_projects: HashSet::new(),
            selected_group_path: None,
            preview_header_rows: Vec::new(),
            preview_session_path: None,
            preview_search_matches: Vec::new(),
            preview_search_index: None,
            last_browser_nav_at: None,
            pending_preview_search_jump: None,
            browser_clipboard: None,
            last_browser_click: None,
            launch_codex_after_exit: None,
            remote_states: BTreeMap::new(),
        }
    }

    fn init_test_state_db(path: &Path) {
        let conn = Connection::open(path).expect("open sqlite");
        conn.execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                rollout_path TEXT NOT NULL,
                created_at INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0,
                source TEXT NOT NULL DEFAULT '',
                model_provider TEXT NOT NULL DEFAULT '',
                cwd TEXT NOT NULL DEFAULT '',
                title TEXT NOT NULL DEFAULT '',
                sandbox_policy TEXT NOT NULL DEFAULT '',
                approval_mode TEXT NOT NULL DEFAULT '',
                tokens_used INTEGER NOT NULL DEFAULT 0,
                has_user_event INTEGER NOT NULL DEFAULT 0,
                archived INTEGER NOT NULL DEFAULT 0,
                archived_at INTEGER,
                git_sha TEXT,
                git_branch TEXT,
                git_origin_url TEXT,
                cli_version TEXT NOT NULL DEFAULT '',
                first_user_message TEXT NOT NULL DEFAULT '',
                agent_nickname TEXT,
                agent_role TEXT,
                memory_mode TEXT NOT NULL DEFAULT 'enabled'
            );",
        )
        .expect("create threads table");
    }

    fn sample_session(path: &str, cwd: &str, id: &str) -> SessionSummary {
        SessionSummary {
            path: PathBuf::from(path),
            storage_path: String::from(path),
            file_name: format!("{id}.jsonl"),
            id: String::from(id),
            cwd: String::from(cwd),
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            started_at: String::from("2026-01-01T00:00:00Z"),
            modified_epoch: 123,
            event_count: 1,
            user_message_count: 1,
            assistant_message_count: 1,
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
    fn search_requires_literal_token_presence_not_fuzzy_character_walk() {
        let score = search_score("abc", "a_b_c", "/repo/demo", "demo.jsonl", "sess-1");
        assert!(score.is_none());
    }

    #[test]
    fn search_tokens_match_across_multiple_words() {
        let score = search_score(
            "deploy alpha",
            "fix deploy pipeline for alpha release",
            "/repo/alpha",
            "deploy-alpha.jsonl",
            "sess-1",
        );
        assert!(score.is_some());
    }

    #[test]
    fn search_tokens_require_all_terms() {
        let score = search_score(
            "deploy alpha",
            "deploy pipeline only",
            "/repo/beta",
            "deploy.jsonl",
            "sess-1",
        );
        assert!(score.is_none());
    }

    #[test]
    fn search_tokens_preserve_quoted_phrases() {
        assert_eq!(
            search_tokens(r#""openrouter error" auth"#),
            vec![String::from("openrouter error"), String::from("auth")]
        );
    }

    #[test]
    fn search_score_matches_quoted_phrase_literal() {
        let score = search_score(
            r#""openrouter error" auth"#,
            "investigate openrouter error during auth refresh",
            "/repo/demo",
            "demo.jsonl",
            "sess-1",
        );
        assert!(score.is_some());

        let miss = search_score(
            r#""openrouter error""#,
            "openrouter timeout and auth refresh",
            "/repo/demo",
            "demo.jsonl",
            "sess-1",
        );
        assert!(miss.is_none());
    }

    #[test]
    fn project_tree_label_uses_shared_prefix() {
        let projects = vec![
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/work/src/api"),
                sessions: Vec::new(),
            },
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/work/src/web"),
                sessions: Vec::new(),
            },
        ];
        let label = project_label(&projects, 0);
        assert!(label.contains("api"));
        assert!(!label.contains("/work/src/api"));
    }

    #[test]
    fn project_tree_label_keeps_missing_parent_segments() {
        let projects = vec![
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/work/src/foo"),
                sessions: Vec::new(),
            },
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/work/src/bar/baz"),
                sessions: Vec::new(),
            },
        ];

        assert_eq!(project_label(&projects, 0), "foo");
        assert_eq!(project_label(&projects, 1), "bar/baz");
    }

    #[test]
    fn project_tree_indent_only_uses_existing_project_ancestors() {
        let projects = vec![
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/work/src/foo"),
                sessions: Vec::new(),
            },
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/work/src/bar"),
                sessions: Vec::new(),
            },
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/work/src/bar/baz"),
                sessions: Vec::new(),
            },
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/work/src/bar/baz/qux"),
                sessions: Vec::new(),
            },
        ];

        assert_eq!(project_indent(&projects, 0), "");
        assert_eq!(project_indent(&projects, 1), "");
        assert_eq!(project_indent(&projects, 2), "  ");
        assert_eq!(project_indent(&projects, 3), "    ");
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
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd: String::from("/repo"),
            sessions: vec![
                sample_session("/tmp/a.jsonl", "/repo", "a"),
                sample_session("/tmp/b.jsonl", "/repo", "b"),
            ],
        }];
        app.focus = Focus::Projects;
        app.browser_cursor = BrowserCursor::Session;
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
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd: String::from("/repo"),
            sessions: vec![
                sample_session("/tmp/a.jsonl", "/repo", "a"),
                sample_session("/tmp/b.jsonl", "/repo", "b"),
                sample_session("/tmp/c.jsonl", "/repo", "c"),
            ],
        }];
        app.focus = Focus::Projects;
        app.browser_cursor = BrowserCursor::Session;
        app.session_idx = 1;

        app.select_all_sessions_current_project();
        assert_eq!(app.selected_count_current_project(), 3);

        app.invert_sessions_selection_current_project();
        assert_eq!(app.selected_count_current_project(), 0);
    }

    #[test]
    fn action_targets_prefers_selected_sessions() {
        let mut app = empty_test_app();
        app.focus = Focus::Projects;
        app.browser_cursor = BrowserCursor::Session;
        app.projects = vec![ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
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
    fn browser_rows_hide_sessions_when_project_collapsed() {
        let mut app = empty_test_app();
        app.projects = vec![ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd: String::from("/repo"),
            sessions: vec![
                sample_session("/tmp/a.jsonl", "/repo", "a"),
                sample_session("/tmp/b.jsonl", "/repo", "b"),
            ],
        }];
        assert_eq!(app.browser_rows().len(), 4);
        app.collapsed_projects.insert(String::from("/repo"));
        assert_eq!(app.browser_rows().len(), 2);
    }

    #[test]
    fn browser_row_index_counts_expanded_projects_above_selection() {
        let mut app = empty_test_app();
        app.projects = vec![
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo-a"),
                sessions: vec![sample_session("/tmp/a.jsonl", "/repo-a", "a")],
            },
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo-b"),
                sessions: vec![sample_session("/tmp/b.jsonl", "/repo-b", "b")],
            },
        ];
        app.project_idx = 1;
        app.browser_cursor = BrowserCursor::Project;

        assert_eq!(app.current_browser_row_index(), 4);
    }

    #[test]
    fn delete_key_starts_delete_action_for_session_row() {
        let mut app = empty_test_app();
        app.projects = vec![ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd: String::from("/repo"),
            sessions: vec![sample_session("/tmp/a.jsonl", "/repo", "a")],
        }];
        app.browser_cursor = BrowserCursor::Session;
        app.focus = Focus::Projects;
        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        let quit = handle_normal_mode(key, &mut app).expect("handle");
        assert!(!quit);
        assert_eq!(app.mode, Mode::Input);
        assert_eq!(app.pending_action, Some(Action::Delete));
    }

    #[test]
    fn move_copy_and_fork_keys_start_actions_for_session_row() {
        let actions = [
            (KeyCode::Char('m'), Action::Move),
            (KeyCode::Char('c'), Action::Copy),
            (KeyCode::Char('f'), Action::Fork),
            (KeyCode::Char('e'), Action::Export),
        ];

        for (code, expected) in actions {
            let mut app = empty_test_app();
            app.projects = vec![ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo"),
                sessions: vec![sample_session("/tmp/a.jsonl", "/repo", "a")],
            }];
            app.browser_cursor = BrowserCursor::Session;
            app.focus = Focus::Projects;

            let quit = handle_normal_mode(KeyEvent::new(code, KeyModifiers::NONE), &mut app)
                .expect("handle");
            assert!(!quit);
            assert_eq!(app.mode, Mode::Input);
            assert_eq!(app.pending_action, Some(expected));
        }
    }

    #[test]
    fn parse_remote_export_target_requires_ssh_destination_format() {
        let parsed = parse_remote_export_target("avikalpa@example.com:/var/tmp/codex/project")
            .expect("parse");
        assert_eq!(
            parsed,
            RemoteExportTarget {
                ssh_target: String::from("avikalpa@example.com"),
                remote_cwd: String::from("/var/tmp/codex/project"),
            }
        );
        assert!(parse_remote_export_target("/var/tmp/codex/project").is_err());
        assert!(parse_remote_export_target("example.com:").is_err());
    }

    #[test]
    fn remote_join_path_preserves_single_separator() {
        assert_eq!(
            remote_join_path("/var/tmp/codex", "a.jsonl"),
            "/var/tmp/codex/a.jsonl"
        );
        assert_eq!(
            remote_join_path("/var/tmp/codex/", "a.jsonl"),
            "/var/tmp/codex/a.jsonl"
        );
    }

    #[test]
    fn remote_session_path_uses_remote_codex_home_layout() {
        let ts = chrono::DateTime::parse_from_rfc3339("2026-03-14T17:00:00Z")
            .expect("ts")
            .with_timezone(&Utc);
        assert_eq!(
            remote_session_dir("/home/pi/.codex", ts),
            "/home/pi/.codex/sessions/2026/03/14"
        );
        assert_eq!(
            remote_session_path("/home/pi/.codex", ts, "session.jsonl"),
            "/home/pi/.codex/sessions/2026/03/14/session.jsonl"
        );
    }

    #[test]
    fn rewrite_session_content_rewrites_export_target_cwd() {
        let input = [
            r#"{"timestamp":"2026-03-14T00:00:00Z","type":"session_meta","payload":{"id":"sess-1","timestamp":"2026-03-14T00:00:00Z","cwd":"/old/path"}}"#,
            r#"{"timestamp":"2026-03-14T00:00:01Z","type":"turn_context","payload":{"cwd":"/old/path"}}"#,
        ]
        .join("\n");
        let out = rewrite_session_content(&input, "/remote/project/path", None, false, "test")
            .expect("rewrite");
        assert!(out.contains("\"cwd\":\"/remote/project/path\""));
        assert!(!out.contains("\"cwd\":\"/old/path\""));
    }

    #[test]
    fn ctrl_c_copies_browser_selection_into_clipboard() {
        let mut app = empty_test_app();
        app.projects = vec![ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd: String::from("/repo"),
            sessions: vec![
                sample_session("/tmp/a.jsonl", "/repo", "a"),
                sample_session("/tmp/b.jsonl", "/repo", "b"),
            ],
        }];
        app.browser_cursor = BrowserCursor::Session;
        app.focus = Focus::Projects;
        app.session_idx = 1;

        handle_normal_mode(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &mut app,
        )
        .expect("handle");

        let clipboard = app.browser_clipboard.expect("clipboard");
        assert_eq!(clipboard.mode, BrowserClipboardMode::Copy);
        assert_eq!(clipboard.targets.len(), 1);
        assert_eq!(clipboard.targets[0].id, "b");
    }

    #[test]
    fn ctrl_v_pastes_copied_session_into_current_folder() {
        let dir = std::env::temp_dir().join(format!("cse-paste-{}", Uuid::new_v4()));
        let sessions_root = dir.join("sessions");
        let now = Utc::now();
        let dated_dir = sessions_root
            .join(now.format("%Y").to_string())
            .join(now.format("%m").to_string())
            .join(now.format("%d").to_string());
        let source_path = dated_dir.join("source.jsonl");
        write_test_session(&source_path, &sample_chat_jsonl());

        let mut app = empty_test_app();
        app.sessions_root = sessions_root.clone();
        let source = SessionSummary {
            path: source_path.clone(),
            storage_path: path_to_string(Path::new(&source_path.clone())),
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            file_name: String::from("source.jsonl"),
            id: String::from("abc"),
            cwd: String::from("/old"),
            started_at: String::from("2026-01-01T00:00:00Z"),
            modified_epoch: 123,
            event_count: 4,
            user_message_count: 2,
            assistant_message_count: 1,
            search_blob: String::new(),
        };
        app.projects = vec![ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd: String::from("/new"),
            sessions: vec![],
        }];
        app.browser_cursor = BrowserCursor::Project;
        app.focus = Focus::Projects;
        app.browser_clipboard = Some(BrowserClipboard {
            mode: BrowserClipboardMode::Copy,
            targets: vec![source],
            source_label: String::from("/old"),
        });

        handle_normal_mode(
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL),
            &mut app,
        )
        .expect("handle");

        let created = fs::read_dir(dated_dir)
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
            .collect::<Vec<_>>();
        assert_eq!(created.len(), 2);

        let pasted = created
            .into_iter()
            .find(|path| path != &source_path)
            .expect("pasted file");
        let content = fs::read_to_string(pasted).expect("read pasted");
        assert!(content.contains("\"cwd\":\"/new\""));
    }

    #[test]
    fn normalize_local_target_cwd_makes_path_absolute_and_trims_slash() {
        let base = PathBuf::from("/root/gh/codex-session-tui");
        assert_eq!(
            normalize_local_target_cwd("./demo/", &base).expect("normalize"),
            "/root/gh/codex-session-tui/demo"
        );
        assert_eq!(
            normalize_local_target_cwd("/tmp/example//", &base).expect("normalize"),
            "/tmp/example"
        );
    }

    #[test]
    fn repair_session_file_cwds_normalizes_existing_bad_cwds() {
        let dir = std::env::temp_dir().join(format!("cse-repair-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("broken.jsonl");
        fs::write(
            &path,
            [
                r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":"abc","timestamp":"2026-01-01T00:00:00Z","cwd":"./repo/"}}"#,
                r#"{"timestamp":"2026-01-01T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","cwd":"./repo/","content":[{"type":"input_text","text":"hello"}]}}"#,
            ]
            .join("\n"),
        )
        .expect("write");

        let changed = repair_session_file_cwds(&path, Path::new("/root/work")).expect("repair");
        assert!(changed);

        let repaired = fs::read_to_string(&path).expect("read repaired");
        assert!(repaired.contains("\"cwd\":\"/root/work/repo\""));
        assert!(!repaired.contains("\"cwd\":\"./repo/\""));
        assert!(
            path.parent()
                .expect("parent")
                .read_dir()
                .expect("read dir")
                .filter_map(Result::ok)
                .any(|entry| entry.file_name().to_string_lossy().contains(".jsonl.bak."))
        );
    }

    #[test]
    fn repair_session_cwds_updates_existing_pre_moved_sessions() {
        let root = std::env::temp_dir().join(format!("cse-repair-root-{}", Uuid::new_v4()));
        let nested = root.join("2026/03/14");
        fs::create_dir_all(&nested).expect("mkdir");
        let path = nested.join("session.jsonl");
        fs::write(
            &path,
            r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":"abc","timestamp":"2026-01-01T00:00:00Z","cwd":"../repo/"}}"#,
        )
        .expect("write");

        let repaired = repair_session_cwds(&root, Path::new("/root/gh/codex-session-tui"))
            .expect("repair tree");
        assert_eq!(repaired, 1);

        let content = fs::read_to_string(&path).expect("read");
        assert!(content.contains("\"cwd\":\"/root/gh/repo\""));
    }

    #[test]
    fn resolve_state_db_path_picks_latest_state_db() {
        let dir = std::env::temp_dir().join(format!("cse-state-db-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(dir.join("state_2.sqlite"), "").expect("write");
        fs::write(dir.join("state_5.sqlite"), "").expect("write");
        fs::write(dir.join("state_5.sqlite-wal"), "").expect("write");

        let picked = resolve_state_db_path(&dir).expect("picked");
        assert_eq!(picked, dir.join("state_5.sqlite"));
    }

    #[test]
    fn sync_thread_record_updates_stale_cwd_in_state_db() {
        let dir = std::env::temp_dir().join(format!("cse-sync-thread-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("mkdir");
        let db = dir.join("state_5.sqlite");
        init_test_state_db(&db);

        let rollout = dir.join("sessions/2026/03/14/session.jsonl");
        let rollout_s = path_to_string(&rollout);
        let conn = Connection::open(&db).expect("open");
        conn.execute(
            "INSERT INTO threads (id, rollout_path, cwd, title, first_user_message) VALUES (?1, ?2, ?3, '', '')",
            params!["sess-1", rollout_s, "/old/path"],
        )
        .expect("insert");
        drop(conn);

        let changed =
            sync_thread_record(&db, "sess-1", &rollout, "/new/path", &rollout).expect("sync");
        assert!(changed);

        let conn = Connection::open(&db).expect("open");
        let row = conn
            .query_row(
                "SELECT cwd, rollout_path FROM threads WHERE id = 'sess-1'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .expect("select");
        assert_eq!(row.0, "/new/path");
        assert_eq!(row.1, path_to_string(&rollout));
    }

    #[test]
    fn sync_threads_db_from_projects_repairs_existing_stale_index_rows() {
        let dir = std::env::temp_dir().join(format!("cse-sync-projects-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("mkdir");
        let db = dir.join("state_5.sqlite");
        init_test_state_db(&db);

        let rollout = dir.join("sessions/2026/03/14/session.jsonl");
        let rollout_s = path_to_string(&rollout);
        let conn = Connection::open(&db).expect("open");
        conn.execute(
            "INSERT INTO threads (id, rollout_path, cwd, title, first_user_message) VALUES (?1, ?2, ?3, '', '')",
            params!["sess-1", rollout_s, "/old/path"],
        )
        .expect("insert");
        drop(conn);

        let projects = vec![ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd: String::from("/new/path"),
            sessions: vec![SessionSummary {
                path: rollout.clone(),
                storage_path: path_to_string(Path::new(&rollout.clone())),
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                file_name: String::from("session.jsonl"),
                id: String::from("sess-1"),
                cwd: String::from("/new/path"),
                started_at: String::from("2026-03-14T00:00:00Z"),
                modified_epoch: 1,
                event_count: 1,
                user_message_count: 1,
                assistant_message_count: 0,
                search_blob: String::new(),
            }],
        }];

        let repaired = sync_threads_db_from_projects(&db, &projects).expect("sync all");
        assert_eq!(repaired, 1);

        let conn = Connection::open(&db).expect("open");
        let cwd = conn
            .query_row("SELECT cwd FROM threads WHERE id = 'sess-1'", [], |row| {
                row.get::<_, String>(0)
            })
            .expect("select");
        assert_eq!(cwd, "/new/path");
    }

    #[test]
    fn submit_input_move_updates_state_db_for_session() {
        let dir = std::env::temp_dir().join(format!("cse-move-state-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("mkdir");
        let db = dir.join("state_5.sqlite");
        init_test_state_db(&db);

        let session_path = dir.join("sessions/2026/03/14/session.jsonl");
        write_test_session(
            &session_path,
            r#"{"timestamp":"2026-03-14T00:00:00Z","type":"session_meta","payload":{"id":"sess-1","timestamp":"2026-03-14T00:00:00Z","cwd":"/old/path"}}"#,
        );

        let conn = Connection::open(&db).expect("open");
        conn.execute(
            "INSERT INTO threads (id, rollout_path, cwd, title, first_user_message) VALUES (?1, ?2, ?3, '', '')",
            params!["sess-1", path_to_string(&session_path), "/old/path"],
        )
        .expect("insert");
        drop(conn);

        let mut app = empty_test_app();
        app.sessions_root = dir.join("sessions");
        app.state_db_path = Some(db.clone());
        app.projects = vec![ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd: String::from("/old/path"),
            sessions: vec![SessionSummary {
                path: session_path.clone(),
                storage_path: path_to_string(Path::new(&session_path.clone())),
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                file_name: String::from("session.jsonl"),
                id: String::from("sess-1"),
                cwd: String::from("/old/path"),
                started_at: String::from("2026-03-14T00:00:00Z"),
                modified_epoch: 1,
                event_count: 1,
                user_message_count: 1,
                assistant_message_count: 0,
                search_blob: String::new(),
            }],
        }];
        app.all_projects = app.projects.clone();
        app.focus = Focus::Projects;
        app.browser_cursor = BrowserCursor::Session;
        app.pending_action = Some(Action::Move);
        app.mode = Mode::Input;
        app.input = String::from("/new/path/");

        app.submit_input().expect("submit");

        let conn = Connection::open(&db).expect("open");
        let cwd = conn
            .query_row("SELECT cwd FROM threads WHERE id = 'sess-1'", [], |row| {
                row.get::<_, String>(0)
            })
            .expect("select");
        assert_eq!(cwd, "/new/path");
    }

    #[test]
    fn register_browser_click_detects_double_click_on_same_row() {
        let mut app = empty_test_app();
        let row = BrowserRow {
            kind: BrowserRowKind::Project { project_idx: 0 },
            depth: 0,
            label: String::from("repo"),
        };
        let now = Instant::now();

        assert!(!app.register_browser_click(row.clone(), now));
        assert!(app.register_browser_click(
            row,
            now.checked_add(Duration::from_millis(200)).unwrap_or(now)
        ));
        assert!(!app.register_browser_click(
            BrowserRow {
                kind: BrowserRowKind::Session {
                    project_idx: 0,
                    session_idx: 0,
                },
                depth: 1,
                label: String::from("sess"),
            },
            now.checked_add(Duration::from_millis(250)).unwrap_or(now)
        ));
    }

    #[test]
    fn double_click_project_row_toggles_folder() {
        let mut app = empty_test_app();
        app.projects = vec![ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd: String::from("/repo"),
            sessions: vec![sample_session("/tmp/a.jsonl", "/repo", "a")],
        }];
        app.panes.browser = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 8,
        };

        handle_mouse_event(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 8,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
            &mut app,
        );
        assert!(!app.collapsed_projects.contains("/repo"));

        handle_mouse_event(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 8,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
            &mut app,
        );
        assert!(app.collapsed_projects.contains("/repo"));
    }

    #[test]
    fn browser_enter_toggles_project_and_session_enter_focuses_preview() {
        let mut app = empty_test_app();
        app.projects = vec![ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd: String::from("/repo"),
            sessions: vec![sample_session("/tmp/a.jsonl", "/repo", "a")],
        }];
        app.collapsed_projects.insert(String::from("/repo"));

        app.browser_enter();
        assert_eq!(app.browser_cursor, BrowserCursor::Project);
        assert!(!app.collapsed_projects.contains("/repo"));

        app.browser_enter();
        assert_eq!(app.browser_cursor, BrowserCursor::Project);
        assert!(app.collapsed_projects.contains("/repo"));

        app.browser_cursor = BrowserCursor::Session;

        app.browser_enter();
        assert_eq!(app.focus, Focus::Preview);
    }

    #[test]
    fn browser_row_navigation_auto_collapses_unpinned_projects() {
        let mut app = empty_test_app();
        app.projects = vec![
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo-a"),
                sessions: vec![
                    sample_session("/tmp/a1.jsonl", "/repo-a", "a1"),
                    sample_session("/tmp/a2.jsonl", "/repo-a", "a2"),
                ],
            },
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo-b"),
                sessions: vec![sample_session("/tmp/b1.jsonl", "/repo-b", "b1")],
            },
        ];

        app.move_down();
        assert_eq!(app.browser_cursor, BrowserCursor::Session);
        assert_eq!(app.project_idx, 0);
        assert_eq!(app.session_idx, 0);

        app.move_down();
        assert_eq!(app.browser_cursor, BrowserCursor::Session);
        assert_eq!(app.project_idx, 0);
        assert_eq!(app.session_idx, 1);

        app.move_down();
        assert_eq!(app.browser_cursor, BrowserCursor::Project);
        assert_eq!(app.project_idx, 1);
        assert!(app.collapsed_projects.contains("/repo-a"));

        app.move_up();
        assert_eq!(app.browser_cursor, BrowserCursor::Project);
        assert_eq!(app.project_idx, 0);
        assert!(!app.collapsed_projects.contains("/repo-a"));
    }

    #[test]
    fn browser_rows_preserve_order_with_mixed_collapsed_projects() {
        let mut app = empty_test_app();
        app.projects = vec![
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo-a"),
                sessions: vec![
                    sample_session("/tmp/a1.jsonl", "/repo-a", "a1"),
                    sample_session("/tmp/a2.jsonl", "/repo-a", "a2"),
                ],
            },
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo-b"),
                sessions: vec![sample_session("/tmp/b1.jsonl", "/repo-b", "b1")],
            },
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo-c"),
                sessions: vec![sample_session("/tmp/c1.jsonl", "/repo-c", "c1")],
            },
        ];
        app.collapsed_projects.insert(String::from("/repo-b"));

        let rows = app.browser_rows();
        let shape = rows
            .iter()
            .map(|row| match &row.kind {
                BrowserRowKind::Group { path } => format!("g:{path}"),
                BrowserRowKind::Project { project_idx } => format!("p:{project_idx}"),
                BrowserRowKind::Session {
                    project_idx,
                    session_idx,
                } => format!("s:{project_idx}:{session_idx}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            shape,
            vec![
                "g:local".to_string(),
                "g:local/".to_string(),
                "p:0".to_string(),
                "s:0:0".to_string(),
                "s:0:1".to_string(),
                "p:1".to_string(),
                "p:2".to_string(),
                "s:2:0".to_string(),
            ]
        );
    }

    #[test]
    fn repeated_browser_navigation_clamps_at_visible_bounds() {
        let mut app = empty_test_app();
        app.projects = vec![
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo-a"),
                sessions: vec![
                    sample_session("/tmp/a1.jsonl", "/repo-a", "a1"),
                    sample_session("/tmp/a2.jsonl", "/repo-a", "a2"),
                ],
            },
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo-b"),
                sessions: vec![sample_session("/tmp/b1.jsonl", "/repo-b", "b1")],
            },
        ];

        for _ in 0..12 {
            app.move_down();
        }
        let rows = app.browser_rows();
        let last = rows.last().cloned().expect("last row");
        match last.kind {
            BrowserRowKind::Group { path } => {
                assert_eq!(app.browser_cursor, BrowserCursor::Group);
                assert_eq!(app.selected_group_path.as_deref(), Some(path.as_str()));
            }
            BrowserRowKind::Project { project_idx } => {
                assert_eq!(app.browser_cursor, BrowserCursor::Project);
                assert_eq!(app.project_idx, project_idx);
            }
            BrowserRowKind::Session {
                project_idx,
                session_idx,
            } => {
                assert_eq!(app.browser_cursor, BrowserCursor::Session);
                assert_eq!(app.project_idx, project_idx);
                assert_eq!(app.session_idx, session_idx);
            }
        }

        for _ in 0..12 {
            app.move_up();
        }
        assert_eq!(app.project_idx, 0);
        assert_eq!(app.browser_cursor, BrowserCursor::Group);
        assert_eq!(app.selected_group_path.as_deref(), Some("local"));
    }

    #[test]
    fn right_on_project_row_enters_first_session_when_expanded() {
        let mut app = empty_test_app();
        app.projects = vec![ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd: String::from("/repo"),
            sessions: vec![
                sample_session("/tmp/a.jsonl", "/repo", "a"),
                sample_session("/tmp/b.jsonl", "/repo", "b"),
            ],
        }];
        app.focus = Focus::Projects;
        app.browser_cursor = BrowserCursor::Project;

        let quit = handle_normal_mode(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE), &mut app)
            .expect("handle");
        assert!(!quit);
        assert_eq!(app.browser_cursor, BrowserCursor::Session);
        assert_eq!(app.session_idx, 0);
    }

    #[test]
    fn left_on_session_row_returns_to_project_row() {
        let mut app = empty_test_app();
        app.projects = vec![ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd: String::from("/repo"),
            sessions: vec![
                sample_session("/tmp/a.jsonl", "/repo", "a"),
                sample_session("/tmp/b.jsonl", "/repo", "b"),
            ],
        }];
        app.focus = Focus::Projects;
        app.browser_cursor = BrowserCursor::Session;
        app.session_idx = 1;

        let quit = handle_normal_mode(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE), &mut app)
            .expect("handle");
        assert!(!quit);
        assert_eq!(app.browser_cursor, BrowserCursor::Project);
        assert_eq!(app.project_idx, 0);
    }

    #[test]
    fn left_and_right_toggle_project_collapse_state() {
        let mut app = empty_test_app();
        app.projects = vec![ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd: String::from("/repo"),
            sessions: vec![sample_session("/tmp/a.jsonl", "/repo", "a")],
        }];
        app.focus = Focus::Projects;
        app.browser_cursor = BrowserCursor::Project;

        handle_normal_mode(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE), &mut app)
            .expect("left");
        assert!(app.collapsed_projects.contains("/repo"));

        handle_normal_mode(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE), &mut app)
            .expect("right");
        assert!(!app.collapsed_projects.contains("/repo"));
    }

    #[test]
    fn ctrl_left_collapses_all_except_current_project() {
        let mut app = empty_test_app();
        app.projects = vec![
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo-a"),
                sessions: vec![sample_session("/tmp/a.jsonl", "/repo-a", "a")],
            },
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo-b"),
                sessions: vec![sample_session("/tmp/b.jsonl", "/repo-b", "b")],
            },
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo-c"),
                sessions: vec![sample_session("/tmp/c.jsonl", "/repo-c", "c")],
            },
        ];
        app.project_idx = 1;
        app.browser_cursor = BrowserCursor::Session;

        let quit = handle_normal_mode(
            KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL),
            &mut app,
        )
        .expect("handle");
        assert!(!quit);
        assert_eq!(app.browser_cursor, BrowserCursor::Project);
        assert!(!app.collapsed_projects.contains("/repo-b"));
        assert!(app.collapsed_projects.contains("/repo-a"));
        assert!(app.collapsed_projects.contains("/repo-c"));
    }

    #[test]
    fn ctrl_right_expands_all_projects() {
        let mut app = empty_test_app();
        app.projects = vec![
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo-a"),
                sessions: vec![sample_session("/tmp/a.jsonl", "/repo-a", "a")],
            },
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo-b"),
                sessions: vec![sample_session("/tmp/b.jsonl", "/repo-b", "b")],
            },
        ];
        app.collapsed_projects.insert(String::from("/repo-a"));
        app.collapsed_projects.insert(String::from("/repo-b"));

        let quit = handle_normal_mode(
            KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL),
            &mut app,
        )
        .expect("handle");
        assert!(!quit);
        assert!(app.collapsed_projects.is_empty());
    }

    #[test]
    fn ctrl_down_and_up_jump_between_projects_only() {
        let mut app = empty_test_app();
        app.projects = vec![
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo-a"),
                sessions: vec![sample_session("/tmp/a.jsonl", "/repo-a", "a")],
            },
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo-b"),
                sessions: vec![sample_session("/tmp/b.jsonl", "/repo-b", "b")],
            },
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo-c"),
                sessions: vec![sample_session("/tmp/c.jsonl", "/repo-c", "c")],
            },
        ];
        app.focus = Focus::Projects;
        app.browser_cursor = BrowserCursor::Session;
        app.session_idx = 0;

        handle_normal_mode(
            KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL),
            &mut app,
        )
        .expect("ctrl-down");
        assert_eq!(app.project_idx, 1);
        assert_eq!(app.browser_cursor, BrowserCursor::Project);

        handle_normal_mode(
            KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL),
            &mut app,
        )
        .expect("ctrl-down");
        assert_eq!(app.project_idx, 2);
        assert_eq!(app.browser_cursor, BrowserCursor::Project);

        handle_normal_mode(KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL), &mut app)
            .expect("ctrl-up");
        assert_eq!(app.project_idx, 1);
        assert_eq!(app.browser_cursor, BrowserCursor::Project);
    }

    #[test]
    fn moving_up_to_project_row_auto_expands_it() {
        let mut app = empty_test_app();
        app.projects = vec![ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd: String::from("/repo"),
            sessions: vec![
                sample_session("/tmp/a.jsonl", "/repo", "a"),
                sample_session("/tmp/b.jsonl", "/repo", "b"),
            ],
        }];
        app.browser_cursor = BrowserCursor::Session;
        app.session_idx = 0;

        app.move_up();

        assert_eq!(app.browser_cursor, BrowserCursor::Project);
        assert!(!app.collapsed_projects.contains("/repo"));
    }

    #[test]
    fn moving_onto_project_row_auto_expands_it() {
        let mut app = empty_test_app();
        app.projects = vec![
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo-a"),
                sessions: vec![sample_session("/tmp/a.jsonl", "/repo-a", "a")],
            },
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo-b"),
                sessions: vec![sample_session("/tmp/b.jsonl", "/repo-b", "b")],
            },
        ];
        app.browser_cursor = BrowserCursor::Session;
        app.session_idx = 0;
        app.collapsed_projects.insert(String::from("/repo-b"));

        app.move_down();

        assert_eq!(app.project_idx, 1);
        assert_eq!(app.browser_cursor, BrowserCursor::Project);
        assert!(!app.collapsed_projects.contains("/repo-b"));
    }

    #[test]
    fn pinned_project_stays_open_when_navigating_to_next_project() {
        let mut app = empty_test_app();
        app.projects = vec![
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo-a"),
                sessions: vec![sample_session("/tmp/a.jsonl", "/repo-a", "a")],
            },
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo-b"),
                sessions: vec![sample_session("/tmp/b.jsonl", "/repo-b", "b")],
            },
        ];
        app.pinned_open_projects.insert(String::from("/repo-a"));
        app.browser_cursor = BrowserCursor::Session;
        app.session_idx = 0;

        app.move_down();

        assert_eq!(app.project_idx, 1);
        assert_eq!(app.browser_cursor, BrowserCursor::Project);
        assert!(!app.collapsed_projects.contains("/repo-a"));
    }

    #[test]
    fn mouse_project_toggle_pins_folder_open_and_closed() {
        let mut app = empty_test_app();
        app.projects = vec![ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd: String::from("/repo"),
            sessions: vec![sample_session("/tmp/a.jsonl", "/repo", "a")],
        }];
        app.panes.browser = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 8,
        };

        handle_mouse_event(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 4,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
            &mut app,
        );
        assert!(app.collapsed_projects.contains("/repo"));
        assert!(!app.pinned_open_projects.contains("/repo"));

        handle_mouse_event(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 4,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
            &mut app,
        );
        assert!(!app.collapsed_projects.contains("/repo"));
        assert!(app.pinned_open_projects.contains("/repo"));
    }

    #[test]
    fn delete_targets_prefers_selected_sessions() {
        let mut app = empty_test_app();
        app.focus = Focus::Projects;
        app.browser_cursor = BrowserCursor::Session;
        app.projects = vec![ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
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
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
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
            config_path: PathBuf::from("/tmp/codex-session-tui.toml"),
            config: AppConfig::default(),
            sessions_root: PathBuf::from("/tmp"),
            state_db_path: None,
            all_projects: Vec::new(),
            projects: Vec::new(),
            project_idx: 0,
            session_idx: 0,
            browser_cursor: BrowserCursor::Project,
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
            session_width_pct: 0,
            project_scroll: 0,
            session_scroll: 0,
            preview_scroll: 0,
            preview_content_len: 2,
            preview_selection: None,
            preview_rendered_lines: vec![String::from("abcde"), String::from("vwxyz")],
            preview_focus_turn: None,
            preview_cache: HashMap::new(),
            rendered_preview_cache: HashMap::new(),
            preview_folded: HashMap::new(),
            collapsed_projects: HashSet::new(),
            collapsed_groups: HashSet::new(),
            pinned_open_projects: HashSet::new(),
            selected_group_path: None,
            preview_header_rows: Vec::new(),
            preview_session_path: None,
            preview_search_matches: Vec::new(),
            preview_search_index: None,
            last_browser_nav_at: None,
            pending_preview_search_jump: None,
            browser_clipboard: None,
            last_browser_click: None,
            launch_codex_after_exit: None,
            remote_states: BTreeMap::new(),
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
    fn session_browser_line_uses_only_short_hash() {
        let s = SessionSummary {
            path: PathBuf::from("/tmp/a.jsonl"),
            storage_path: path_to_string(Path::new(&PathBuf::from("/tmp/a.jsonl"))),
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            file_name: String::from("rollout-a.jsonl"),
            id: String::from("123456789abcdef"),
            cwd: String::from("/tmp"),
            started_at: String::from("2026-01-01T00:00:00Z"),
            modified_epoch: 123,
            event_count: 42,
            user_message_count: 1,
            assistant_message_count: 1,
            search_blob: String::from("first user prompt"),
        };
        let line = format_session_browser_line(&s);
        assert_eq!(line, "9abcdef");
    }

    #[test]
    fn session_browser_line_marks_user_only_sessions() {
        let s = SessionSummary {
            path: PathBuf::from("/tmp/a.jsonl"),
            storage_path: path_to_string(Path::new(&PathBuf::from("/tmp/a.jsonl"))),
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            file_name: String::from("rollout-a.jsonl"),
            id: String::from("123456789abcdef"),
            cwd: String::from("/tmp"),
            started_at: String::from("2026-01-01T00:00:00Z"),
            modified_epoch: 123,
            event_count: 1,
            user_message_count: 3,
            assistant_message_count: 0,
            search_blob: String::from("first user prompt"),
        };
        let line = format_session_browser_line(&s);
        assert_eq!(line, "9abcdef !");
        assert!(is_user_only_session(&s));
    }

    #[test]
    fn preview_session_defers_follow_during_rapid_browser_navigation() {
        let mut app = empty_test_app();
        app.projects = vec![ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd: String::from("/repo"),
            sessions: vec![
                sample_session("/tmp/a.jsonl", "/repo", "aaaaaaa"),
                sample_session("/tmp/b.jsonl", "/repo", "bbbbbbb"),
            ],
        }];
        app.focus = Focus::Projects;
        app.browser_cursor = BrowserCursor::Session;
        app.session_idx = 1;
        app.preview_session_path = Some(PathBuf::from("/tmp/a.jsonl"));
        let now = Instant::now();
        app.last_browser_nav_at = Some(now);

        let preview = app
            .current_preview_session_at(now)
            .expect("preview session");
        assert_eq!(preview.id, "aaaaaaa");
    }

    #[test]
    fn pending_search_jump_overrides_browser_preview_debounce() {
        let mut app = empty_test_app();
        app.projects = vec![ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd: String::from("/repo"),
            sessions: vec![
                sample_session("/tmp/a.jsonl", "/repo", "aaaaaaa"),
                sample_session("/tmp/b.jsonl", "/repo", "bbbbbbb"),
            ],
        }];
        app.focus = Focus::Projects;
        app.browser_cursor = BrowserCursor::Session;
        app.session_idx = 1;
        app.preview_session_path = Some(PathBuf::from("/tmp/a.jsonl"));
        app.pending_preview_search_jump =
            Some((PathBuf::from("/tmp/b.jsonl"), String::from("bbb")));
        let now = Instant::now();
        app.last_browser_nav_at = Some(now);

        let preview = app
            .current_preview_session_at(now)
            .expect("preview session");
        assert_eq!(preview.id, "bbbbbbb");
    }

    #[test]
    fn preview_session_follows_selection_after_browser_navigation_settles() {
        let mut app = empty_test_app();
        app.projects = vec![ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd: String::from("/repo"),
            sessions: vec![
                sample_session("/tmp/a.jsonl", "/repo", "aaaaaaa"),
                sample_session("/tmp/b.jsonl", "/repo", "bbbbbbb"),
            ],
        }];
        app.focus = Focus::Projects;
        app.browser_cursor = BrowserCursor::Session;
        app.session_idx = 1;
        app.preview_session_path = Some(PathBuf::from("/tmp/a.jsonl"));
        let now = Instant::now();
        app.last_browser_nav_at = Some(now.checked_sub(Duration::from_millis(220)).unwrap_or(now));

        let preview = app
            .current_preview_session_at(now)
            .expect("preview session");
        assert_eq!(preview.id, "bbbbbbb");
    }

    #[test]
    fn preview_window_bounds_clamps_to_visible_slice() {
        assert_eq!(preview_window_bounds(100, 10, 20), (10, 30));
        assert_eq!(preview_window_bounds(100, 95, 20), (95, 100));
    }

    #[test]
    fn preview_window_bounds_handles_empty_content() {
        assert_eq!(preview_window_bounds(0, 0, 20), (0, 0));
        assert_eq!(preview_window_bounds(10, 0, 0), (0, 0));
    }

    #[test]
    fn browser_highlight_style_is_terminal_adaptive() {
        let style = browser_highlight_style();
        assert_eq!(style.fg, None);
        assert_eq!(style.bg, None);
        assert!(style.add_modifier.contains(Modifier::BOLD));
        assert!(style.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn human_timestamp_formats_readably() {
        assert_eq!(
            format_human_timestamp("2026-03-31T14:04:00Z"),
            "March 31, 2026 2:04PM"
        );
    }

    #[test]
    fn browser_display_path_shortens_root_prefix() {
        assert_eq!(
            browser_display_path("/root/gh/codex-session-tui"),
            "/gh/codex-session-tui"
        );
        assert_eq!(browser_display_path("/root"), "/root");
        assert_eq!(browser_display_path("/"), "/");
        assert_eq!(browser_display_path("/tmp/x"), "/tmp/x");
    }

    #[test]
    fn project_label_preserves_root_names() {
        let projects = vec![
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/root"),
                sessions: Vec::new(),
            },
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/"),
                sessions: Vec::new(),
            },
        ];
        assert_eq!(project_label(&projects, 0), "/root");
        assert_eq!(project_label(&projects, 1), "/");
    }

    #[test]
    fn coalesce_adjacent_turns_merges_same_role() {
        let turns = vec![
            ChatTurn {
                role: String::from("user"),
                timestamp: String::from("2026-01-01T00:00:00Z"),
                text: String::from("one"),
            },
            ChatTurn {
                role: String::from("user"),
                timestamp: String::from("2026-01-01T00:01:00Z"),
                text: String::from("two"),
            },
            ChatTurn {
                role: String::from("assistant"),
                timestamp: String::from("2026-01-01T00:02:00Z"),
                text: String::from("three"),
            },
        ];
        let merged = coalesce_chat_turns(&turns);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].text, "one\n\ntwo");
        assert_eq!(merged[0].timestamp, "2026-01-01T00:01:00Z");
    }

    #[test]
    fn default_folded_turns_collapses_assistant_and_first_user() {
        let turns = vec![
            ChatTurn {
                role: String::from("user"),
                timestamp: String::from("2026-01-01T00:00:00Z"),
                text: String::from("system-ish"),
            },
            ChatTurn {
                role: String::from("assistant"),
                timestamp: String::from("2026-01-01T00:01:00Z"),
                text: String::from("reply"),
            },
            ChatTurn {
                role: String::from("user"),
                timestamp: String::from("2026-01-01T00:02:00Z"),
                text: String::from("real user"),
            },
        ];
        let folded = default_folded_turns(&turns);
        assert!(folded.contains(&0));
        assert!(folded.contains(&1));
        assert!(!folded.contains(&2));
    }

    #[test]
    fn default_preview_scroll_opens_at_end() {
        assert_eq!(default_preview_scroll(120, 20), 100);
        assert_eq!(default_preview_scroll(10, 20), 0);
    }

    #[test]
    fn build_preview_marks_toned_rows() {
        let dir = std::env::temp_dir().join(format!("cse-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("sample.jsonl");
        fs::write(&path, sample_chat_jsonl()).expect("write");

        let session = SessionSummary {
            path: path.clone(),
            storage_path: path_to_string(Path::new(&path.clone())),
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            file_name: String::from("sample.jsonl"),
            id: String::from("abc"),
            cwd: String::from("/tmp/x"),
            started_at: String::from("2026-01-01T00:00:00Z"),
            modified_epoch: 123,
            event_count: 4,
            user_message_count: 2,
            assistant_message_count: 1,
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
            path: path.clone(),
            storage_path: path_to_string(&path),
            file_name: String::from("w.jsonl"),
            id: String::from("x"),
            cwd: String::from("/tmp"),
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            started_at: String::from("2026-01-01T00:00:00Z"),
            modified_epoch: 123,
            event_count: 2,
            user_message_count: 1,
            assistant_message_count: 0,
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
            path: path.clone(),
            storage_path: path_to_string(&path),
            file_name: String::from("all.jsonl"),
            id: String::from("x"),
            cwd: String::from("/tmp"),
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            started_at: String::from("2026-01-01T00:00:00Z"),
            modified_epoch: 123,
            event_count: 141,
            user_message_count: 140,
            assistant_message_count: 0,
            search_blob: String::new(),
        };
        let preview = build_preview(&s, PreviewMode::Chat, 60).expect("preview");
        assert_eq!(preview.header_rows.len(), 1);
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
            storage_path: path_to_string(Path::new(&PathBuf::from("/tmp/fold.jsonl"))),
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            file_name: String::from("fold.jsonl"),
            id: String::from("x"),
            cwd: String::from("/tmp"),
            started_at: String::from("2026-01-01T00:00:00Z"),
            modified_epoch: 123,
            event_count: 2,
            user_message_count: 1,
            assistant_message_count: 0,
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
    fn preview_body_omits_old_count_line_when_assistant_present() {
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
            storage_path: path_to_string(Path::new(&PathBuf::from("/tmp/c.jsonl"))),
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            file_name: String::from("c.jsonl"),
            id: String::from("x"),
            cwd: String::from("/tmp"),
            started_at: String::from("t0"),
            modified_epoch: 123,
            event_count: 2,
            user_message_count: 1,
            assistant_message_count: 1,
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
        assert!(!joined.contains("assistant=1"));
        assert!(!joined.contains("Warning: no assistant messages detected"));
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
    fn adjacent_assistant_turns_merge_into_single_block() {
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
            storage_path: path_to_string(Path::new(&PathBuf::from("/tmp/sep.jsonl"))),
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            file_name: String::from("sep.jsonl"),
            id: String::from("x"),
            cwd: String::from("/tmp"),
            started_at: String::from("t0"),
            modified_epoch: 123,
            event_count: 2,
            user_message_count: 0,
            assistant_message_count: 2,
            search_blob: String::new(),
        };
        let preview =
            build_preview_from_cached(&s, PreviewMode::Chat, 30, &cached, &HashSet::new());
        assert_eq!(preview.header_rows.len(), 1);
    }

    #[test]
    fn apply_search_filter_reduces_to_matching_sessions() {
        let s1 = SessionSummary {
            path: PathBuf::from("/tmp/a.jsonl"),
            storage_path: path_to_string(Path::new(&PathBuf::from("/tmp/a.jsonl"))),
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            file_name: String::from("a.jsonl"),
            id: String::from("a"),
            cwd: String::from("/repo/a"),
            started_at: String::from("2026-01-01T00:00:00Z"),
            modified_epoch: 123,
            event_count: 1,
            user_message_count: 1,
            assistant_message_count: 1,
            search_blob: String::from("deploy fix alpha"),
        };
        let s2 = SessionSummary {
            path: PathBuf::from("/tmp/b.jsonl"),
            storage_path: path_to_string(Path::new(&PathBuf::from("/tmp/b.jsonl"))),
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            file_name: String::from("b.jsonl"),
            id: String::from("b"),
            cwd: String::from("/repo/b"),
            started_at: String::from("2026-01-01T00:00:00Z"),
            modified_epoch: 122,
            event_count: 1,
            user_message_count: 1,
            assistant_message_count: 1,
            search_blob: String::from("unrelated text"),
        };

        let mut app = App {
            config_path: PathBuf::from("/tmp/codex-session-tui.toml"),
            config: AppConfig::default(),
            sessions_root: PathBuf::from("/tmp"),
            state_db_path: None,
            all_projects: vec![
                ProjectBucket {
                    machine_name: String::from("local"),
                    machine_target: None,
                    machine_codex_home: None,
                    machine_exec_prefix: None,
                    cwd: String::from("/repo/a"),
                    sessions: vec![s1],
                },
                ProjectBucket {
                    machine_name: String::from("local"),
                    machine_target: None,
                    machine_codex_home: None,
                    machine_exec_prefix: None,
                    cwd: String::from("/repo/b"),
                    sessions: vec![s2],
                },
            ],
            projects: Vec::new(),
            project_idx: 0,
            session_idx: 0,
            browser_cursor: BrowserCursor::Project,
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
            rendered_preview_cache: HashMap::new(),
            preview_folded: HashMap::new(),
            collapsed_projects: HashSet::new(),
            collapsed_groups: HashSet::new(),
            pinned_open_projects: HashSet::new(),
            selected_group_path: None,
            preview_header_rows: Vec::new(),
            preview_session_path: None,
            preview_search_matches: Vec::new(),
            preview_search_index: None,
            last_browser_nav_at: None,
            pending_preview_search_jump: None,
            browser_clipboard: None,
            last_browser_click: None,
            launch_codex_after_exit: None,
            remote_states: BTreeMap::new(),
        };

        app.apply_search_filter();
        assert_eq!(app.projects.len(), 1);
        assert_eq!(app.projects[0].cwd, "/repo/a");
        assert_eq!(app.browser_cursor, BrowserCursor::Session);
        assert_eq!(app.session_idx, 0);
        assert_eq!(
            app.pending_preview_search_jump,
            Some((PathBuf::from("/tmp/a.jsonl"), String::from("alpha")))
        );
        assert!(app.status.contains("1 session"));
    }

    #[test]
    fn apply_search_filter_orders_by_best_session_match_not_project_match_count() {
        let exact = SessionSummary {
            path: PathBuf::from("/tmp/exact.jsonl"),
            storage_path: path_to_string(Path::new(&PathBuf::from("/tmp/exact.jsonl"))),
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            file_name: String::from("exact.jsonl"),
            id: String::from("exact"),
            cwd: String::from("/repo/exact"),
            started_at: String::from("2026-01-01T00:00:00Z"),
            modified_epoch: 200,
            event_count: 1,
            user_message_count: 1,
            assistant_message_count: 1,
            search_blob: String::from("johyperr exact hit"),
        };
        let weak1 = SessionSummary {
            path: PathBuf::from("/tmp/weak1.jsonl"),
            storage_path: path_to_string(Path::new(&PathBuf::from("/tmp/weak1.jsonl"))),
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            file_name: String::from("weak1.jsonl"),
            id: String::from("weak1"),
            cwd: String::from("/repo/weak"),
            started_at: String::from("2026-01-01T00:00:00Z"),
            modified_epoch: 100,
            event_count: 1,
            user_message_count: 1,
            assistant_message_count: 1,
            search_blob: String::from("johyperr appears once"),
        };
        let weak2 = SessionSummary {
            path: PathBuf::from("/tmp/weak2.jsonl"),
            storage_path: path_to_string(Path::new(&PathBuf::from("/tmp/weak2.jsonl"))),
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            file_name: String::from("weak2.jsonl"),
            id: String::from("weak2"),
            cwd: String::from("/repo/weak"),
            started_at: String::from("2026-01-01T00:00:01Z"),
            modified_epoch: 99,
            event_count: 1,
            user_message_count: 1,
            assistant_message_count: 1,
            search_blob: String::from("another johyperr match"),
        };

        let mut app = empty_test_app();
        app.all_projects = vec![
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo/weak"),
                sessions: vec![weak1, weak2],
            },
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo/exact"),
                sessions: vec![exact],
            },
        ];
        app.search_query = String::from("johyperr");

        app.apply_search_filter();

        assert_eq!(app.projects[0].cwd, "/repo/exact");
        assert_eq!(app.browser_cursor, BrowserCursor::Session);
        assert_eq!(
            app.pending_preview_search_jump,
            Some((PathBuf::from("/tmp/exact.jsonl"), String::from("johyperr")))
        );
    }

    #[test]
    fn apply_search_filter_empty_collapses_all_projects() {
        let mut app = empty_test_app();
        app.all_projects = vec![
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo/a"),
                sessions: vec![sample_session("/tmp/a.jsonl", "/repo/a", "a")],
            },
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo/b"),
                sessions: vec![sample_session("/tmp/b.jsonl", "/repo/b", "b")],
            },
        ];
        app.search_query = String::new();

        app.apply_search_filter();

        assert_eq!(app.browser_cursor, BrowserCursor::Group);
        assert_eq!(app.selected_group_path.as_deref(), Some("local"));
        assert!(app.collapsed_projects.contains("/repo/a"));
        assert!(app.collapsed_projects.contains("/repo/b"));
        assert!(app.pinned_open_projects.is_empty());
        assert!(app.current_preview_session().is_none());
    }

    #[test]
    fn search_tab_moves_focus_out_of_search() {
        let mut app = empty_test_app();
        app.search_focused = true;

        let quit = handle_normal_mode(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &mut app)
            .expect("handle");
        assert!(!quit);
        assert!(!app.search_focused);
        assert_eq!(app.focus, Focus::Preview);
    }

    #[test]
    fn render_status_shows_search_onboarding_keys() {
        let mut app = empty_test_app();
        app.search_focused = true;
        app.search_query = String::from("johyperr");

        let backend = TestBackend::new(100, 4);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                render_status(
                    frame,
                    ratatui::layout::Rect {
                        x: 0,
                        y: 0,
                        width: 100,
                        height: 4,
                    },
                    &app,
                );
            })
            .expect("draw");

        let backend = terminal.backend();
        assert!(buffer_contains(backend, "enter"));
        assert!(buffer_contains(backend, "esc"));
        assert!(buffer_contains(backend, "tab"));
        assert!(buffer_contains(backend, "shift+tab"));
        assert!(buffer_contains(backend, "close search"));
    }

    #[test]
    fn render_status_shows_bulk_folder_shortcuts() {
        let mut app = empty_test_app();
        app.focus = Focus::Projects;
        app.browser_cursor = BrowserCursor::Project;

        let backend = TestBackend::new(120, 4);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                render_status(
                    frame,
                    ratatui::layout::Rect {
                        x: 0,
                        y: 0,
                        width: 120,
                        height: 4,
                    },
                    &app,
                );
            })
            .expect("draw");

        let backend = terminal.backend();
        assert!(buffer_contains(backend, "ctrl+←"));
        assert!(buffer_contains(backend, "collapse others"));
        assert!(buffer_contains(backend, "ctrl+→"));
        assert!(buffer_contains(backend, "expand all"));
        assert!(buffer_contains(backend, "ctrl+↑/↓"));
        assert!(buffer_contains(backend, "project jump"));
    }

    #[test]
    fn render_status_shows_refresh_shortcuts() {
        let app = empty_test_app();

        let backend = TestBackend::new(180, 4);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                render_status(
                    frame,
                    ratatui::layout::Rect {
                        x: 0,
                        y: 0,
                        width: 180,
                        height: 4,
                    },
                    &app,
                );
            })
            .expect("draw");

        let backend = terminal.backend();
        assert!(buffer_contains(backend, "f5"));
        assert!(buffer_contains(backend, "ctrl+r"));
        assert!(buffer_contains(backend, "refresh"));
    }

    #[test]
    fn f5_reloads_sessions() {
        let dir = std::env::temp_dir().join(format!("cse-refresh-f5-{}", Uuid::new_v4()));
        let sessions_root = dir.join("sessions");
        let source_path = sessions_root.join("2026/03/14/source.jsonl");
        write_test_session(&source_path, &sample_chat_jsonl());

        let mut app = empty_test_app();
        app.sessions_root = sessions_root;

        let quit = handle_normal_mode(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE), &mut app)
            .expect("handle");
        assert!(!quit);
        assert_eq!(app.projects.len(), 1);
        assert_eq!(app.projects[0].cwd, "/tmp/x");
    }

    #[test]
    fn ctrl_r_reloads_sessions() {
        let dir = std::env::temp_dir().join(format!("cse-refresh-ctrlr-{}", Uuid::new_v4()));
        let sessions_root = dir.join("sessions");
        let source_path = sessions_root.join("2026/03/14/source.jsonl");
        write_test_session(&source_path, &sample_chat_jsonl());

        let mut app = empty_test_app();
        app.sessions_root = sessions_root;

        let quit = handle_normal_mode(
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
            &mut app,
        )
        .expect("handle");
        assert!(!quit);
        assert_eq!(app.projects.len(), 1);
        assert_eq!(app.projects[0].cwd, "/tmp/x");
    }

    #[test]
    fn render_status_shows_export_shortcut_for_session_rows() {
        let mut app = empty_test_app();
        app.focus = Focus::Projects;
        app.browser_cursor = BrowserCursor::Session;

        let backend = TestBackend::new(140, 4);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                render_status(
                    frame,
                    ratatui::layout::Rect {
                        x: 0,
                        y: 0,
                        width: 140,
                        height: 4,
                    },
                    &app,
                );
            })
            .expect("draw");

        let backend = terminal.backend();
        assert!(buffer_contains(backend, "export ssh"));
        assert!(buffer_contains(backend, "e"));
    }

    #[test]
    fn mouse_click_browser_row_matches_row_mapping() {
        let mut app = empty_test_app();
        app.projects = vec![
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo-a"),
                sessions: vec![
                    sample_session("/tmp/a1.jsonl", "/repo-a", "a1"),
                    sample_session("/tmp/a2.jsonl", "/repo-a", "a2"),
                ],
            },
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo-b"),
                sessions: vec![sample_session("/tmp/b1.jsonl", "/repo-b", "b1")],
            },
        ];
        app.panes.browser = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        };

        let rows = app.browser_rows();
        let target = rows
            .iter()
            .find(|row| matches!(row.kind, BrowserRowKind::Session { .. }))
            .cloned()
            .expect("session row");
        handle_mouse_event(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 3,
                row: 4,
                modifiers: KeyModifiers::NONE,
            },
            &mut app,
        );

        match target.kind {
            BrowserRowKind::Session {
                project_idx,
                session_idx,
            } => {
                assert_eq!(app.project_idx, project_idx);
                assert_eq!(app.session_idx, session_idx);
                assert_eq!(app.browser_cursor, BrowserCursor::Session);
            }
            _ => panic!("expected session row"),
        }
    }

    #[test]
    fn render_search_shows_cursor_when_focused() {
        let mut app = empty_test_app();
        app.search_focused = true;
        app.search_query = String::from("johyperr");

        let backend = TestBackend::new(40, 3);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                render_search(
                    frame,
                    ratatui::layout::Rect {
                        x: 0,
                        y: 0,
                        width: 40,
                        height: 3,
                    },
                    &app,
                );
            })
            .expect("draw");

        assert!(buffer_contains(terminal.backend(), "johyperr"));
        assert!(buffer_contains(terminal.backend(), "█"));
    }

    #[test]
    fn preview_match_row_finds_first_matching_line() {
        let preview = PreviewData {
            lines: vec![
                Line::from("alpha"),
                Line::from("hello johyperr world"),
                Line::from("omega"),
            ],
            tone_rows: Vec::new(),
            header_rows: vec![(1, 0)],
            block_ranges: vec![(0, 1, 1)],
        };

        assert_eq!(preview_match_row(&preview, "johyperr"), Some(1));
        assert_eq!(
            preview_turn_at_or_before_row(&preview.header_rows, 1),
            Some(0)
        );
    }

    #[test]
    fn render_preview_applies_search_highlight_overlay() {
        let dir = std::env::temp_dir().join(format!("cse-preview-highlight-{}", Uuid::new_v4()));
        let path = dir.join("sample.jsonl");
        let body = [
            r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":"abc","timestamp":"2026-01-01T00:00:00Z","cwd":"/tmp/x"}}"#,
            r#"{"timestamp":"2026-01-01T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello johyperr world"}]}}"#,
        ]
        .join("\n");
        write_test_session(&path, &body);

        let mut app = empty_test_app();
        app.projects = vec![ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd: String::from("/tmp/x"),
            sessions: vec![SessionSummary {
                path: path.clone(),
                storage_path: path_to_string(Path::new(&path.clone())),
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                file_name: String::from("sample.jsonl"),
                id: String::from("abcdef1"),
                cwd: String::from("/tmp/x"),
                started_at: String::from("2026-01-01T00:00:00Z"),
                modified_epoch: 1,
                event_count: 2,
                user_message_count: 1,
                assistant_message_count: 0,
                search_blob: String::from("hello johyperr world"),
            }],
        }];
        app.browser_cursor = BrowserCursor::Session;
        app.search_query = String::from("johyperr");
        app.preview_folded.insert(path.clone(), HashSet::new());
        app.panes.preview = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 60,
            height: 12,
        };

        let backend = TestBackend::new(60, 12);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                render_preview(
                    frame,
                    ratatui::layout::Rect {
                        x: 0,
                        y: 0,
                        width: 60,
                        height: 12,
                    },
                    &mut app,
                );
            })
            .expect("draw");

        let backend = terminal.backend();
        assert!(buffer_contains(backend, "johyperr"));
        let area = backend.buffer().area;
        let mut highlighted = false;
        for y in 0..area.height {
            let line = (0..area.width)
                .map(|x| backend.buffer()[(x, y)].symbol().to_string())
                .collect::<Vec<_>>()
                .join("");
            if let Some(start) = line.find("johyperr") {
                let mut any_highlight = false;
                for x in start as u16..(start + "johyperr".len()) as u16 {
                    let cell = &backend.buffer()[(x, y)];
                    if cell.modifier.contains(Modifier::UNDERLINED)
                        || cell.modifier.contains(Modifier::REVERSED)
                    {
                        any_highlight = true;
                    }
                }
                highlighted = any_highlight;
                if highlighted {
                    break;
                }
            }
        }
        assert!(highlighted);
    }

    #[test]
    fn render_preview_title_shows_total_message_counts() {
        let dir = std::env::temp_dir().join(format!("cse-preview-title-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("sample.jsonl");
        fs::write(&path, sample_chat_jsonl()).expect("write");

        let session = SessionSummary {
            path: path.clone(),
            storage_path: path_to_string(Path::new(&path.clone())),
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            file_name: String::from("sample.jsonl"),
            id: String::from("abcdef123456"),
            cwd: String::from("/tmp/x"),
            started_at: String::from("2026-01-01T00:00:00Z"),
            modified_epoch: 123,
            event_count: 4,
            user_message_count: 2,
            assistant_message_count: 1,
            search_blob: String::from("hello world normalized user"),
        };
        let mut app = empty_test_app();
        app.projects = vec![ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd: String::from("/tmp/x"),
            sessions: vec![session],
        }];
        app.focus = Focus::Preview;
        app.browser_cursor = BrowserCursor::Session;

        let backend = TestBackend::new(100, 20);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                render_preview(
                    frame,
                    ratatui::layout::Rect {
                        x: 0,
                        y: 0,
                        width: 100,
                        height: 20,
                    },
                    &mut app,
                );
            })
            .expect("draw");

        let backend = terminal.backend();
        assert!(buffer_contains(backend, "user=2 assistant=1"));
    }

    #[test]
    fn render_preview_title_marks_user_only_session() {
        let dir = std::env::temp_dir().join(format!("cse-preview-user-only-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("sample.jsonl");
        let body = [
            r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":"abcdef123456","timestamp":"2026-01-01T00:00:00Z","cwd":"/tmp/x"}}"#,
            r#"{"timestamp":"2026-01-01T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello world"}]}}"#,
        ]
        .join("\n");
        fs::write(&path, body).expect("write");

        let session = SessionSummary {
            path: path.clone(),
            storage_path: path_to_string(Path::new(&path.clone())),
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            file_name: String::from("sample.jsonl"),
            id: String::from("abcdef123456"),
            cwd: String::from("/tmp/x"),
            started_at: String::from("2026-01-01T00:00:00Z"),
            modified_epoch: 123,
            event_count: 2,
            user_message_count: 1,
            assistant_message_count: 0,
            search_blob: String::from("hello world"),
        };
        let mut app = empty_test_app();
        app.projects = vec![ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd: String::from("/tmp/x"),
            sessions: vec![session],
        }];
        app.browser_cursor = BrowserCursor::Session;
        app.preview_folded.insert(path, HashSet::new());

        let backend = TestBackend::new(120, 20);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                render_preview(
                    frame,
                    ratatui::layout::Rect {
                        x: 0,
                        y: 0,
                        width: 120,
                        height: 20,
                    },
                    &mut app,
                );
            })
            .expect("draw");

        let backend = terminal.backend();
        assert!(buffer_contains(
            backend,
            "user-only; may not resume in codex"
        ));
        assert!(app.status.contains("may not resume"));
    }

    #[test]
    fn render_preview_shows_no_session_selected_on_project_row() {
        let mut app = empty_test_app();
        app.projects = vec![ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd: String::from("/repo"),
            sessions: vec![sample_session("/tmp/a.jsonl", "/repo", "a")],
        }];
        app.browser_cursor = BrowserCursor::Project;
        app.focus = Focus::Projects;

        let backend = TestBackend::new(80, 12);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                render_preview(
                    frame,
                    ratatui::layout::Rect {
                        x: 0,
                        y: 0,
                        width: 80,
                        height: 12,
                    },
                    &mut app,
                );
            })
            .expect("draw");

        let backend = terminal.backend();
        assert!(buffer_contains(backend, "No session selected"));
    }

    #[test]
    fn highlight_ranges_returns_character_offsets() {
        assert_eq!(
            highlight_ranges("hello johyperr world", "johyperr"),
            vec![(6, 14)]
        );
        assert_eq!(
            highlight_ranges("alpha beta alpha", "alpha"),
            vec![(0, 5), (11, 16)]
        );
    }

    #[test]
    fn preview_toggle_all_folds_collapses_and_expands() {
        let mut app = App {
            config_path: PathBuf::from("/tmp/codex-session-tui.toml"),
            config: AppConfig::default(),
            sessions_root: PathBuf::from("/tmp"),
            state_db_path: None,
            all_projects: Vec::new(),
            projects: Vec::new(),
            project_idx: 0,
            session_idx: 0,
            browser_cursor: BrowserCursor::Project,
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
            rendered_preview_cache: HashMap::new(),
            preview_folded: HashMap::new(),
            collapsed_projects: HashSet::new(),
            collapsed_groups: HashSet::new(),
            pinned_open_projects: HashSet::new(),
            selected_group_path: None,
            preview_header_rows: vec![(10, 0), (20, 1), (30, 2)],
            preview_session_path: Some(PathBuf::from("/tmp/x.jsonl")),
            preview_search_matches: Vec::new(),
            preview_search_index: None,
            last_browser_nav_at: None,
            pending_preview_search_jump: None,
            browser_clipboard: None,
            last_browser_click: None,
            launch_codex_after_exit: None,
            remote_states: BTreeMap::new(),
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

    #[test]
    fn parse_config_machine_input_accepts_default_and_custom_codex_home() {
        let bare = parse_config_machine_input("pi@192.168.0.12").expect("bare");
        assert_eq!(bare.name, "192.168.0.12");
        assert_eq!(bare.ssh_target, "pi@192.168.0.12");
        assert_eq!(bare.codex_home, None);
        assert_eq!(bare.exec_prefix, None);

        let plain = parse_config_machine_input("pi=pi@192.168.0.12").expect("plain");
        assert_eq!(plain.name, "pi");
        assert_eq!(plain.ssh_target, "pi@192.168.0.12");
        assert_eq!(plain.codex_home, None);
        assert_eq!(plain.exec_prefix, None);

        let custom = parse_config_machine_input("lab=pi@192.168.0.13:/home/pi/custom-codex")
            .expect("custom");
        assert_eq!(custom.name, "lab");
        assert_eq!(custom.ssh_target, "pi@192.168.0.13");
        assert_eq!(custom.codex_home.as_deref(), Some("/home/pi/custom-codex"));
        assert_eq!(custom.exec_prefix, None);
    }

    #[test]
    fn parse_config_machine_input_accepts_exec_prefix() {
        let machine =
            parse_config_machine_input("dev=root@example-host|lxc-attach -n dev --|/root/.codex")
                .expect("machine");
        assert_eq!(machine.name, "dev");
        assert_eq!(machine.ssh_target, "root@example-host");
        assert_eq!(machine.exec_prefix.as_deref(), Some("lxc-attach -n dev --"));
        assert_eq!(machine.codex_home.as_deref(), Some("/root/.codex"));
    }

    #[test]
    fn wrap_remote_exec_supports_container_prefix() {
        let wrapped = wrap_remote_exec(Some("lxc-attach -n dev --"), "python3 - /root/.codex");
        assert!(wrapped.contains("lxc-attach -n dev -- sh -lc"));
        assert!(wrapped.contains("python3 - /root/.codex"));
    }

    #[test]
    fn upsert_config_machine_renames_existing_endpoint() {
        let mut config = AppConfig {
            machines: vec![ConfigMachine {
                name: String::from("old"),
                ssh_target: String::from("root@example-host"),
                exec_prefix: Some(String::from("lxc-attach -n dev --")),
                codex_home: Some(String::from("/root/.codex")),
            }],
        };
        upsert_config_machine(
            &mut config,
            ConfigMachine {
                name: String::from("dev"),
                ssh_target: String::from("root@example-host"),
                exec_prefix: Some(String::from("lxc-attach -n dev --")),
                codex_home: Some(String::from("/root/.codex")),
            },
        );

        assert_eq!(config.machines.len(), 1);
        assert_eq!(config.machines[0].name, "dev");
    }

    #[test]
    fn handle_input_mode_enter_keeps_tui_alive_on_invalid_remote_input() {
        let mut app = empty_test_app();
        app.start_action(Action::AddRemote);
        app.input = String::from("bad=|");

        handle_input_mode(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut app)
            .expect("input handling");

        assert_eq!(app.mode, Mode::Input);
        assert!(app.status.contains("remote"));
    }

    #[test]
    fn scan_remote_machine_reuses_recent_cache() {
        let app = empty_test_app();
        let machine = ConfigMachine {
            name: String::from("pi"),
            ssh_target: String::from("pi@192.168.0.20"),
            exec_prefix: None,
            codex_home: Some(String::from("/home/pi/.codex")),
        };
        let previous = RemoteMachineState {
            status: RemoteMachineStatus::Cached,
            last_error: Some(String::from("timed out")),
            cached_projects: vec![ProjectBucket {
                machine_name: String::from("pi"),
                machine_target: Some(String::from("pi@192.168.0.20")),
                machine_codex_home: Some(String::from("/home/pi/.codex")),
                machine_exec_prefix: None,
                cwd: String::from("/remote/repo"),
                sessions: vec![sample_session(
                    "/tmp/remote.jsonl",
                    "/remote/repo",
                    "abc1234",
                )],
            }],
            last_scan_at: Some(Instant::now()),
        };

        let next = app.scan_remote_machine(&machine, &previous, false);
        assert_eq!(next.status, RemoteMachineStatus::Cached);
        assert_eq!(next.cached_projects.len(), 1);
        assert_eq!(next.cached_projects[0].cwd, "/remote/repo");
    }

    #[test]
    fn browser_rows_include_configured_remote_without_projects() {
        let mut app = empty_test_app();
        app.config.machines.push(ConfigMachine {
            name: String::from("dev"),
            ssh_target: String::from("root@example-host"),
            exec_prefix: Some(String::from("lxc-attach -n dev --")),
            codex_home: Some(String::from("/root/.codex")),
        });

        let labels = app
            .browser_render_rows()
            .into_iter()
            .map(|row| row.label)
            .collect::<Vec<_>>();
        assert!(labels.contains(&String::from("dev")));
    }

    #[test]
    fn browser_tree_segments_normalize_double_leading_slash() {
        assert_eq!(
            browser_tree_segments("//home/pi"),
            vec![String::from("/"), String::from("home"), String::from("pi")]
        );
    }

    #[test]
    fn collapse_all_projects_expands_first_machine_and_first_folder() {
        let mut app = empty_test_app();
        app.config.machines.push(ConfigMachine {
            name: String::from("pi"),
            ssh_target: String::from("pi@192.168.0.20"),
            exec_prefix: None,
            codex_home: Some(String::from("/home/pi/.codex")),
        });
        app.projects = vec![
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/repo/a"),
                sessions: vec![sample_session("/tmp/a.jsonl", "/repo/a", "a")],
            },
            ProjectBucket {
                machine_name: String::from("pi"),
                machine_target: Some(String::from("pi@192.168.0.20")),
                machine_codex_home: Some(String::from("/home/pi/.codex")),
                machine_exec_prefix: None,
                cwd: String::from("/remote/repo"),
                sessions: vec![sample_session("/tmp/b.jsonl", "/remote/repo", "b")],
            },
        ];

        app.collapse_all_projects();

        assert_eq!(app.browser_cursor, BrowserCursor::Group);
        assert_eq!(app.selected_group_path.as_deref(), Some("local"));
        assert!(!app.collapsed_groups.contains("local"));
        assert!(!app.collapsed_groups.contains("local/"));
        assert!(app.collapsed_groups.contains("pi"));
    }

    #[test]
    fn resolve_machine_target_supports_machine_prefixed_paths() {
        let mut app = empty_test_app();
        app.config.machines.push(ConfigMachine {
            name: String::from("pi"),
            ssh_target: String::from("pi@192.168.0.20"),
            codex_home: Some(String::from("/home/pi/.codex")),
            exec_prefix: None,
        });

        let target = app.resolve_machine_target("pi:/work/repo").expect("target");
        assert_eq!(target.name, "pi");
        assert_eq!(target.ssh_target.as_deref(), Some("pi@192.168.0.20"));
        assert_eq!(target.cwd, "/work/repo");
    }

    #[test]
    fn browser_tree_groups_common_parent_segments() {
        let mut app = empty_test_app();
        app.projects = vec![
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/root/git/this"),
                sessions: vec![sample_session("/tmp/this.jsonl", "/root/git/this", "this")],
            },
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/root/git/that"),
                sessions: vec![sample_session("/tmp/that.jsonl", "/root/git/that", "that")],
            },
            ProjectBucket {
                machine_name: String::from("local"),
                machine_target: None,
                machine_codex_home: None,
                machine_exec_prefix: None,
                cwd: String::from("/root/misc"),
                sessions: vec![sample_session("/tmp/misc.jsonl", "/root/misc", "misc")],
            },
        ];
        app.collapse_all_projects();
        app.collapsed_groups.clear();
        app.collapsed_projects.clear();

        let labels = app
            .browser_render_rows()
            .into_iter()
            .filter(|row| !matches!(row.kind, BrowserRowKind::Session { .. }))
            .map(|row| row.label)
            .collect::<Vec<_>>();

        assert_eq!(
            labels,
            vec!["local", "/root", "git", "that", "this", "misc"]
        );
    }

    #[test]
    fn machine_status_suffixes_render_for_browser_roots() {
        assert_eq!(machine_status_suffix(RemoteMachineStatus::Healthy), "[ok]");
        assert_eq!(
            machine_status_suffix(RemoteMachineStatus::Cached),
            "[cached]"
        );
        assert_eq!(
            machine_status_suffix(RemoteMachineStatus::Error),
            "[offline]"
        );
    }

    #[test]
    fn render_browser_shows_machine_health_suffix() {
        let mut app = empty_test_app();
        app.projects = vec![ProjectBucket {
            machine_name: String::from("pi"),
            machine_target: Some(String::from("pi@192.168.0.20")),
            machine_codex_home: Some(String::from("/home/pi/.codex")),
            machine_exec_prefix: None,
            cwd: String::from("/remote/repo"),
            sessions: vec![sample_session(
                "/tmp/remote.jsonl",
                "/remote/repo",
                "abc1234",
            )],
        }];
        app.collapsed_groups.clear();
        app.collapsed_projects.clear();
        app.remote_states.insert(
            String::from("pi"),
            RemoteMachineState {
                status: RemoteMachineStatus::Error,
                last_error: Some(String::from("timed out")),
                cached_projects: Vec::new(),
                last_scan_at: Some(Instant::now()),
            },
        );

        let backend = TestBackend::new(80, 12);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                render_browser(
                    frame,
                    ratatui::layout::Rect {
                        x: 0,
                        y: 0,
                        width: 80,
                        height: 12,
                    },
                    &app,
                );
            })
            .expect("draw");
        assert!(buffer_contains(terminal.backend(), "[offline]"));
    }

    #[test]
    fn browser_tree_shows_sessions_under_project_leaf_only() {
        let mut app = empty_test_app();
        app.projects = vec![ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd: String::from("/root/git/this"),
            sessions: vec![sample_session(
                "/tmp/this.jsonl",
                "/root/git/this",
                "abcdef012345",
            )],
        }];
        app.collapsed_projects.clear();
        app.collapsed_groups.clear();

        let rows = app.browser_render_rows();
        assert_eq!(rows[0].label, "local");
        assert_eq!(rows[1].label, "/root/git/this");
        assert_eq!(rows[2].label, "f012345");
    }

    #[test]
    fn preview_match_positions_marks_primary_and_secondary_occurrences() {
        let preview = PreviewData {
            lines: vec![
                Line::from("alpha johyperr"),
                Line::from("beta johyperr"),
                Line::from("gamma"),
            ],
            tone_rows: Vec::new(),
            header_rows: vec![(0, 0), (1, 1)],
            block_ranges: vec![(0, 0, 0), (1, 1, 1)],
        };

        let matches = preview_match_positions(&preview, "johyperr");
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].row, 0);
        assert!(matches[0].is_primary);
        assert_eq!(matches[1].row, 1);
        assert!(!matches[1].is_primary);
    }

    #[test]
    fn preview_search_navigation_moves_between_occurrences() {
        let mut app = empty_test_app();
        app.preview_search_matches = vec![
            PreviewMatch {
                row: 10,
                col_start: 3,
                col_end: 11,
                is_primary: true,
            },
            PreviewMatch {
                row: 25,
                col_start: 1,
                col_end: 9,
                is_primary: false,
            },
        ];
        app.preview_search_index = Some(0);
        app.panes.preview.height = 12;

        app.focus_next_preview_search_match();
        assert_eq!(app.preview_search_index, Some(1));
        assert_eq!(app.preview_scroll, 22);

        app.focus_prev_preview_search_match();
        assert_eq!(app.preview_search_index, Some(0));
        assert_eq!(app.preview_scroll, 7);
    }

    #[test]
    fn page_navigation_scrolls_preview_by_view_height() {
        let mut app = empty_test_app();
        app.focus = Focus::Preview;
        app.preview_content_len = 200;
        app.preview_scroll = 50;
        app.panes.preview.height = 20;

        app.page_preview(1);
        assert_eq!(app.preview_scroll, 67);

        app.page_preview(-1);
        assert_eq!(app.preview_scroll, 50);
    }

    #[test]
    fn home_end_navigation_jumps_preview_bounds() {
        let mut app = empty_test_app();
        app.focus = Focus::Preview;
        app.preview_content_len = 200;
        app.panes.preview.height = 20;
        app.preview_scroll = 50;

        app.jump_preview_to_edge(false);
        assert_eq!(app.preview_scroll, 182);

        app.jump_preview_to_edge(true);
        assert_eq!(app.preview_scroll, 0);
    }

    #[test]
    fn preview_header_shows_full_session_id() {
        let dir = std::env::temp_dir().join(format!("cse-preview-full-id-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("sample.jsonl");
        fs::write(&path, sample_chat_jsonl()).expect("write");

        let session = SessionSummary {
            path: path.clone(),
            storage_path: path_to_string(Path::new(&path.clone())),
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            file_name: String::from("sample.jsonl"),
            id: String::from("abcdef1234567890"),
            cwd: String::from("/tmp/x"),
            started_at: String::from("2026-01-01T00:00:00Z"),
            modified_epoch: 123,
            event_count: 4,
            user_message_count: 2,
            assistant_message_count: 1,
            search_blob: String::from("hello world"),
        };
        let mut app = empty_test_app();
        app.projects = vec![ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd: String::from("/tmp/x"),
            sessions: vec![session],
        }];
        app.browser_cursor = BrowserCursor::Session;

        let backend = TestBackend::new(120, 20);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                render_preview(
                    frame,
                    ratatui::layout::Rect {
                        x: 0,
                        y: 0,
                        width: 120,
                        height: 20,
                    },
                    &mut app,
                );
            })
            .expect("draw");

        assert!(buffer_contains(terminal.backend(), "abcdef1234567890"));
    }

    #[test]
    fn codex_launch_spec_uses_current_session_id_and_cwd() {
        let mut app = empty_test_app();
        app.projects = vec![ProjectBucket {
            machine_name: String::from("local"),
            machine_target: None,
            machine_codex_home: None,
            machine_exec_prefix: None,
            cwd: String::from("/tmp/work"),
            sessions: vec![sample_session("/tmp/a.jsonl", "/tmp/work", "abcdef123456")],
        }];
        app.browser_cursor = BrowserCursor::Session;

        let launch = app.plan_open_current_session_in_codex().expect("launch");
        assert_eq!(launch.cwd, PathBuf::from("/tmp/work"));
        assert_eq!(launch.session_id, String::from("abcdef123456"));
    }
}
