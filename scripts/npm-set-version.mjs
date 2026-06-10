// Usage: node scripts/npm-set-version.mjs <version> <packageDir>
import fs from "node:fs";

const [version, dir] = process.argv.slice(2);
const file = `${dir}/package.json`;
const p = JSON.parse(fs.readFileSync(file, "utf8"));
p.version = version;
for (const k of Object.keys(p.optionalDependencies ?? {})) {
  p.optionalDependencies[k] = version;
}
fs.writeFileSync(file, JSON.stringify(p, null, 2) + "\n");
