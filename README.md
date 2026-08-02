# codex-session-tui

Move, search, and repair Codex sessions from a terminal.

I mostly use this as:

```bash
npx -y codex-session-tui@latest
```

The main job is moving Codex sessions between computers, SSH boxes, containers, and renamed folders so `codex resume` can find the conversation again. That is why I made it.

It also browses sessions, but the important part is search. Ever lost the terminal where a useful thread was? Press `/`, search for a phrase, path, file name, session id, or hash, then move/copy/fork the session into the place where you need to continue.

## Install

Run it without installing:

```bash
npx -y codex-session-tui@latest
```

Install globally:

```bash
npm i -g codex-session-tui
codex-session-tui
```

Use another Codex home:

```bash
CODEX_HOME=/path/to/.codex codex-session-tui
```

## What It Does

- Finds Codex sessions under `${CODEX_HOME:-~/.codex}/sessions`.
- Groups sessions by recorded `cwd`.
- Searches conversation text, cwd paths, file names, session ids, and hashes.
- Moves sessions by rewriting the recorded `cwd`.
- Copies sessions into another cwd or machine.
- Forks sessions into a new session id when you want a fresh branch.
- Exports sessions over SSH into another machine's Codex store.
- Repairs stale local Codex thread-index rows so `codex resume` can see sessions again.

## The Common Workflow

1. Start it:

   ```bash
   npx -y codex-session-tui@latest
   ```

2. Press `/` and search for the thing you remember.

   Examples: `"openrouter error"`, `auth`, `/home/user/old-repo`, a session hash, or a file name.

3. Open the session preview and check it is the conversation you wanted.

4. Move, copy, or fork it into the right folder or machine.

5. Go back to Codex and resume from that folder.

## Keys

Browser:

- `Up` / `Down`: move through rows
- `Tab`, `Enter`, `Left`, `Right`: open and close folders
- `/`: search
- `n` / `N`: next/previous hit in the open session
- `[` / `]`: previous/next matching session
- `c`: copy selected sessions
- `m` or `x`: move selected sessions
- `f`: fork selected sessions
- `v`: paste into the selected folder
- `M` / `C` / `F`: type a move/copy/fork target path
- `R`: add or update a remote machine
- `d`: delete the selected session, folder, or remote entry
- `F5` or `Ctrl+R`: refresh

Preview:

- `Up` / `Down`: move between blocks
- `PageUp` / `PageDown`: scroll a long conversation
- `Home` / `End`: jump to top or bottom
- `Tab`: fold or unfold the current block
- `Shift+Tab`: fold or unfold all blocks
- `o`: leave the TUI and open the selected session with `codex resume`
- `b`: create a flattened recovery clone for sessions Codex resumes incorrectly
- `Esc`: go back to the browser

The footer shows the relevant keybindings while you use the app.

## CLI Mode

You can also run specific operations without opening the TUI:

```bash
codex-session-tui copy <session-id> pi:/home/user/work/repo
codex-session-tui move <session-id> /home/user/work/repo
codex-session-tui fork <session-id> dev:/srv/project
codex-session-tui export <session-id> user@host:/remote/project/path
codex-session-tui tree
codex-session-tui ls
codex-session-tui ls pi:/home/user/work/repo
codex-session-tui repair-index
codex-session-tui repair-index pi
```

Use `copy` when you want the same conversation in a second place. Use `fork` when you want a fresh branch with a new session id. Use `move` when the existing session should belong to a different cwd.

## Remote Machines

Press `R` in the TUI to add an SSH machine.

Supported forms:

```text
user@host
name=user@host
name=user@host:/absolute/path/to/.codex
name=user@host|exec-prefix
name=user@host|exec-prefix|/absolute/path/to/.codex
```

Examples:

```text
pi@192.0.2.124
pi=pi@192.0.2.124:/home/user/.codex
dev=root@example-host|lxc-attach -n dev --|/root/.codex
```

Configuration is read from:

- `.codex-session-tui.toml` in the current directory
- `~/.config/codex-session-tui.toml`

SSH keys or an SSH agent are the best setup. Password-based SSH can work if your environment handles the prompt, but it is not the smooth path.

## Safety

Session rewrites are deliberately boring:

- a backup is created before mutating session files
- writes use temp-file plus rename
- unknown JSON fields are preserved
- remap operations only touch the targeted `cwd`
- fork operations only add/change the fork metadata needed for a new branch

Backups live next to the original session JSONL:

```text
<original>.jsonl.bak.YYYYMMDDHHMMSS
```

Find them:

```bash
find "${CODEX_HOME:-$HOME/.codex}/sessions" -type f -name "*.jsonl.bak.*"
```

## Limits

`codex-session-tui` is not perfect.

- It is a terminal tool, so very large transcripts can still feel cramped.
- It follows Codex's session JSONL shape; if Codex changes that shape, this tool may need updates.
- SSH and container workflows depend on your shell, keys, and remote environment being sane.
- Some complex or fork-heavy sessions can be visible in Preview while `codex resume` opens only an older branch prefix. The `b` recovery clone flow exists for that case, but it is still a workaround.

For a richer GUI workflow, `codex-session-tui` is superseded by `github.com/yggdrasilhq/yggterm`. I still keep this alive because it is a nifty little terminal tool: it starts quickly in any SSH session and lets me move or recover Codex work without opening a desktop app.

## Development

Run locally:

```bash
cargo run
```

Run checks:

```bash
cargo check
cargo test
```

## License

Code in `2.x` releases is Apache-2.0. Repository documentation is CC BY-SA 4.0. Prior `1.x` releases remain under their original terms.
