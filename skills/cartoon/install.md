# Installing cartoon & setting up auto-wrap

Read this **only when installing cartoon or wiring up automatic wrapping**.
Day-to-day use (when/how to prefix `cartoon`) is in [SKILL.md](SKILL.md);
this file stays out of context until setup is actually needed.

## 1. Install the binary

Check first:

```bash
command -v cartoon
```

If missing, install with the first toolchain available, then verify:

```bash
uv tool install cartoon        # preferred when uv exists
pipx install cartoon           # Python fallback
npm install -g cartoon-wrap    # Node (installs the `cartoon` binary)
cargo install cartoon          # Rust
cartoon adapters               # verify: lists the test-runner adapters
```

If no toolchain is available or installs need permission you don't have,
skip wrapping — never block the user's actual task on cartoon.

## 2. Auto-wrap so noisy commands wrap without being prefixed

A `PreToolUse` hook intercepts the command and rewrites it to run under
cartoon — wrapping stops depending on the model remembering. One
`cartoon hook rewrite` serves every agent; install it where each one looks.
Default scope is **user**; add `--project` for repo scope.

### Claude Code

```bash
cartoon hook install              # ~/.claude/settings.json (user)
cartoon hook install --project    # .claude/settings.json   (repo)
```

Or the plugin (skill + hook in one): `/plugin marketplace add
abhijitbansal/cartoon` then `/plugin install cartoon@cartoon`.

### Copilot CLI (≥ v1.0.24)

```bash
cartoon hook install --copilot              # ~/.copilot/hooks/cartoon.json (user)
cartoon hook install --copilot --project    # .github/hooks/cartoon.json (repo/team + coding agent)
```

If Copilot shows a confirmation dialog on every wrapped command (a known
v1.0.24 bug), add `--deny` for the smoother deny-and-suggest flow.

### VS Code Copilot Chat (Preview)

```bash
cartoon hook install --vscode     # shares ~/.claude/settings.json
```

Chat has no rewrite field, so the hook denies the raw command and suggests
the wrapped form (one extra round-trip). `--copilot` and `--vscode` are
different config files — don't combine them.

### Any agent without hooks — shell shims

```bash
cartoon shim install
export BASH_ENV=~/.config/cartoon/shims.sh   # for the non-interactive shells agents spawn
```

Functions shadow the bare tool names and beat PATH and venv-local binaries.

Check what's active anytime: `cartoon hook status` (every surface) and
`cartoon shim status`.

## 3. Disabling & uninstalling

- **Session** (set in the agent's environment): `CARTOON_NO_WRAP=1` (hook) or
  `CARTOON_NO_SHIM=1` (shims) — any non-empty value turns it off; unset to
  re-enable. One command raw: `cartoon --raw <cmd>`.
- **Permanent**: `cartoon hook uninstall [--copilot | --vscode] [--project]`
  and/or `cartoon shim uninstall` (then drop the `BASH_ENV` / source line).

## Caveats

- **Conservative allowlist**: only test/lint/typecheck/build commands wrap;
  infra CLIs (docker, kubectl, gh, aws) and mutating subcommands
  (`cargo publish`, `npm install`) pass through. The net-savings guard means
  wrapped output is never bigger than raw.
- **Buffered, not streamed**: a wrapped command's report prints when it
  finishes, not live. Use `cartoon --raw` when you need live output.
- **Install hooks to `~/.copilot/hooks` or `.github/hooks`**, not a plugin
  directory — plugin-defined Copilot hooks currently don't fire.
- **`.github/hooks/cartoon.json` is Copilot-CLI format** (camelCase
  `preToolUse`); VS Code Chat does not read it — use `--vscode` for Chat.
- **Shims don't cover** hyphenated tool names (`pre-commit`),
  `python -m pytest`, or `xcodebuild` — the hook does. Re-running an install
  won't overwrite a `cartoon.json` you didn't create.
