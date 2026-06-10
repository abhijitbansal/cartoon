// Usage: node scripts/npm-platform-packages.mjs <version> <binDir> <outDir>
// binDir layout: <binDir>/cartoon-bin-<rust-target>/cartoon[.exe]
import fs from "node:fs";
import path from "node:path";

const [version, binDir, outDir] = process.argv.slice(2);
if (!version || !binDir || !outDir) {
  console.error("usage: npm-platform-packages.mjs <version> <binDir> <outDir>");
  process.exit(1);
}

const TARGETS = {
  "aarch64-apple-darwin": { key: "darwin-arm64", os: "darwin", cpu: "arm64" },
  "x86_64-apple-darwin": { key: "darwin-x64", os: "darwin", cpu: "x64" },
  "aarch64-unknown-linux-gnu": { key: "linux-arm64", os: "linux", cpu: "arm64" },
  "x86_64-unknown-linux-gnu": { key: "linux-x64", os: "linux", cpu: "x64" },
  "x86_64-pc-windows-msvc": { key: "win32-x64", os: "win32", cpu: "x64" },
};

let made = 0;
for (const [target, t] of Object.entries(TARGETS)) {
  const exe = t.os === "win32" ? "cartoon.exe" : "cartoon";
  const src = path.join(binDir, `cartoon-bin-${target}`, exe);
  if (!fs.existsSync(src)) {
    console.error(`skip ${target}: ${src} missing`);
    continue;
  }
  const name = `cartoon-wrap-${t.key}`;
  const binOut = path.join(outDir, name, "bin");
  fs.mkdirSync(binOut, { recursive: true });
  fs.copyFileSync(src, path.join(binOut, exe));
  fs.chmodSync(path.join(binOut, exe), 0o755);
  fs.writeFileSync(
    path.join(outDir, name, "package.json"),
    JSON.stringify(
      {
        name,
        version,
        description: `cartoon binary for ${t.key}`,
        license: "MIT",
        repository: "github:abhijitbansal/cartoon",
        os: [t.os],
        cpu: [t.cpu],
      },
      null,
      2
    ) + "\n"
  );
  made++;
}
if (made === 0) {
  console.error("no platform packages generated");
  process.exit(1);
}
console.log(`generated ${made} platform packages`);
