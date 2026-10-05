// Usage: node scripts/npm-set-version.mjs <version> <packageDir> [--omit name,name]
// --omit drops optionalDependencies that could not be published this
// release (a platform package that does not exist on npm yet), so the
// parent never points at a version that isn't there.
import fs from "node:fs";

const args = process.argv.slice(2);
const omitIdx = args.indexOf("--omit");
const omit = new Set(
  omitIdx >= 0 ? (args[omitIdx + 1] ?? "").split(",").filter(Boolean) : []
);
const [version, dir] = args.filter((_, i) => omitIdx < 0 || (i !== omitIdx && i !== omitIdx + 1));
const file = `${dir}/package.json`;
const p = JSON.parse(fs.readFileSync(file, "utf8"));
p.version = version;
for (const k of Object.keys(p.optionalDependencies ?? {})) {
  if (omit.has(k)) {
    delete p.optionalDependencies[k];
  } else {
    p.optionalDependencies[k] = version;
  }
}
fs.writeFileSync(file, JSON.stringify(p, null, 2) + "\n");
