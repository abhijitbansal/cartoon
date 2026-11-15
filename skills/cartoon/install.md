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
brew tap abhijitbansal/cartoon https://github.com/abhijitbansal/cartoon && brew install cartoon   # Homebrew
cargo install cartoon          # Rust
uv tool install cartoon        # Python (uv); or: pipx install cartoon
npm install -g cartoon-wrap    # Node (installs the `cartoon` binary)
# No toolchain: prebuilt binary to ~/.local/bin, checksum-verified
curl -fsSL https://raw.githubusercontent.com/abhijitbansal/cartoon/main/install.sh | sh
cartoon adapters               # verify: lists the test-runner adapters
```

If no toolchain is available or installs need permission you don't have,
skip wrapping — never block the user's actual task on cartoon.

## 2. Set up auto-wrap (Claude Code)

A `PreToolUse` hook intercepts the Bash call and rewrites it to run under
cartoon — wrapping stops depending on the model remembering. One command
does the whole setup:

```bash
cartoon setup                 # hook in ~/.claude/settings.json + instructions block + doctor
cartoon setup --project       # hook in this repo's .claude/settings.json instead
cartoon setup --no-instructions   # hook only
```

`setup` is idempotent and never prompts. It writes the hook unless the
cartoon plugin is enabled (the plugin already ships it), and writes the
instructions block into `CLAUDE.md` if present, else `AGENTS.md`.

Or the plugin (skill + hook in one): `/plugin marketplace add
abhijitbansal/cartoon` then `/plugin install cartoon@cartoon`.

The hook is Claude Code only. For other agents install the skill
(`npx skills add abhijitbansal/cartoon`) and the instructions block
(`cartoon instructions install`), or register the MCP server
(`claude mcp add cartoon -- cartoon mcp`, or the JSON in
[docs/agents.md](../../docs/agents.md)).

### Cover piped commands — the instructions directive

The hook rewrites a trailing `| head -n N` / `| tail -n N` on a command with
an adapter, but not any other pipe (`pytest | grep FAIL`). The thing that
closes that gap is an instruction telling the agent to wrap and not pipe.
`cartoon setup` writes it; on its own:

```bash
cartoon instructions install            # → ./CLAUDE.md if present, else ./AGENTS.md
```

Check what's active anytime: `cartoon doctor`, `cartoon hook status`,
`cartoon instructions status`.

## 3. Disabling & uninstalling

- **Session** (set in the agent's environment): `CARTOON_NO_WRAP=1` — any
  non-empty value turns the hook off; unset to re-enable. One command raw:
  `cartoon --raw <cmd>`.
- **Permanent**: `cartoon hook uninstall [--project]` and `cartoon
  instructions uninstall`.

## Caveats

- **Conservative allowlist**: only test/lint/typecheck/build commands wrap.
  Mutating modes such as `ruff format`/`--fix`, env-prefixed commands like
  `PATH=… pytest`, `npx <non-JS tool>`, deploy/publish targets and watch
  modes are left to the normal permission prompt; infra CLIs (docker,
  kubectl, gh, aws) and mutating subcommands (`cargo publish`,
  `npm install`) pass through. The net-savings guard means wrapped output is
  never bigger than raw.
- **Your permission rules win**: the hook reads your Bash `ask` / `deny`
  rules and stays silent when one could match, and in plan mode. A command
  that was not rewritten is going through the normal permission flow.
- **Buffered, not streamed**: a wrapped command's report prints when it
  finishes, not live. Use `cartoon --raw` when you need live output.
