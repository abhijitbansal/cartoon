#!/usr/bin/env node
const { spawn } = require("node:child_process");
const os = require("node:os");

// One package per platform (scripts/npm-platform-packages.mjs).
// Linux binaries are static musl builds, so they run on glibc and musl alike.
const PLATFORMS = {
  "darwin-arm64": "cartoon-wrap-darwin-arm64",
  "darwin-x64": "cartoon-wrap-darwin-x64",
  "linux-arm64": "cartoon-wrap-linux-arm64",
  "linux-x64": "cartoon-wrap-linux-x64",
  "win32-x64": "cartoon-wrap-windows-x64",
};
const SIGNALS = ["SIGINT", "SIGTERM", "SIGHUP"];

const key = `${process.platform}-${process.arch}`;
const noBinary = () => {
  console.error(`cartoon: no prebuilt cartoon binary for ${key}; install with \`cargo install cartoon\``);
  process.exit(1);
};

const pkg = PLATFORMS[key];
if (!pkg) noBinary();
let bin;
try {
  const exe = process.platform === "win32" ? "cartoon.exe" : "cartoon";
  bin = require.resolve(`${pkg}/bin/${exe}`);
} catch {
  noBinary();
}

// Killing node must not orphan cartoon (and the test run under it).
// Installed before the spawn: the child can already be running (and be
// signalled) by the time spawn() returns. Handlers run on a later tick, so
// `child` is always set when one fires.
let child;
const forward = {};
for (const sig of SIGNALS) {
  forward[sig] = () => child.kill(sig);
  process.on(sig, forward[sig]);
}

child = spawn(bin, process.argv.slice(2), { stdio: "inherit" });

child.on("error", (err) => {
  console.error(`cartoon: failed to exec ${bin}: ${err.message}`);
  process.exit(1);
});

child.on("exit", (code, signal) => {
  if (signal) {
    // Die the same way the child did, so callers see the signal, not a code.
    for (const sig of SIGNALS) process.off(sig, forward[sig]);
    process.kill(process.pid, signal);
    // Not delivered (e.g. Windows): fall back to the shell convention.
    setTimeout(() => process.exit(128 + (os.constants.signals[signal] ?? 0)), 1000);
    return;
  }
  process.exit(code ?? 1);
});
