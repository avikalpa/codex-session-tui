# Changelog

## 1.3.14 - 2026-03-15

- Added non-blocking browser transfer progress for paste and drag/drop, with a live status-bar progress indicator showing completed, skipped, and failed session transfers.
- Added plain browser clipboard keys `c`, `x`, and `v`, while keeping `Ctrl+C`, `Ctrl+X`, and `Ctrl+V` working.
- Moved typed copy-to-target-path to `C` and updated the status bar onboarding accordingly.
- Changed browser `Tab` to toggle the selected folder open/closed.
- Added `Alt+Left` / `Alt+Up` and `Alt+Right` / `Alt+Down` for consistent pane switching.
- Updated `README.md` to document the new clipboard, transfer progress, and pane navigation UX.

## 1.3.13 - 2026-03-15

### Added
- Added persistent virtual folders in the Browser so you can create cwd destinations that do not exist on disk yet and still use them as move/copy/paste/drop targets.
- Added `n` on machine and folder rows to create virtual folders directly from the TUI.

### Changed
- Grouped-folder drag/drop now preserves the dragged folder as a subtree when dropped onto another machine or folder, which makes workflows like moving a local `git/...` tree into a remote `gh/...` layout practical.
- Updated the README manual to document virtual folders, grouped-folder drag/drop, and cross-machine destination planning.

### Fixed
- New virtual folders now expand their ancestor groups immediately so the created target stays visible in the Browser.

## 1.3.12 - 2026-03-15

### Fixed
- Recut the search-fix release from the correct `main` commit so npm and GitHub assets pick up the actual `Esc`-clears-search behavior and the updated README manual.

## 1.3.11 - 2026-03-15

### Fixed
- Fixed search escape behavior so `Esc` now fully clears the active search query, removes the kept filter state, and hides the search bar instead of leaving stale search results applied.

### Changed
- Expanded the README product manual to market cross-machine browsing, drag/drop session transfer, and SSH plus container-backed remotes as first-class command-center workflows.

## 1.3.10 - 2026-03-15

### Added
- Added direct browser drag-and-drop session transfer so sessions and folder session groups can be dropped onto another local or remote folder target.
- Added `Ctrl+drag` copy semantics in the Browser, reusing the same cross-machine clipboard path as keyboard `Ctrl+C` / `Ctrl+X` / `Ctrl+V`.

### Changed
- Updated the Browser status bar hints and README manual to document drag-to-move and `Ctrl+drag`-to-copy workflows.

### Fixed
- Fixed browser group-path target normalization so grouped tree folders resolve correctly as drop targets, including paths that render with the synthetic tree slash segments.
- Fixed the test harness and browser mouse path to carry the new drag state consistently through move/copy operations.

## 1.3.9 - 2026-03-15

### Fixed
- Fixed remote-machine selection for older saved entries whose machine name contains `/`, which was preventing rename/delete actions from recognizing some prefixed LXC remote rows as machine roots.

## 1.3.8 - 2026-03-15

### Fixed
- Added bracketed paste support to the search box and action prompt so terminal paste works reliably in TUI input areas.
- Allowed `v` on remote machine rows to follow the same rename-machine prompt flow as `m` and `r`, matching the reported machine-line behavior.

## 1.3.7 - 2026-03-15

### Fixed
- Enabled `m` and `r` on remote machine rows to rename the selected machine entry, matching the existing delete-remote UX on `d`.
- Preserved browser focus on the renamed machine after the config reload so the rename action feels stable in-place.

## 1.3.6 - 2026-03-15

### Fixed
- Normalized saved and newly entered `exec_prefix` values for known LXC container commands so `lxc-attach -n dev` automatically becomes `lxc-attach -n dev --`.
- Added startup config repair for older machine entries missing the trailing `--`, which was still causing container-backed remotes to appear offline even after the shell-wrapper fix.

## 1.3.5 - 2026-03-15

### Fixed
- Fixed `exec_prefix` remote execution for container-backed machines by switching the nested shell wrapper from `sh -lc` to `sh -c`, matching the failure reported in `ssh-bug.md`.
- Added regression coverage so `lxc-attach -n <name> --` remote wrappers no longer emit the brittle combined `-lc` form.

## 1.3.4 - 2026-03-15

### Fixed
- Fixed browser machine-root ordering so `local` is always first, configured remotes follow in config order, and arrow navigation traverses all visible machines correctly.
- Fixed browser keyboard navigation to stop auto-expanding folders and to work even when only machine roots are visible.
- Fixed remote machine deletion so `d` on a machine root uses the same confirmation UX as session deletion.
- Fixed `exec_prefix` remote Python execution to use the same SSH wrapper path as other remote operations, improving container-backed remote health detection.

### Changed
- Updated the README manual for the new browser startup position, explicit-only expansion behavior, and remote deletion flow.

## 1.3.3 - 2026-03-14

### Changed
- Republished the current clean history and README after removing a private hostname from recent `v1.3.x` git/npm history.
- Public examples now use `root@example-host` instead of the removed hostname.

## 1.3.1 - 2026-03-14

### Fixed
- Fixed the `Connect Remote` input flow so bare `user@host` entries no longer terminate the TUI.
- Added shorthand remote parsing for `user@host`, deriving the machine name from the SSH host when `name=` is omitted.
- Kept invalid remote input inside the prompt and surfaced the validation error in the status bar instead of exiting the app.
- Fixed configured remote machines not appearing in the Browser until they already had discovered session projects.
- Fixed startup browser selection so the first machine root stays in view instead of starting one row below it.
- Fixed browser path rendering to normalize accidental double-leading-slash paths such as `//home/pi`.

### Changed
- Reusing `R` with the same remote endpoint but a different `name=` now renames the existing machine entry instead of creating a duplicate.

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
