# AGENTS.md

## Project Purpose

`codex-session-explorer` is a Rust TUI for inspecting and remapping Codex sessions stored in `~/.codex/sessions`.
The key use case is recovering sessions after repository/folder moves by rewriting recorded `cwd` fields.

## Current Scope

- Parse Codex session JSONL files.
- Group sessions by `cwd`.
- Interactive TUI browsing (projects, sessions, preview).
- Session operations: move, copy, fork across folder contexts.

## Architecture

- Runtime: single binary (`src/main.rs`) using `ratatui` + `crossterm`.
- Data source: filesystem scan of `${CODEX_HOME:-~/.codex}/sessions`.
- Persistence model:
  - move: in-place JSONL rewrite + backup
  - copy/fork: write new JSONL file under current date path

## Safety Rules

- Never mutate a session file without creating a backup first.
- Use atomic writes (`tmp` + rename) for all file writes.
- Preserve unknown JSON fields when rewriting; only touch targeted keys (`cwd`, and fork metadata keys).
- Keep operations local and deterministic; no network side effects.

## Developer Workflow

1. Run `cargo check` before finalizing changes.
2. Keep UI keybindings visible in the status/footer region.
3. Prefer incremental, reviewable changes; avoid broad refactors unless needed for correctness.
4. Document any behavior changes in `README.md`.

## Test-First Protocol

Use a strict test-first loop for all non-trivial changes (especially TUI rendering, scrolling, and session rewrite logic):

1. Write a failing test that captures the exact behavior gap before implementing the fix.
2. Implement the smallest change necessary to make that test pass.
3. Run the narrowest relevant test target first, then full `cargo test`.
4. Refactor only after tests pass, and keep behavior unchanged.
5. Add regression tests for any bug fixed from real usage reports.

Coverage expectations:

- Unit tests for pure logic: parsing, filtering, scoring, row mapping, rewrite transforms.
- Integration tests for end-to-end behavior: preview composition, search/filter flows, session operations.
- For TUI bugs, include tests that assert scroll/index mapping and wrapped-line behavior assumptions.

## Future Direction

- Add explicit confirmation prompts before destructive operations.
- Add filtering/search over `cwd`, session id, and timestamps.
- Add tests for JSON rewrite semantics and backup behavior.
