// Keep the marketing site's displayed version pinned to the package version.
//
// Cargo.toml `version` is the in-repo source of truth (see docs/RELEASING.md);
// this writes it into the version marker in docs/index.html so the site never
// drifts. Run it whenever you bump Cargo.toml; CI runs it with --check so a
// bump that forgets the site fails fast.
//
// Usage:
//   node scripts/sync-site-version.mjs           # write Cargo.toml version into the site
//   node scripts/sync-site-version.mjs --check   # exit 1 if the site is out of sync
import fs from "node:fs";

const SITE = "docs/index.html";
// The single span the site renders the version into. Keep this marker stable;
// it is the contract between the page and this script.
const MARKER = /(<span id="version" data-cartoon-version>)([^<]*)(<\/span>)/;

const cargo = fs.readFileSync("Cargo.toml", "utf8");
const cargoVersion = cargo.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if (!cargoVersion) {
  console.error("sync-site-version: could not read version from Cargo.toml");
  process.exit(2);
}

const html = fs.readFileSync(SITE, "utf8");
const found = html.match(MARKER);
if (!found) {
  console.error(`sync-site-version: version marker not found in ${SITE}`);
  process.exit(2);
}
const siteVersion = found[2];
const check = process.argv.includes("--check");

if (check) {
  if (siteVersion !== cargoVersion) {
    console.error(
      `sync-site-version: ${SITE} shows ${siteVersion}, Cargo.toml is ${cargoVersion}\n` +
        "  run: node scripts/sync-site-version.mjs"
    );
    process.exit(1);
  }
  console.log(`sync-site-version: in sync (${cargoVersion})`);
  process.exit(0);
}

if (siteVersion === cargoVersion) {
  console.log(`sync-site-version: already ${cargoVersion}`);
  process.exit(0);
}
fs.writeFileSync(SITE, html.replace(MARKER, `$1${cargoVersion}$3`));
console.log(`sync-site-version: ${siteVersion} -> ${cargoVersion}`);
