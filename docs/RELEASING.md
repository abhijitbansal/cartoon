# Releasing cartoon

One tag push publishes everywhere. This documents how the pipeline works,
what it assumes, and how to recover when a channel fails.

## TL;DR

```bash
git checkout main && git pull
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
git tag -a vX.Y.Z -m "cartoon vX.Y.Z"
git push origin vX.Y.Z
gh run watch
```

`release.yml` then runs this graph (a job starts only when everything it
`needs:` succeeded):

```
verify-version ─┐
test ───────────┼─► build ─┬─► github-release
                │          ├─► npm-publish
                │          └─► crates-publish
                ├─► pypi-wheels ─┐
                └─► pypi-sdist ──┴─► pypi-publish
```

| Job | Publishes | Auth |
|---|---|---|
| `verify-version` | nothing: every manifest must match the tag | — |
| `test` | nothing: fmt, clippy, `cargo test`, npm wrapper test | — |
| `build` → `github-release` | 5 binary tarballs + `SHA256SUMS` + build provenance on the GitHub release | `GITHUB_TOKEN` |
| `pypi-wheels` + `pypi-sdist` → `pypi-publish` | 7 wheels + 1 sdist to PyPI | Trusted Publishing (OIDC) |
| `npm-publish` | `cartoon-wrap` + 5 `@cartoon-wrap/*` platform packages | Trusted Publishing (OIDC) |
| `crates-publish` | `cartoon` to crates.io | Trusted Publishing (OIDC) |

A failure in `verify-version` or `test` publishes nothing. After that the
three publish branches (GitHub/npm/crates via `build`, PyPI via the wheel
jobs) are independent: a red PyPI job does not block npm, and vice versa.

### Artifacts

| Target | Tarball / npm package | Wheel |
|---|---|---|
| `x86_64-unknown-linux-musl` (static) | `cartoon-x86_64-unknown-linux-musl.tar.gz`, `@cartoon-wrap/linux-x64` | `musllinux_1_2_x86_64` |
| `aarch64-unknown-linux-musl` (static) | `cartoon-aarch64-unknown-linux-musl.tar.gz`, `@cartoon-wrap/linux-arm64` | `musllinux_1_2_aarch64` |
| `x86_64-unknown-linux-gnu` | — | `manylinux2014_x86_64` (glibc ≥ 2.17) |
| `aarch64-unknown-linux-gnu` | — | `manylinux2014_aarch64` (glibc ≥ 2.17) |
| `x86_64-apple-darwin` | `cartoon-x86_64-apple-darwin.tar.gz`, `@cartoon-wrap/darwin-x64` | macOS x86_64 |
| `aarch64-apple-darwin` | `cartoon-aarch64-apple-darwin.tar.gz`, `@cartoon-wrap/darwin-arm64` | macOS arm64 |
| `x86_64-pc-windows-msvc` | `cartoon-x86_64-pc-windows-msvc.tar.gz`, `@cartoon-wrap/win32-x64` | Windows x86_64 |

Linux tarballs and npm binaries are static musl builds, so one binary runs
on every distro (glibc or musl, old or new). Until 0.6.0 they were built
natively on ubuntu-24.04 and needed glibc 2.39, which broke Ubuntu 22.04,
Debian 12, RHEL 9 and `python:*` images. The npm platform packages carry no
`libc` field for the same reason: the static binary works on both.

The tarball names are a contract: `install.sh` and
`[package.metadata.binstall]` in Cargo.toml download
`cartoon-<target>.tar.gz` (a Linux gnu host fetches the musl tarball).

## Versioning

**Cargo.toml `version` is the source of truth**, and the tag must match it:

- `cargo publish` and maturin (crate and wheels/sdist) read it from
  Cargo.toml (`pyproject.toml` declares `dynamic = ["version"]`).
- npm versions are injected from the tag at publish time
  (`npm-set-version.mjs`, `npm-platform-packages.mjs`); the committed
  `package.json` stays `0.0.0`. Since the tag must equal Cargo.toml, this is
  the same number.
- `.claude-plugin/plugin.json` and the site marker in `docs/index.html` are
  copies: `node scripts/check-versions.mjs --write` updates both.
- Drift fails early: `cargo test` (tests/version_sync.rs) and CI
  (`check-versions.mjs`) catch a manifest that disagrees with Cargo.toml,
  and the release's `verify-version` job runs `check-versions.mjs --tag
  $GITHUB_REF_NAME`, so a tag that doesn't match Cargo.toml publishes
  nothing.

## Auth: Trusted Publishing everywhere (no long-lived tokens)

There are **no registry secrets** in the repo. Each registry trusts this
repo + `release.yml` via GitHub OIDC (`permissions: id-token: write` on the
publish jobs only; the workflow default is `permissions: {}`):

- **PyPI**: pypi.org → project `cartoon` → Publishing.
- **npm**: each package → Settings → Trusted Publisher, allowed action
  `npm publish`. Requires npm ≥ 11.5 on the runner (the workflow installs a
  pinned npm 11.x). New packages can't use OIDC for their *first* publish —
  bootstrap those with a short-lived granular token (30-day expiry,
  bypass-2FA checked), then configure the trusted publisher and delete it.
- **crates.io**: crate `cartoon` → Settings → Trusted Publishing. The
  workflow mints a temporary token via `rust-lang/crates-io-auth-action`.

If a trusted publisher config drifts (renamed workflow file, transferred
repo), the publish job fails with an auth error — fix the registry-side
config, not the workflow.

### Before the next release: the `@cartoon-wrap` npm scope

The platform packages moved from `cartoon-wrap-<platform>` to
`@cartoon-wrap/<platform>`: the unscoped `cartoon-wrap-win32-x64` name is
npm's `0.0.1-security` placeholder, so Windows never got a binary. On
2026-10-02 the scope was free (`npm view @cartoon-wrap/linux-x64` → 404,
org lookup → "Scope not found"). Before tagging, the maintainer must:

1. Create the npm org `cartoon-wrap` (npmjs.com → Add Organization; free
   for public packages).
2. Bootstrap-publish the five `@cartoon-wrap/*` packages once with a
   short-lived granular token (see above), or let the first release do it
   with such a token, then configure a Trusted Publisher (this repo,
   `release.yml`) on each of the five and delete the token.
3. Optionally deprecate the old unscoped packages:
   `npm deprecate cartoon-wrap-linux-x64 "moved to @cartoon-wrap/linux-x64"`
   (and darwin-arm64, darwin-x64, linux-arm64).

The npm job now fails if any platform package fails to publish; it no
longer skips Windows silently.

## Supply chain

- Every third-party action is pinned by full commit SHA with the tag in a
  comment; `.github/dependabot.yml` proposes updates weekly.
- npm and `cross` are pinned to exact versions in the workflow.
- `SHA256SUMS` is uploaded with the tarballs; `install.sh` refuses a
  tarball that doesn't match it.
- Each tarball has a build-provenance attestation
  (`actions/attest-build-provenance`). Verify with
  `gh attestation verify cartoon-<target>.tar.gz -R abhijitbansal/cartoon`.

## Recovery

```bash
gh run rerun <run-id> --failed     # rerun only the failed jobs
```

All publish paths are idempotent: PyPI uses `skip-existing`, the npm job
checks `npm view <pkg>@<version>` before each publish, and re-publishing an
existing crate version fails loudly without side effects.

Publishing a missing npm platform package manually:

```bash
gh run download <run-id> --pattern "cartoon-bin-*" -D bins
node scripts/npm-platform-packages.mjs <version> bins npm-out
(cd npm-out/<platform> && npm publish --access public)   # prompts for OTP
```

Artifacts are retained ~90 days; after that, rebuild from the tag.

## Release checklist

1. CI green on main (`ci.yml`; same commands as the local gate).
2. Bump `Cargo.toml` version to match the tag, run
   `node scripts/check-versions.mjs --write` (plugin manifest + site);
   commit.
3. Tag + push (TL;DR above); watch the run.
4. Smoke-test on a clean machine or container, including an old-glibc one
   (`docker run --rm -it ubuntu:22.04`): `uv tool install cartoon`,
   `npm i -g cartoon-wrap`, `cargo install cartoon`,
   `curl -fsSL https://raw.githubusercontent.com/abhijitbansal/cartoon/main/install.sh | sh`
   → `cartoon adapters`.
5. Check the registry pages render README + metadata.
6. Quarterly: audit registry token pages — there should be **zero**
   long-lived publish tokens; anything alive needs a reason.

## GitHub Actions settings

`ci.yml` was disabled in the repository's Actions settings in 2026-06.
Editing the file does not re-enable it: a maintainer must open Actions →
ci → "Enable workflow". `upstream-drift` is a new file and starts enabled,
but GitHub disables scheduled workflows after 60 days without repository
activity; re-enable it the same way if that happens. The repository is
public, so standard GitHub-hosted runners are free.
