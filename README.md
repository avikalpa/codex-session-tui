# codex-session-tui

`codex-session-tui` is a terminal-first session explorer for Codex.

It gives you a VS Code style workflow for `~/.codex`: a browser tree on the left, a rich preview on the right, searchable conversations, foldable blocks, multi-select, drag and drop, and cross-machine session management.

It can also act as a Codex session command center: connect to SSH machines, step through container prefixes such as `lxc-attach`, inspect all those session stores in one browser, and move conversations between them without leaving the TUI.

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

Non-interactive session operations:

```bash
codex-session-tui copy <session-id> pi@openclaw:/home/pi/data/cases
codex-session-tui move <session-id> /home/pi/work/repo
codex-session-tui fork <session-id> dev:/srv/project
codex-session-tui export <session-id> user@host:/remote/project/path
codex-session-tui tree
codex-session-tui ls pi@openclaw
codex-session-tui ls pi@openclaw:/home/pi/data/cases
codex-session-tui repair-index
codex-session-tui repair-index pi@openclaw
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
- Remote session command center for multiple hosts and containers from one screen
- Grouped project tree with compressed single-child folder chains
- Session list ordered by recent activity
- Rich preview with markdown rendering, foldable blocks, timestamps, and per-role grouping
- Search that filters the browser and jumps the preview to relevant matches
- Multi-select session operations
- Drag-to-move and `Ctrl+drag`-to-copy across folders and machines
- Folder-tree drag/drop that preserves grouped subpaths when moving across machines
- Keyboard copy/cut/paste across local and remote folders
- Remote machine support including nested container entry via `exec_prefix`
- Virtual folder creation for cwd targets that do not exist on disk yet
- Export to another machine as a real Codex session, not a loose JSONL dump
- Repair of previously broken local `cwd` mappings and Codex thread-index sync

## Command Center Use Cases

This is where the remote model becomes useful in practice.

You can use one TUI instance to:

- browse your local sessions and multiple SSH machines in one tree
- hop through container boundaries such as `lxc-attach -n dev --`
- inspect a chat running inside a remote container without opening another terminal
- drag a session from one machine into another machine's project folder
- copy a conversation from a laptop to a server, or from a server into a container
- recover chats after a mountpoint change, repo rename, or machine migration

If your workflow spans local dev, remote boxes, and containers, this app is meant to be the control plane above all of them.

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
- `Tab`: toggle the selected folder open/closed
- `Right`: expand a folder or enter its sessions
- `Left`: collapse a folder or return from a session to its folder row
- `Enter`: expand/collapse folder or open the selected session
- `Alt+Left` / `Alt+Up`: move focus to the previous pane
- `Alt+Right` / `Alt+Down`: move focus to the next pane
- `Ctrl+Up` / `Ctrl+Down`: jump between projects
- `Ctrl+Left`: collapse all folders except the current one
- `Ctrl+Right`: expand all folders
- `F5` / `Ctrl+R`: refresh local and remote state
- `R`: add or update a remote machine
- `d`: delete the selected remote machine entry
- `n`: create a new virtual folder under the selected machine or folder
- `m` / `x`: cut into the browser clipboard
- `c`: copy into the browser clipboard
- `f`: fork into the browser clipboard
- `v`: paste into the selected folder
- `M` / `C`: typed move/copy-to-target-path flow for the selected folder or subtree
- `r`: typed rename of the selected folder or subtree

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
- `Alt+Left` / `Alt+Up`: move focus to the previous pane
- `Alt+Right` / `Alt+Down`: move focus to the next pane
- `Up` / `Down`: move between preview blocks
- `PageUp` / `PageDown`: page through large conversations
- `Home` / `End`: jump to top or bottom
- `Ctrl+Up` / `Ctrl+Down`: jump to top or bottom like a spreadsheet
- `Ctrl+Left` / `Ctrl+Right`: move to previous or next folded block
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
- `Left` / `Right`: move inside the search text
- `Ctrl+A` / `Ctrl+E`: jump to start/end of the search text
- `Tab` / `Shift+Tab`: move focus out of the search box
- `n` / `N` in Preview: next/previous occurrence in the current chat

## Session Workflows

### Move, Copy, Fork

On a session:

- `m` or `x`: cut into the browser clipboard
- `c`: copy into the browser clipboard
- `f`: fork into the browser clipboard
- `v`: paste into the selected folder or grouped subtree
- `M`: typed move to `/path` or `machine:/path`
- `C`: typed copy to `/path` or `machine:/path`
- `F`: typed fork to `/path` or `machine:/path`
- `d`: delete
- delete now runs with live status/progress feedback instead of freezing the UI during long removals
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

- `c` or `Ctrl+C`: copy current session selection, current project, or current grouped folder sessions
- `m`, `x`, or `Ctrl+X`: cut current selection
- `f`: prepare a fork of the current selection
- `v` or `Ctrl+V`: paste into the selected folder
- drag: move into the hovered folder
- `Ctrl+drag`: copy into the hovered folder
- dragging a grouped folder preserves that folder as a subtree instead of flattening all sessions into one cwd

The intent is file-manager style session handling: you should be able to move operational context around your estate the same way you move files in Explorer or VS Code's workspace tree, without stopping to type paths every time.

### Non-Interactive CLI

For reproducible operations, automation, or debugging a suspicious session transfer, use the non-interactive CLI mode.

Supported commands:

- `codex-session-tui copy <session-id> <target>`
- `codex-session-tui move <session-id> <target>`
- `codex-session-tui fork <session-id> <target>`
- `codex-session-tui export <session-id> <target>`
- `codex-session-tui tree`
- `codex-session-tui ls [machine|machine:/path]`

Examples:

```bash
codex-session-tui copy 019aee85-21cf-78a2-9a65-5286d2f341b6 pi@openclaw:/home/pi/data/cases
codex-session-tui move 019aee85-21cf-78a2-9a65-5286d2f341b6 /home/pi/data/cases-debug
```

CLI mode loads the local Codex store directly and does not wait for remote browser scans before running the requested session action. That makes it suitable for recovery work and for isolating transfer bugs without going through the interactive UI.

`tree` and `ls` use the Browser's grouped tree model instead of dumping raw files. They are useful for checking exactly what the TUI thinks exists on each machine and folder when debugging remote visibility problems.

`repair-index` backs up the Codex thread database and removes stale rows whose rollout path no longer exists. Use it after older buggy copies, manual filesystem cleanup, or interrupted migrations. You can run it for `local`, for one remote machine name, or across all configured machines.

This works for:

- local to local
- local to remote
- remote to local
- remote to remote

Typical examples:

- drag a local chat into `pi:/home/pi/work/repo`
- `Ctrl+drag` a production debugging conversation from one remote machine into another machine's staging repo
- cut a session from a host machine and paste it into a container-backed machine configured with `lxc-attach -n dev --`
- drag the grouped `git` folder from `local` onto a remote machine root and keep the `git/...` subtree intact there
- create `gh` as a virtual folder on a remote machine, then drag sessions into it before the actual repo exists on disk

Folder and grouped-tree targets resolve automatically, so the Browser can be used like a workspace explorer instead of a path prompt.

### Folder-Level Work

Project and grouped-folder rows also support folder-wide copy/rename style workflows, so you can remap entire project histories instead of one session at a time.

Important distinction:

- drag/drop on a grouped folder preserves that folder name as part of the destination subtree
- typed `r`, `M`, or `C` on a grouped folder performs a prefix remap

Example:

- drag `git` onto `pi:/home/pi/work` -> sessions land under `pi:/home/pi/work/git/...`
- rename grouped `/root` to `/home/pi` -> sessions land under `/home/pi/...` rather than `/home/pi/root/...`

### Virtual Folders

Sometimes you want a destination cwd before the actual repository exists on that machine.

Use `n` on a machine root or folder row to create a virtual folder.

Important behavior:

- this does not create a real directory on disk
- it creates a persistent browser destination in the TUI config
- you can use it as a drop target, paste target, or typed move/copy/fork target later
- once sessions are moved there, that cwd becomes part of the normal session tree even if no real repo exists yet

Typical use:

- create `gh` on a remote machine
- drag local `git/...` session groups into `gh`
- keep session organization aligned with the repo layout you intend to create later

## Busy Operations

Move, copy, paste, export, and folder-wide operations can take time, especially across SSH machines.

When that happens, the status bar shows:

- an explicit `Working...` state immediately
- a blinking `Working...` indicator while the operation is active
- a live progress bar
- counts for completed, skipped, and failed session transfers

That keeps long remote copies and grouped drag/drop operations understandable instead of looking like a stalled terminal UI.

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

Remote support is not an add-on. It is one of the main reasons this project exists.

Once configured, remote machines appear directly in the Browser beside `local`, so the app behaves more like a distributed workspace explorer than a single-machine viewer.

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

Practical meaning of those examples:

- `pi@192.168.0.124`
  connects to a plain remote Codex home with pubkey SSH
- `pi=pi@192.168.0.124:/home/pi/.codex`
  pins a friendly machine name and a non-default Codex home
- `dev=root@example-host|lxc-attach -n dev --|/root/.codex`
  SSHes to the host, enters the `dev` container, and then treats that container as another first-class Codex machine in the Browser

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

That is the intended model: a container is not a second-class target. If Codex runs there, `codex-session-tui` should let you manage it as if it were just another workspace root.

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

For serious use, key-based auth is the right setup. It makes the Browser feel immediate and keeps the remote command-center workflow usable. Password-based SSH can work, but it is operationally worse and should be treated as fallback.

## Recovery and Repair

The app does more than browse.

It also repairs common Codex session breakage:

- local move/copy/fork targets are normalized before being written
- relative paths become absolute
- trailing slashes and `.` / `..` segments are cleaned
- previously broken local `cwd` rewrites are repaired on startup
- session files whose internal `session_meta.id` drifted away from the rollout filename are repaired on startup
- Codex's local `threads` SQLite index is reconciled so repaired sessions reappear in `codex resume`

User-only sessions are also marked clearly:

- Browser shows `!`
- Preview warns that the session may not be resumable by Codex

## Typical Recovery Workflow

1. Launch `codex-session-tui`.
2. Press `/` and search by old path, session hash, or conversation text.
3. Inspect the conversation in Preview.
4. Move it with `m`, paste it with `Ctrl+V`, or drag it into the correct project.
5. For large tree-level rewrites, select the grouped parent folder and use `r` or `M`.
6. Return to Codex and resume from the corrected working directory.

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
