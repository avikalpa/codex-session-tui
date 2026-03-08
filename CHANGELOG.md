# Changelog

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
