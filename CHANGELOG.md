# Changelog

All notable changes to `codex-session-explorer` are documented in this file.

## [0.2.2] - 2026-02-23

### Fixed
- GitHub Actions workflow now has explicit `contents: write` permission so release binaries can be attached to GitHub Releases.
- npm publish remains CI-only (release-triggered workflow path).

## [0.2.1] - 2026-02-23

### Fixed
- CI release workflow now uses a supported macOS Intel runner label for `x86_64-apple-darwin` builds.

## [0.2.0] - 2026-02-23

### Added
- Multi-select sessions with keyboard and mouse checkbox toggles.
- Bulk session operations over selected sessions.
- Project-scope folder remap actions:
  - Rename folder sessions (rewrite `cwd` for all sessions in a project).
  - Copy folder sessions (duplicate all sessions to a new `cwd`).
- Session delete action with strict confirmation (`DELETE`) and backup-before-delete.
- Input path tab-completion with repeated-Tab directory listing.
- Theme-aware highlighted status rendering for repeated-Tab match lists.

### Changed
- Sessions pane default width increased for better readability.
- Sessions pane status/footer controls now expose select-all (`a`) and invert (`i`).

### Fixed
- Deterministic mouse checkbox toggle/unselect behavior in Sessions pane.
- Improved status bar action discoverability for project and session workflows.
