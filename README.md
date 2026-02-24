# codex-session-tui

`npx -y codex-session-tui`

## Demo

[![asciinema demo](https://asciinema.org/a/Du2tYavIkMCoXLvI.svg)](https://asciinema.org/a/Du2tYavIkMCoXLvI)

## Features

- Parse/session-index JSONL files from `${CODEX_HOME:-~/.codex}/sessions`
- Group sessions by project `cwd`
- 3-pane TUI: Projects, Sessions, Preview
- Operations: move, copy, fork, delete, project-folder rename/copy
- Multi-select sessions and bulk operations
- Search/filter, foldable preview blocks, mouse selection/copy, draggable splitters/scrollbars

## Motivation

`codex resume` groups sessions by stored `cwd`.  
After a repo/folder move, old sessions can become hard to discover.

## Why

`codex-session-tui` is a Rust TUI to inspect and remap Codex sessions safely.

## Quickstart

Install globally:

```bash
npm i -g codex-session-tui
codex-session-tui
```

Use a different Codex home:

```bash
CODEX_HOME=/path/to/.codex codex-session-tui
```

or with `npx`:

```bash
CODEX_HOME=/path/to/.codex npx -y codex-session-tui
```

## Dev Run

```bash
cargo run
```

```bash
CODEX_HOME=/path/to/.codex cargo run
```

## Safety

- Backups are created before mutating/deleting session files
- Writes use atomic temp-file + rename
- Unknown JSON fields are preserved

Backup location and restore:

- Backups are created next to the original session file under `${CODEX_HOME:-~/.codex}/sessions`
- Backup filename format: `<original>.jsonl.bak.YYYYMMDDHHMMSS`

Find backups:

```bash
find "${CODEX_HOME:-$HOME/.codex}/sessions" -type f -name "*.jsonl.bak.*"
```

Restore a backup:

```bash
cp "/path/to/rollout-....jsonl.bak.20260224101530" "/path/to/rollout-....jsonl"
```
