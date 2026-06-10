#!/usr/bin/env node
const { spawnSync } = require("node:child_process");

const PLATFORMS = {
  "darwin-arm64": "cartoon-wrap-darwin-arm64",
  "darwin-x64": "cartoon-wrap-darwin-x64",
  "linux-arm64": "cartoon-wrap-linux-arm64",
  "linux-x64": "cartoon-wrap-linux-x64",
  "win32-x64": "cartoon-wrap-win32-x64",
};

const key = `${process.platform}-${process.arch}`;
const pkg = PLATFORMS[key];
if (!pkg) {
  console.error(`cartoon: unsupported platform ${key}`);
  process.exit(1);
}
let bin;
try {
  const exe = process.platform === "win32" ? "cartoon.exe" : "cartoon";
  bin = require.resolve(`${pkg}/bin/${exe}`);
} catch {
  console.error(
    `cartoon: platform package ${pkg} missing — reinstall with optional deps enabled`
  );
  process.exit(1);
}
const result = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
process.exit(result.status ?? 1);
