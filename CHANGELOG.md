# Changelog

All notable changes to `codex-session-explorer` are documented in this file.

## [0.2.6] - 2026-02-24

### Changed
- Rust release binary renamed from `codex-session-explorer` to `codex-session-tui` across CI artifacts and release assets.

### Fixed
- npm launcher now ensures executable permissions (`chmod +x`) before spawn on Unix-like systems to avoid `EACCES` in `npx` cache.
- npm launcher keeps a legacy fallback to old binary name for compatibility.

## [0.2.5] - 2026-02-24

### Changed
- npm package renamed to unscoped `codex-session-tui`.
- Added npm bin alias so `npx -y codex-session-tui` runs directly.
- npm publish fallback check in CI now uses package name from `npm/package.json`.

## [0.2.4] - 2026-02-24

### Fixed
- npm publish workflow now correctly includes `dist/*` binaries in the package tarball.
- Added Linux `armv7` binary build/distribution path for 32-bit Raspberry Pi environments.
- npm launcher now maps `linux/arm` to `armv7-unknown-linux-gnueabihf`.

## [0.2.3] - 2026-02-24

### Changed
- Refresh release to pick up updated CI `NPM_TOKEN` for npm publish from GitHub Actions.

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
