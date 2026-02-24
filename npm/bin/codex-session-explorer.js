#!/usr/bin/env node

const { spawnSync } = require("node:child_process");
const { chmodSync, existsSync } = require("node:fs");
const { join } = require("node:path");

const platform = process.platform;
const arch = process.arch;

const targetMap = {
  "linux:x64": "x86_64-unknown-linux-gnu",
  "linux:arm64": "aarch64-unknown-linux-gnu",
  "linux:arm": "armv7-unknown-linux-gnueabihf",
  "darwin:x64": "x86_64-apple-darwin",
  "darwin:arm64": "aarch64-apple-darwin",
  "win32:x64": "x86_64-pc-windows-msvc",
  "win32:arm64": "aarch64-pc-windows-msvc"
};

const key = `${platform}:${arch}`;
const target = targetMap[key];
if (!target) {
  console.error(
    `Unsupported platform/arch: ${platform}/${arch}. Supported: ${Object.keys(targetMap).join(", ")}`
  );
  process.exit(1);
}

const binName = platform === "win32" ? "codex-session-tui.exe" : "codex-session-tui";
const legacyBinName = platform === "win32" ? "codex-session-explorer.exe" : "codex-session-explorer";
let binPath = join(__dirname, "..", "dist", target, binName);
if (!existsSync(binPath)) {
  const legacyPath = join(__dirname, "..", "dist", target, legacyBinName);
  if (existsSync(legacyPath)) {
    binPath = legacyPath;
  }
}

if (!existsSync(binPath)) {
  console.error(`Binary not found: ${binPath}`);
  console.error("The npm package appears incomplete for this platform.");
  process.exit(1);
}

if (platform !== "win32") {
  try {
    chmodSync(binPath, 0o755);
  } catch (_) {
    // Ignore chmod failures and let spawn report a concrete error.
  }
}

const result = spawnSync(binPath, process.argv.slice(2), {
  stdio: "inherit"
});

if (result.error) {
  console.error(result.error.message);
  process.exit(1);
}
process.exit(result.status ?? 0);
