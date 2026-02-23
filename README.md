# codex-session-explorer

`codex-session-explorer` is a Rust TUI for inspecting and repairing Codex sessions under `~/.codex/sessions`.

## Motivation

`codex resume` groups sessions by the stored `cwd`.  
If a repository path changes (rename/move), old sessions still reference the old path and become hard to discover.

This tool exists to recover and remap those sessions safely.

## Features

- Scans `${CODEX_HOME:-~/.codex}/sessions` recursively.
- Parses JSONL session files and groups sessions by recorded `cwd`.
- 3-pane TUI:
  - Projects (`cwd`)
  - Sessions
  - Preview (chat/events)
- Session operations:
  - `Move`: rewrite `cwd` in-place (backup + atomic write)
  - `Copy`: duplicate to a new session file with a new `cwd`
  - `Fork`: duplicate with new id/timestamp + new `cwd`
  - Project-scope `Rename Folder`: rewrite `cwd` for all sessions in a project
  - Project-scope `Copy Folder`: duplicate all sessions in a project to a new `cwd`
- Foldable preview blocks with keyboard navigation.
- Fuzzy search over session metadata/content.
- Multi-select sessions (`Space` toggle, mouse checkbox click, select-all/invert).
- Mouse QoL:
  - drag splitters to resize panes
  - drag scrollbars to jump/scrub
  - drag-select preview text (character-accurate) and copy via OSC52

## Controls

- `Tab`: focus next pane (or toggle focused preview block when Preview is focused)
- `Shift+Tab`: focus previous pane (or toggle fold-all in Preview)
- `↑/↓` (`j/k`): navigate; in Preview, move focused block
- `←/→`: fold/unfold focused preview block
- `/`: open search
- `v`: toggle preview mode (`chat` / `events`)
- `h/l`: resize focused pane
- `Space` (Sessions pane): toggle session selection
- checkbox click (Sessions pane): toggle selection on that session
- `a` (Sessions pane): select all sessions in current project
- `i` (Sessions pane): invert selection in current project
- `m/c/f` (Sessions/Preview focus): move/copy/fork selected sessions (or current session)
- `m` or `r` (Projects focus): rename folder sessions
- `c` or `y` (Projects focus): copy folder sessions
- `g`: refresh
- `Esc`: cancel input
- `q`: quit

Mouse:
- click projects/sessions to select
- click preview block to fold/unfold
- drag in preview to select text and auto-copy (OSC52)
- wheel scroll on hovered pane
- drag splitters and scrollbars

## Build

```bash
cargo run
```

Use a different Codex home:

```bash
CODEX_HOME=/path/to/.codex cargo run
```

## CI and Releases

GitHub Actions builds release binaries for:

- Linux: `x86_64`, `aarch64`
- macOS: `x86_64`, `aarch64`
- Windows (MSVC): `x86_64`, `aarch64`

Workflow file:

- `.github/workflows/build-and-package.yml`

It also assembles an npm package layout for publishing under:

- `@avikalpa/codex-session-explorer`

## Safety

- Rewrites create a timestamped backup first.
- All writes use temp-file + rename.
- Unknown JSON fields are preserved.
