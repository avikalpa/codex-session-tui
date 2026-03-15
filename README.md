# codex-session-tui

`codex-session-tui` is a terminal-first session explorer for Codex.

It gives you a VS Code style workflow for `~/.codex`: a browser tree on the left, a rich preview on the right, searchable conversations, foldable blocks, multi-select, drag and drop, and cross-machine session management.

If `codex resume` stops finding conversations after a repo move, folder rename, machine migration, or container hop, this tool is built to recover that state safely.

## Install

Run directly:

```bash
npx -y codex-session-tui@latest
```

Install globally:

```bash
npm i -g codex-session-tui
codex-session-tui
```

Use a different Codex home:

```bash
CODEX_HOME=/path/to/.codex codex-session-tui
```

## Demo

[![asciinema major release demo](https://asciinema.org/a/3c0x6ukgrqpRCL4a.svg)](https://asciinema.org/a/3c0x6ukgrqpRCL4a)

## Why This Exists

Codex sessions are tied to recorded `cwd` values.

That means:

- rename a repository and sessions can disappear from `codex resume`
- move work to another disk or mountpoint and sessions can look lost
- shift between local, SSH, and container environments and session continuity breaks

`codex-session-tui` fixes that workflow:

- browse sessions as conversations, not raw JSON
- search by text, path, and hash
- move, copy, fork, or export sessions into a new project context
- repair stale `cwd` mappings so Codex can see the sessions again
- work across local and remote machines from one browser

## Why It Feels Better

This is not a file dumper. It is meant to feel like an editor/workspace browser:

- GitHub-style grouped folder tree
- VS Code style left-browser plus right-preview workflow
- discoverable status bar shortcuts
- mouse support for splitters, scrolling, folding, selection, and drag/drop
- multi-machine browsing with health indicators
- direct open-in-Codex flow from the current session

## Key Features

- Unified browser for `local` plus SSH-connected remotes
- Grouped project tree with compressed single-child folder chains
- Session list ordered by recent activity
- Rich preview with markdown rendering, foldable blocks, timestamps, and per-role grouping
- Search that filters the browser and jumps the preview to relevant matches
- Multi-select session operations
- Drag-to-move and `Ctrl+drag`-to-copy across folders and machines
- Keyboard copy/cut/paste across local and remote folders
- Remote machine support including nested container entry via `exec_prefix`
- Export to another machine as a real Codex session, not a loose JSONL dump
- Repair of previously broken local `cwd` mappings and Codex thread-index sync

## Interface Overview

### Browser

The left pane is the workspace/session browser.

It shows:

- top-level machine roots: `local` plus configured remotes
- grouped folder tree under each machine
- sessions underneath their project folder
- machine health badges: `[ok]`, `[cached]`, `[offline]`
- user-only sessions marked with `!`

It is designed for the same scanning pattern as a code editor sidebar: move through structure first, then inspect detail.

### Preview

The right pane is the conversation viewer.

It shows:

- chat, not raw event JSON
- adjacent user messages merged into one block
- adjacent assistant messages merged into one block
- readable timestamps
- total user and assistant message counts in the header
- full session id in the header
- default focus at the end of the conversation

Assistant blocks start collapsed by default. User blocks start expanded, except the first large prompt block, which starts collapsed.

### Status Bar

The footer is part of the UI, not decoration.

It always shows the important shortcuts for the current context so you do not have to memorize the app.

## First Run

Start the app:

```bash
codex-session-tui
```

On launch:

- the browser starts on the `local` machine root
- the first machine root is visible immediately
- folders start collapsed so the tree is readable
- no session preview is shown until you actually select a session

## Navigation

### Browser Navigation

- `Up` / `Down`: move through visible rows
- `Right`: expand a folder or enter its sessions
- `Left`: collapse a folder or return from a session to its folder row
- `Enter`: expand/collapse folder or open the selected session
- `Ctrl+Up` / `Ctrl+Down`: jump between projects
- `Ctrl+Left`: collapse all folders except the current one
- `Ctrl+Right`: expand all folders
- `F5` / `Ctrl+R`: refresh local and remote state
- `R`: add or update a remote machine
- `d`: delete the selected remote machine entry

Mouse:

- click to select folders and sessions
- double-click folders to expand/collapse
- double-click sessions to open Preview
- drag splitters to resize panes
- drag scrollbars to jump quickly

### Preview Navigation

- `Esc`: return focus to Browser
- `Tab`: fold/unfold current block
- `Shift+Tab`: fold/unfold all blocks
- `Up` / `Down`: move between preview blocks
- `PageUp` / `PageDown`: page through large conversations
- `Home` / `End`: jump to top or bottom
- `n` / `N`: jump to next/previous match in the current chat
- `o`: leave the TUI and open the selected session in `codex resume`

Mouse:

- scroll
- fold blocks
- select text
- copy selected preview text through OSC52-capable terminals

## Search

Press `/` to search.

Search is built to feel like editor search, not fuzzy guesswork.

It:

- filters the browser tree
- searches conversation text, path, session id/hash, and file name
- supports multi-word search
- supports quoted phrases such as `"openrouter error" auth`
- auto-selects the best matching session
- jumps the preview to the first relevant occurrence
- highlights matches in Browser and Preview
- highlights the primary preview hit more strongly than later hits

Search navigation:

- `Enter`: keep the current result
- `Esc`: close search
- `Tab` / `Shift+Tab`: move focus out of the search box
- `n` / `N` in Preview: next/previous occurrence in the current chat

## Session Workflows

### Move, Copy, Fork

On a session:

- `m`: move into another project context
- `c`: copy into another project context
- `f`: fork into another project context
- `d`: delete
- `e`: export over SSH
- `o`: open in Codex

Selection:

- `Space`: toggle selection
- `a`: select all sessions in the current project
- `i`: invert selection

Targets can be:

- a local path, for example `/home/me/work/repo`
- a machine-qualified path, for example `pi:/home/pi/work/repo`

### Seamless Browser Copy/Paste

If you do not want to type paths, you do not have to.

Browser actions work across connected machines as if everything were local:

- `Ctrl+C`: copy current session selection, current project, or current grouped folder sessions
- `Ctrl+X`: cut current selection
- `Ctrl+V`: paste into the selected folder
- drag: move into the hovered folder
- `Ctrl+drag`: copy into the hovered folder

This works for:

- local to local
- local to remote
- remote to local
- remote to remote

Folder and grouped-tree targets resolve automatically, so the Browser can be used like a workspace explorer instead of a path prompt.

### Folder-Level Work

Project and grouped-folder rows also support folder-wide copy/rename style workflows, so you can remap entire project histories instead of one session at a time.

## Export Over SSH

Export is for sending a real Codex session to another machine.

Enter targets like:

```text
user@host:/remote/project/path
```

Important behavior:

- the path is the remote project `cwd`
- it is not the remote `~/.codex` storage path
- the session is installed under the remote machine's `${CODEX_HOME:-~/.codex}/sessions/...`
- the session JSONL is rewritten to the remote project path
- the remote Codex thread index is updated so `codex resume` can see it
- existing remote rollout files are not overwritten

## Remote Machines

Configured remotes are loaded from:

- `.codex-session-tui.toml` in the current working directory
- `~/.config/codex-session-tui.toml`

You can add or update a machine from inside the app with `R`.

Supported input forms:

- `user@host`
- `name=user@host`
- `name=user@host:/absolute/path/to/.codex`
- `name=user@host|exec-prefix`
- `name=user@host|exec-prefix|/absolute/path/to/.codex`

If `name=` is omitted, the machine name is derived from the SSH host.

Examples:

```text
pi@192.168.0.124
192.168.0.124=pi@192.168.0.124
pi=pi@192.168.0.124:/home/pi/.codex
root@example-host|lxc-attach -n dev --|/root/.codex
dev=root@example-host|lxc-attach -n dev --|/root/.codex
```

Reusing `R` with the same connection details but a new name updates the existing machine entry in place.

### Config Example

```toml
[[machines]]
name = "pi"
ssh_target = "pi@192.168.0.124"
codex_home = "/home/pi/.codex"
```

If `codex_home` is omitted, the remote defaults to `~/.codex`.

### Container / Nested Shell Example

```toml
[[machines]]
name = "dev"
ssh_target = "root@example-host"
exec_prefix = "lxc-attach -n dev --"
codex_home = "/root/.codex"
```

This lets the TUI:

- SSH to the host
- enter the container
- scan sessions
- preview chats
- move/copy/fork sessions
- launch `codex resume` inside that environment

### Remote Health and Caching

- machine roots are marked `[ok]`, `[cached]`, or `[offline]`
- failed scans keep the last good snapshot instead of dropping the machine
- `g`, `F5`, and `Ctrl+R` force a fresh remote scan
- internal reloads may reuse a recent cached scan for responsiveness

### SSH Authentication

Preferred:

- SSH keys
- SSH agent
- host aliases in `~/.ssh/config`

Supported but less ideal:

- password-based SSH when your environment already handles the prompt externally

The app uses non-interactive remote scanning so startup does not hang waiting on password prompts.

## Recovery and Repair

The app does more than browse.

It also repairs common Codex session breakage:

- local move/copy/fork targets are normalized before being written
- relative paths become absolute
- trailing slashes and `.` / `..` segments are cleaned
- previously broken local `cwd` rewrites are repaired on startup
- Codex's local `threads` SQLite index is reconciled so repaired sessions reappear in `codex resume`

User-only sessions are also marked clearly:

- Browser shows `!`
- Preview warns that the session may not be resumable by Codex

## Typical Recovery Workflow

1. Launch `codex-session-tui`.
2. Press `/` and search by old path, session hash, or conversation text.
3. Inspect the conversation in Preview.
4. Move it with `m`, paste it with `Ctrl+V`, or drag it into the correct project.
5. Return to Codex and resume from the corrected working directory.

## Safety

The tool is conservative by design:

- backups are created before mutating or deleting session files
- writes use atomic temp-file plus rename
- unknown JSON fields are preserved
- only targeted fields are rewritten during remap/fork/export operations

Backups are created next to the original session file under `${CODEX_HOME:-~/.codex}/sessions`.

Backup filename format:

```text
<original>.jsonl.bak.YYYYMMDDHHMMSS
```

Find backups:

```bash
find "${CODEX_HOME:-$HOME/.codex}/sessions" -type f -name "*.jsonl.bak.*"
```

Restore a backup:

```bash
cp "/path/to/session.jsonl.bak.20260224101530" "/path/to/session.jsonl"
```

## Platform Support

Prebuilt binaries are published for:

- Linux: `x86_64`, `aarch64`, `armv7`
- macOS: `x86_64`, `aarch64`
- Windows (MSVC): `x86_64`, `aarch64`

## Development

Run locally:

```bash
cargo run
```

Run against a different Codex home:

```bash
CODEX_HOME=/path/to/.codex cargo run
```
