# Changelog

## 1.3.0 - 2026-03-14

### Added
- Added remote machine health indicators in the Browser root rows: `[ok]`, `[cached]`, and `[offline]`.
- Added support for nested remote execution with `exec_prefix`, so machines like `ssh root@host` followed by `lxc-attach -n dev --` can be scanned and operated on directly.
- Added config/input support for `name=user@host|exec-prefix` and `name=user@host|exec-prefix|/absolute/path/to/.codex`.

### Changed
- Remote scans now fail fast instead of blocking on interactive SSH prompts during browser refresh.
- Remote project scans are now cached briefly and reused across non-forced reloads for better responsiveness on multi-machine setups.
- `F5`, `Ctrl+R`, and `g` now force a fresh remote scan instead of reusing cached remote state.

### Fixed
- Fixed the remote-connect crash path by making remote discovery resilient to unreachable or auth-blocked machines.
- Fixed remote preview/open/rewrite flows so they honor the configured nested execution prefix instead of assuming plain `ssh host`.

## 1.2.0 - 2026-03-14

### Added
- Added persistent machine configuration via `.codex-session-tui.toml` or `~/.config/codex-session-tui.toml`.
- Added `R` to connect or update SSH-backed remote machines from inside the TUI.
- Added a unified multi-machine browser that shows `local` and configured remote machines in one tree.

### Changed
- Startup now expands the first machine root and its first folder automatically.
- Preview headers now show which machine the selected session belongs to.
- Move/copy/fork/project-copy/project-rename target input now accepts `machine:/path` in addition to plain local paths.
- Browser paste operations now work across machines using the selected folder as the destination machine/path.

### Fixed
- Made cross-machine session operations route through the same rewrite/copy flow instead of requiring a separate export-only workflow.

## 1.1.0 - 2026-03-14

### Added
- Reworked the Browser into a grouped GitHub-style tree so shared folder ancestry is shown once instead of repeated in each project label.
- Added in-preview match navigation with `n` / `N`, plus `PageUp` / `PageDown` / `Home` / `End` navigation for large conversations.
- Added `o` to leave the TUI and launch the selected session directly in `codex resume`.

### Changed
- Preview headers now show the full session id instead of only the short hash.
- Preview search highlighting now distinguishes the primary match from later matches.
- Updated the README manual for grouped browser navigation, preview paging, in-chat match navigation, and open-in-Codex flow.

### Fixed
- Fixed mouse toggle behavior on grouped browser folders and project rows.
- Fixed preview-on-session-change behavior so newly opened sessions default to the latest conversation content.

## 1.0.11 - 2026-03-14

### Fixed
- Fixed the actual `codex resume` visibility bug by syncing repaired session `cwd` values into Codex's local `threads` SQLite state index, not just the JSONL rollout files.
- Updated move and folder-wide rewrite flows to update the matching `threads` row immediately after rewriting a session.
- Added regression tests for stale thread-index repair and move-action state sync.

## 1.0.10 - 2026-03-14

### Fixed
- Normalized local target paths before rewriting session `cwd` values for move, copy, fork, and folder-wide rewrite actions.
- Added startup repair for previously rewritten session files whose local `cwd` values were left in a non-canonical form.
- Tightened search behavior with quoted phrase support such as `"openrouter error" auth`.
- Updated the README manual for refresh shortcuts, search syntax, and path-repair behavior.

## 1.0.9 - 2026-03-13

### Fixed
- Fixed the release-assets job so `gh release upload` runs with an explicit repository context in non-checkout jobs.
- Revalidated the GitHub release publish path after the Node 24-era workflow migration.

## 1.0.8 - 2026-03-13

### Changed
- Updated the GitHub Actions workflow to current Node 24-era action majors.
- Replaced the JavaScript-based release asset upload action with `gh release upload`.
- Validated the release path against the updated workflow stack.

## 1.0.7 - 2026-03-13

### Added
- Added SSH export for sessions with the `e` action, allowing selected sessions to be uploaded to `user@host:/remote/dir`.
- Documented the SSH export workflow and requirements in the README.

### Fixed
- Corrected browser tree labels so descendant projects keep the missing intermediate path segments instead of collapsing to only the last folder name.
- Corrected browser tree indentation so nesting reflects actual project ancestors shown in the tree rather than raw filesystem depth.
- Updated the date-sensitive paste regression test to derive its expected date path from the current clock.

## 1.0.6 - 2026-03-09

### Changed
- Collapsed all folders by default on startup so the browser opens in a clean tree state.
- Prevented Preview from showing a session when the browser is resting on a folder row.
- Moved total `user` and `assistant` message counts into the Preview title metadata.

## 1.0.5 - 2026-03-08

### Changed
- Rewrote the README as a user-facing manual and synced it for npm publication.
- Added the major-release asciinema demo to the README.
- Added a release-workflow guard that fails if the npm package name is changed away from unscoped `codex-session-tui`.

## 1.0.4 - 2026-03-08

Packaging follow-up.

### Changed
- Switched the npm package name back from `@avikalpa/codex-session-tui` to unscoped `codex-session-tui`.
- Updated install and `npx` usage examples back to the unscoped package name.

## 1.0.3 - 2026-03-08

Release pipeline follow-up.

### Changed
- Kept npm trusted publishing support in the workflow.
- Restored `NPM_TOKEN` fallback for release publishing so package publication works immediately while npm trusted-publisher scope access is finalized.

## 1.0.2 - 2026-03-08

Release pipeline follow-up.

### Changed
- Switched npm publishing in GitHub Actions to npm trusted publishing via GitHub OIDC.
- Updated the release workflow to use a modern Node/npm runtime compatible with trusted publishing.

## 1.0.1 - 2026-03-08

Packaging follow-up release.

### Changed
- Switched the npm package name to `@avikalpa/codex-session-tui`.
- Updated install and `npx` usage examples to the scoped package name.

## 1.0.0 - 2026-03-08

Major release focused on turning the session explorer into a stable, workflow-grade TUI.

### Added
- Unified browser tree with inline projects and sessions.
- Session move, copy, fork, delete, project-folder rename, and project-folder copy actions.
- Multi-select session operations.
- Search-driven browser filtering with preview jump-to-match behavior.
- Foldable preview blocks with block focus navigation.
- Mouse selection, copying, draggable splitters, draggable scrollbars, and folder toggling.
- Bulk browser controls:
  - `Ctrl+Up` / `Ctrl+Down` project jump
  - `Ctrl+Left` collapse all except current
  - `Ctrl+Right` expand all
- Automated TUI regression coverage for browser navigation, mouse mapping, search rendering, preview highlighting, and status-bar onboarding hints.

### Changed
- Browser session labels now use the trailing short hash to reduce collisions.
- Browser root labels now preserve `/` and `/root` correctly.
- Browser navigation now follows a pinned-vs-auto expansion model:
  - manually opened folders remain open
  - navigation auto-expands the current project
  - previously auto-opened projects collapse when moving away
- Search box now shows a visible cursor and exposes keyboard onboarding in the status bar.
- Preview opens on the latest conversation content by default and supports search highlight overlays.

### Fixed
- Preview now renders chat content instead of raw event summaries in normal chat mode.
- Preview search highlights remain visible over two-tone message styling.
- Browser `Left` / `Right` and mouse row mapping are consistent with rendered tree rows.
- Rapid browser navigation no longer stalls as heavily due to deferred preview follow and reduced preview rendering work.
- Folder toggling is available from both keyboard and mouse.
