// Usage: node scripts/npm-platform-packages.mjs <version> <binDir> <outDir>
// binDir layout: <binDir>/cartoon-bin-<rust-target>/cartoon[.exe]
// Writes one package per platform to <outDir>/<key>/ (e.g. npm-out/linux-x64).
//
// Platform packages are unscoped `cartoon-wrap-<platform>` names. Windows is
// `cartoon-wrap-windows-x64`: `cartoon-wrap-win32-x64` is held by npm's
// `0.0.1-security` placeholder and can never be published. Linux targets are static musl
// builds that run on glibc and musl distros alike, so the packages carry no
// `libc` field (one would only make npm skip a binary that works).
import fs from "node:fs";
import path from "node:path";

const [version, binDir, outDir] = process.argv.slice(2);
if (!version || !binDir || !outDir) {
  console.error("usage: npm-platform-packages.mjs <version> <binDir> <outDir>");
  process.exit(1);
}

const TARGETS = {
  "aarch64-apple-darwin": { key: "darwin-arm64", pkg: "cartoon-wrap-darwin-arm64", os: "darwin", cpu: "arm64" },
  "x86_64-apple-darwin": { key: "darwin-x64", pkg: "cartoon-wrap-darwin-x64", os: "darwin", cpu: "x64" },
  "aarch64-unknown-linux-musl": { key: "linux-arm64", pkg: "cartoon-wrap-linux-arm64", os: "linux", cpu: "arm64" },
  "x86_64-unknown-linux-musl": { key: "linux-x64", pkg: "cartoon-wrap-linux-x64", os: "linux", cpu: "x64" },
  "x86_64-pc-windows-msvc": { key: "win32-x64", pkg: "cartoon-wrap-windows-x64", os: "win32", cpu: "x64" },
};

let made = 0;
const missing = [];
for (const [target, t] of Object.entries(TARGETS)) {
  const exe = t.os === "win32" ? "cartoon.exe" : "cartoon";
  const src = path.join(binDir, `cartoon-bin-${target}`, exe);
  if (!fs.existsSync(src)) {
    console.error(`missing ${target}: ${src}`);
    missing.push(target);
    continue;
  }
  const name = t.pkg;
  const dir = path.join(outDir, t.key);
  const binOut = path.join(dir, "bin");
  fs.mkdirSync(binOut, { recursive: true });
  fs.copyFileSync(src, path.join(binOut, exe));
  fs.chmodSync(path.join(binOut, exe), 0o755);
  fs.writeFileSync(
    path.join(dir, "package.json"),
    JSON.stringify(
      {
        name,
        version,
        description: `cartoon binary for ${t.key}`,
        license: "MIT",
        repository: { type: "git", url: "git+https://github.com/abhijitbansal/cartoon.git" },
        os: [t.os],
        cpu: [t.cpu],
      },
      null,
      2
    ) + "\n"
  );
  fs.writeFileSync(
    path.join(dir, "README.md"),
    `# ${name}\n\nPrebuilt \`cartoon\` binary for **${t.key}**. Don't install this directly —\ninstall [cartoon-wrap](https://www.npmjs.com/package/cartoon-wrap), which\npicks the right platform package automatically:\n\n\`\`\`bash\nnpm install -g cartoon-wrap\n\`\`\`\n\ncartoon is a token-optimized TOON output wrapper for any CLI, built for\nLLM coding agents. Docs: https://github.com/abhijitbansal/cartoon\n\nMIT\n`
  );
  made++;
}
// cartoon-wrap depends on every platform package; publishing a partial set
// would ship a parent package that is broken on the missing platforms.
if (missing.length) {
  console.error(`no binary for: ${missing.join(", ")}`);
  process.exit(1);
}
console.log(`generated ${made} platform packages`);
