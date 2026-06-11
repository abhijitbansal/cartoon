# Releasing cartoon

One tag push publishes everywhere. This documents how the pipeline works,
what it assumes, and how to recover when a channel fails.

## TL;DR

```bash
git checkout main && git pull
cargo test
git tag -a vX.Y.Z -m "cartoon vX.Y.Z"
git push origin vX.Y.Z
gh run watch
```

`release.yml` then runs, all jobs independent:

| Job | Publishes | Auth |
|---|---|---|
| `build` → `github-release` | 5 binary tarballs on the GitHub release | `GITHUB_TOKEN` |
| `pypi-wheels` → `pypi-publish` | `cartoon` wheels to PyPI | Trusted Publishing (OIDC) |
| `npm-publish` | `cartoon-wrap` + 5 platform packages | Trusted Publishing (OIDC) |
| `crates-publish` | `cartoon` to crates.io | Trusted Publishing (OIDC) |

## Versioning

Single source of truth is the **git tag** (`vX.Y.Z`):

- npm versions are injected at publish time (`npm-set-version.mjs`,
  `npm-platform-packages.mjs`); the committed `package.json` stays `0.0.0`.
- PyPI version comes from the tag via maturin (`dynamic = ["version"]`).
- **Cargo.toml `version` must be bumped manually to match the tag** before
  tagging — `cargo publish` uses it verbatim.

## Auth: Trusted Publishing everywhere (no long-lived tokens)

There are **no registry secrets** in the repo. Each registry trusts this
repo + `release.yml` via GitHub OIDC (`permissions: id-token: write` on the
publish jobs):

- **PyPI**: pypi.org → project `cartoon` → Publishing. (Bootstrapped via a
  "pending publisher" before the project existed.)
- **npm**: each package → Settings → Trusted Publisher, allowed action
  `npm publish`. Requires npm ≥ 11.5 on the runner (the workflow installs
  `npm@latest`). New packages can't use OIDC for their *first* publish —
  bootstrap those with a short-lived granular token (30-day expiry,
  bypass-2FA checked), then configure the trusted publisher and delete it.
- **crates.io**: crate `cartoon` → Settings → Trusted Publishing. The
  workflow mints a temporary token via `rust-lang/crates-io-auth-action`.

If a trusted publisher config drifts (renamed workflow file, transferred
repo), the publish job fails with an auth error — fix the registry-side
config, not the workflow.

## Recovery

Jobs are independent; a red job never blocks the others.

```bash
gh run rerun <run-id> --failed     # rerun only the failed jobs
```

All three publish paths are idempotent: PyPI uses `--skip-existing`, the
npm job checks `npm view <pkg>@<version>` before each publish, and
re-publishing an existing crate version fails loudly without side effects.

Publishing a missing npm package manually (e.g. spam-blocked name):

```bash
gh run download <run-id> --pattern "cartoon-bin-<target>" -D bins
node scripts/npm-platform-packages.mjs <version> bins npm-out
(cd npm-out/<pkg> && npm publish --access public)   # prompts for OTP
```

Artifacts are retained ~90 days; after that, rebuild from the tag.

## Known issues

- `cartoon-wrap-win32-x64` tripped npm's name spam detection on first
  publish (2026-06-11, support ticket filed). Until it exists, Windows
  npm installs fall back to no prebuilt binary.

## Release checklist

1. CI green on main; `cargo test` locally.
2. Bump `Cargo.toml` version to match the tag; commit.
3. Tag + push (TL;DR above); watch the run.
4. Smoke-test: `uv tool install cartoon`, `npm i -g cartoon-wrap`,
   `cargo install cartoon` → `cartoon adapters`.
5. Check the registry pages render README + metadata.
6. Quarterly: audit registry token pages — there should be **zero**
   long-lived publish tokens; anything alive needs a reason.
