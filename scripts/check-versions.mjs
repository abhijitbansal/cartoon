// One version, every manifest. Cargo.toml is the in-repo source of truth
// (docs/RELEASING.md); this checks that every other place a version is
// written agrees with it, and — on a release — that the git tag does too.
//
// Usage:
//   node scripts/check-versions.mjs              # exit 1 on any mismatch
//   node scripts/check-versions.mjs --tag v0.6.0 # also require the tag to match
//   node scripts/check-versions.mjs --write      # rewrite the JSON manifests + site to match
//
// GitHub CI is disabled by decision (runner minutes cost); `cargo test` runs
// tests/version_sync.rs, which enforces the plugin manifest locally, and the
// release workflow runs this script with --tag before publishing anything.
import fs from "node:fs";
import { execFileSync } from "node:child_process";

const cargo = fs.readFileSync("Cargo.toml", "utf8");
const version = cargo.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if (!version) {
  console.error("check-versions: could not read version from Cargo.toml");
  process.exit(2);
}

const args = process.argv.slice(2);
const write = args.includes("--write");
const tagIdx = args.indexOf("--tag");
const tag = tagIdx >= 0 ? args[tagIdx + 1] : null;

const JSON_MANIFESTS = [
  { file: ".claude-plugin/plugin.json", get: (j) => j.version, set: (j) => (j.version = version) },
];

const problems = [];

for (const m of JSON_MANIFESTS) {
  if (!fs.existsSync(m.file)) continue;
  const json = JSON.parse(fs.readFileSync(m.file, "utf8"));
  const found = m.get(json);
  if (found !== version) {
    if (write) {
      m.set(json);
      fs.writeFileSync(m.file, JSON.stringify(json, null, 2) + "\n");
      console.log(`check-versions: ${m.file} ${found} -> ${version}`);
    } else {
      problems.push(`${m.file} says ${found ?? "(missing)"}, Cargo.toml says ${version}`);
    }
  }
}

// The marketing site keeps its own marker; reuse the existing sync script.
try {
  execFileSync("node", ["scripts/sync-site-version.mjs", ...(write ? [] : ["--check"])], {
    stdio: "inherit",
  });
} catch {
  problems.push("docs/index.html version marker is out of sync (run: node scripts/sync-site-version.mjs)");
}

if (tag !== null) {
  const tagVersion = tag.replace(/^v/, "");
  if (tagVersion !== version) {
    problems.push(`git tag ${tag} does not match Cargo.toml version ${version} — bump Cargo.toml (and run this script with --write) before tagging`);
  }
}

if (problems.length) {
  console.error("check-versions: FAILED\n  " + problems.join("\n  "));
  process.exit(1);
}
console.log(`check-versions: all manifests agree on ${version}${tag ? ` (tag ${tag})` : ""}`);
