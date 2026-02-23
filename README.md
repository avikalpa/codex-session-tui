# codex-session-explorer

A terminal UI (TUI) for exploring and repairing Codex session mappings in `~/.codex`.

## Motivation

Codex sessions are grouped in `codex resume` by the working directory (`cwd`) recorded in each session file.
When a project folder is renamed or moved, those old sessions still point to the original path and can feel "lost" from normal resume workflows.

This project exists to make those sessions visible and editable so you can:
- browse sessions by project-like `cwd`
- inspect session metadata and chat/event previews
- move, copy, or fork conversations from one folder context to another

## What it does

- Scans `~/.codex/sessions` (or `$CODEX_HOME/sessions`) recursively.
- Reads JSONL session files and groups them by recorded `cwd`.
- Provides a 3-pane, keyboard-first TUI:
  - left: project paths (`cwd`) and counts
  - center: sessions in a pretty 2-line layout (timestamp/events + id/file)
  - right: parsed chat transcript preview (user/assistant turns) with wrapping/reflow
- Uses terminal-adaptive styling (no hardcoded pane backgrounds) for better theme compatibility.
- Shows thin scrollbars in project, session, and preview panes when content exceeds viewport.
- Supports actions on selected session:
  - `m` Move: rewrite `cwd` fields in-place (creates backup first)
  - `c` Copy: duplicate session file to current date bucket with new target `cwd`
  - `f` Fork: duplicate + regenerate session id/timestamp + target `cwd`

## Controls

- `Tab` / `Shift+Tab`: switch focus between projects, sessions, and preview
- `j` / `k` or arrow keys: navigate
- `/`: focus search bar (type to filter sessions by conversation/path/id)
- `v`: toggle preview mode (`chat` / `events`)
- `z`: toggle fold for the next visible preview block header
- `H` / `L`: resize focused pane width
- `m`: move selected session to a target path
- `c`: copy selected session to a target path
- `f`: fork selected session to a target path
- `g`: refresh from disk
- `Esc`: cancel action input
- `q`: quit

Mouse:
- left click: select project/session, focus preview
- left click on a preview block header: fold/unfold that block
- wheel: scroll selection in lists or scroll chat preview
- drag pane splitters: resize pane widths interactively
- status buttons: click `Move/Copy/Fork/Refresh`; in input mode click `Apply/Cancel`

## Build and run

```bash
cargo run
```

Optional:

```bash
CODEX_HOME=/some/other/.codex cargo run
```

## Safety notes

- Move action creates a timestamped backup next to the original file before rewrite.
- Writes are atomic (`.tmp` write then rename).
- This tool edits real session files. Test with a copied `CODEX_HOME` first if you want a dry environment.
