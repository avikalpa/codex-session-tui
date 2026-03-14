# codex-session-tui

A terminal UI for browsing, searching, previewing, and remapping Codex session files in `${CODEX_HOME:-~/.codex}/sessions`.

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

## Major Release Demo

[![asciinema major release demo](https://asciinema.org/a/P0QAwEBJlNCqeci5.svg)](https://asciinema.org/a/P0QAwEBJlNCqeci5)

## Why This Exists

`codex resume` groups sessions by the stored `cwd` in session JSONL files.

If you rename a repository, move a folder, or restore work into a different path, those sessions can become effectively hidden from the normal Codex workflow even though the files still exist.

`codex-session-tui` is built to recover that state safely:

- inspect sessions grouped by project path
- preview the actual conversation instead of raw JSON events
- move, copy, fork, or rewrite sessions into a new folder context

## What The UI Shows

- `Browser` pane:
  grouped folder tree plus sessions, sorted by most recent activity
- `Preview` pane:
  grouped conversation blocks with timestamps and folding
- `Status bar`:
  discoverable keybindings for the currently focused pane

## First Run

Start the app:

```bash
codex-session-tui
```

On launch:

- the left pane shows a grouped folder tree
- all folders start collapsed
- the first machine root and its first folder are expanded so you land in a useful starting view
- selecting a session opens the preview at the end of the conversation so you see the latest exchange first

## Core Navigation

Browser:

- `Up` / `Down`: move through visible rows
- top-level rows are machines: `local` plus any configured SSH remotes
- machine rows show health state: `[ok]`, `[cached]`, or `[offline]`
- grouped folders compress single-child path chains in a GitHub-style tree
- `Right`: expand a folder or enter its sessions
- `Left`: collapse a folder or return from a session to its folder row
- `Enter`: expand/collapse folder or open the selected session
- `R`: add or update a remote machine in the config
- `F5` / `Ctrl+R`: refresh the session tree
- `Ctrl+Up` / `Ctrl+Down`: jump between projects
- `Ctrl+Left`: collapse all folders except the current one
- `Ctrl+Right`: expand all folders
- mouse:
  click folders and sessions, drag splitters, use scrollbars

Preview:

- `Esc`: return focus to the browser
- `Tab`: fold/unfold the current block
- `Shift+Tab`: fold/unfold all blocks
- `Up` / `Down`: move between preview blocks
- `PageUp` / `PageDown`: move by a preview page
- `Home` / `End`: jump to top/bottom of the chat
- `n` / `N`: jump to next/previous search hit inside the current chat
- `o`: quit the TUI and open the selected session in `codex resume`
- mouse:
  scroll, fold blocks, select text, drag scrollbar

## Search

Press `/` to search.

Search behavior:

- filters the browser tree
- tokenizes multi-word queries
- supports quoted phrases such as `"openrouter error" auth`
- selects the best matching session
- jumps the preview to the first relevant match
- highlights matches in both browser and preview
- the first preview hit is emphasized more strongly than later hits
- `n` / `N` in Preview moves between hits inside the current session

Tips:

- `Enter`: accept the current search result
- `Esc`: close search
- `Tab` / `Shift+Tab`: move focus out of the search box

## Session Operations

On a session, you can:

- `m`: move session to another folder context
- `c`: copy session to another folder context
- `f`: fork session into another folder context
- `e`: export session over SSH to `user@host:/remote/project/path`
- `o`: leave the TUI and open the selected session in `codex resume`
- `d`: delete session
- `Space`: multi-select sessions
- `a`: select all sessions in the current project
- `i`: invert selection

Project-level operations are also available for folder-wide rename/copy workflows.

Cross-machine targets:

- move, copy, fork, and folder-wide copy/rename accept either a plain local path or a machine-qualified target
- local target example: `/home/me/work/repo`
- remote target example: `pi:/home/pi/work/repo`
- browser copy/cut/paste also works across machines when the target folder is on another configured machine

User-only sessions:

- sessions with user messages but no assistant reply are marked with `!` in the Browser
- the Preview header warns that such sessions may not be resumable by Codex

SSH export behavior:

- enter a remote target in the form `user@host:/remote/project/path`
- the path is the remote project `cwd`, not the remote `~/.codex` storage directory
- the tool installs the session under the remote machine's `${CODEX_HOME:-~/.codex}/sessions/...`
- the exported session JSONL is rewritten to the remote project path
- the tool also updates the remote Codex thread index so `codex resume` can see the session
- export refuses to overwrite an existing remote rollout file with the same name

These operations exist for the main recovery use case: sessions whose original project path no longer matches where your repository lives now.

## Remote Machines

Configured remotes are loaded from either:

- `.codex-session-tui.toml` in the current working directory
- `~/.config/codex-session-tui.toml`

You can add a machine from inside the TUI with `R`.

Input forms:

- `name=user@host`
- `name=user@host:/absolute/path/to/.codex`
- `name=user@host|exec-prefix`
- `name=user@host|exec-prefix|/absolute/path/to/.codex`

Config shape:

```toml
[[machines]]
name = "pi"
ssh_target = "pi@192.168.0.124"
codex_home = "/home/pi/.codex"
```

If `codex_home` is omitted, the remote defaults to `~/.codex`.

Container / nested-shell example:

```toml
[[machines]]
name = "dev"
ssh_target = "root@example-host"
exec_prefix = "lxc-attach -n dev --"
codex_home = "/root/.codex"
```

This lets the TUI:

- SSH to `root@example-host`
- enter the `dev` container with `lxc-attach`
- scan sessions, preview chats, rewrite sessions, and launch `codex resume` inside that container

Remote health and caching:

- remote machine roots are marked `[ok]`, `[cached]`, or `[offline]`
- when a remote scan fails, the TUI keeps the last successful project snapshot instead of dropping the machine from the browser
- explicit refresh with `g`, `F5`, or `Ctrl+R` forces a fresh remote scan
- normal internal reloads may reuse a recent cached remote scan for responsiveness

SSH authentication:

- passwordless SSH with keys or an agent is the recommended setup
- browser startup and remote scans use non-interactive SSH behavior so the TUI does not hang waiting for a password prompt
- password-based SSH can still work for explicit operations if your environment already handles the prompt outside the app, but it is less reliable and less secure than key-based auth
- if you need more complex SSH behavior, prefer an `~/.ssh/config` host alias and reference that alias in `ssh_target`

Path rewrite behavior:

- move, copy, fork, and folder-wide rewrite operations normalize local target paths before writing
- relative paths are converted to absolute paths
- trailing slashes and `.` / `..` path segments are cleaned up
- on startup, the app repairs previously rewritten session files that still contain non-canonical local `cwd` values
- on startup, the app also reconciles Codex's local `threads` state database so `codex resume` sees repaired sessions again

## Typical Recovery Workflow

1. Launch `codex-session-tui`
2. Press `/` and search for part of the old repo path, session hash, or conversation text
3. Open the session and confirm it is the conversation you want
4. Press `m` to move it into the correct current folder context
5. Resume it normally from Codex in the new working directory

## Safety

The tool is designed to be conservative:

- backups are created before mutating or deleting session files
- writes use atomic temp-file plus rename
- unknown JSON fields are preserved
- only targeted fields are rewritten for remap/fork operations

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

## SSH Export Requirements

- `ssh` must be installed and available on your `PATH`
- the remote host must accept your normal SSH authentication
- the target path prompt expects a remote directory, not a local path

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
