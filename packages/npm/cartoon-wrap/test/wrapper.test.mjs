// Exercises bin/cartoon.js against a fake node_modules layout: a stub
// platform package whose "binary" is a shell script. POSIX only (the stubs
// are sh scripts). Run: node --test packages/npm/cartoon-wrap/test/
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const WRAPPER = path.join(here, "..", "bin", "cartoon.js");
const key = `${process.platform}-${process.arch}`;
const posix = process.platform !== "win32";

// <tmp>/node_modules/cartoon-wrap/bin/cartoon.js + (optionally)
// <tmp>/node_modules/<platform package>/bin/cartoon containing `script`.
function layout(script, { mode = 0o755 } = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "cartoon-wrap-"));
  const wrapDir = path.join(root, "node_modules", "cartoon-wrap", "bin");
  fs.mkdirSync(wrapDir, { recursive: true });
  fs.copyFileSync(WRAPPER, path.join(wrapDir, "cartoon.js"));
  let bin = null;
  if (script !== null) {
    const pkg = key === "win32-x64" ? "cartoon-wrap-windows-x64" : `cartoon-wrap-${key}`;
    const platDir = path.join(root, "node_modules", pkg);
    fs.mkdirSync(path.join(platDir, "bin"), { recursive: true });
    fs.writeFileSync(path.join(platDir, "package.json"), JSON.stringify({ name: pkg }));
    bin = path.join(platDir, "bin", "cartoon");
    fs.writeFileSync(bin, `#!/bin/sh\n${script}\n`);
    fs.chmodSync(bin, mode);
  }
  return { root, entry: path.join(wrapDir, "cartoon.js"), bin };
}

function run(entry, args = []) {
  return spawnSync(process.execPath, [entry, ...args], { encoding: "utf8" });
}

test("passes args and stdio through and mirrors the exit code", { skip: !posix }, () => {
  const { entry } = layout('echo "out:$*"; echo err >&2; exit 7');
  const r = run(entry, ["a b", "--flag"]);
  assert.equal(r.status, 7);
  assert.equal(r.stdout, "out:a b --flag\n");
  assert.equal(r.stderr, "err\n");
});

test("re-raises the child's death signal", { skip: !posix }, () => {
  const { entry } = layout("kill -TERM $$");
  const r = run(entry);
  assert.equal(r.status, null);
  assert.equal(r.signal, "SIGTERM");
});

test("forwards SIGTERM to the child instead of orphaning it", { skip: !posix }, async () => {
  const { root, entry } = layout(
    // The child records that it got TERM, then exits 0 from its trap.
    `trap 'echo got-term > "${"$"}MARK"; exit 0' TERM\necho ready\nwhile :; do sleep 0.05; done`
  );
  const mark = path.join(root, "mark");
  const child = spawn(process.execPath, [entry], { env: { ...process.env, MARK: mark } });
  await new Promise((resolve) => child.stdout.once("data", resolve));
  // An orphaned stub would hold this pipe open and hang the runner.
  child.stdout.destroy();
  child.kill("SIGTERM");
  const [code] = await new Promise((resolve) => child.on("exit", (...a) => resolve(a)));
  assert.equal(code, 0);
  assert.equal(fs.readFileSync(mark, "utf8"), "got-term\n");
});

test("missing platform package points at cargo install", () => {
  const { entry } = layout(null);
  const r = run(entry);
  assert.equal(r.status, 1);
  assert.match(r.stderr, new RegExp(`no prebuilt cartoon binary for ${key}; install with \`cargo install cartoon\``));
});

test("spawn failure (no exec bit) is reported, not swallowed", { skip: !posix }, () => {
  // exec needs an x bit even for root, so this holds in containers too.
  const { entry, bin } = layout("exit 0", { mode: 0o644 });
  const r = run(entry);
  assert.equal(r.status, 1);
  assert.equal(r.stderr.trim().startsWith(`cartoon: failed to exec ${bin}: `), true, r.stderr);
});

test("generated platform packages, optionalDependencies and the wrapper agree", () => {
  const repo = path.join(here, "..", "..", "..", "..");
  const parent = JSON.parse(fs.readFileSync(path.join(here, "..", "package.json"), "utf8"));
  const deps = Object.keys(parent.optionalDependencies).sort();
  const wrapperNames = [...fs.readFileSync(WRAPPER, "utf8").matchAll(/"(cartoon-wrap-[a-z0-9-]+)"/g)]
    .map((m) => m[1])
    .sort();
  assert.deepEqual(wrapperNames, deps);

  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "cartoon-npm-"));
  const targets = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-unknown-linux-musl",
    "x86_64-unknown-linux-musl",
    "x86_64-pc-windows-msvc",
  ];
  for (const t of targets) {
    fs.mkdirSync(path.join(tmp, "bins", `cartoon-bin-${t}`), { recursive: true });
    const exe = t.includes("windows") ? "cartoon.exe" : "cartoon";
    fs.writeFileSync(path.join(tmp, "bins", `cartoon-bin-${t}`, exe), "bin");
  }
  const r = spawnSync(
    process.execPath,
    [path.join(repo, "scripts", "npm-platform-packages.mjs"), "1.2.3", path.join(tmp, "bins"), path.join(tmp, "out")],
    { encoding: "utf8" }
  );
  assert.equal(r.status, 0, r.stderr);
  const made = fs
    .readdirSync(path.join(tmp, "out"))
    .map((d) => JSON.parse(fs.readFileSync(path.join(tmp, "out", d, "package.json"), "utf8")))
    .map((p) => {
      assert.equal(p.version, "1.2.3");
      return p.name;
    })
    .sort();
  assert.deepEqual(made, deps);

  // A missing platform binary fails the run instead of publishing a subset.
  fs.rmSync(path.join(tmp, "bins", "cartoon-bin-x86_64-pc-windows-msvc"), { recursive: true });
  const partial = spawnSync(
    process.execPath,
    [path.join(repo, "scripts", "npm-platform-packages.mjs"), "1.2.3", path.join(tmp, "bins"), path.join(tmp, "out2")],
    { encoding: "utf8" }
  );
  assert.equal(partial.status, 1);
});

test("spawn failure with a broken interpreter is reported",{ skip: !posix }, () => {
  const { entry, bin } = layout("exit 0");
  fs.writeFileSync(bin, "#!/nonexistent/interpreter\n");
  const r = run(entry);
  assert.equal(r.status, 1);
  assert.match(r.stderr, /^cartoon: failed to exec .*: .*ENOENT/);
  assert.ok(r.stderr.includes(bin));
});
