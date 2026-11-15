# Using cartoon with coding agents

cartoon's whole point is agents, so the skill that teaches an agent to use
it lives in this repo under [`skills/`](../skills/) and installs with one
command into every major tool.

The [`cartoon` skill](../skills/cartoon/SKILL.md) teaches the agent to
prefix `cartoon` onto test runs, builds and JSON CLIs, to install the binary
if missing, and when not to wrap.

## Claude Code (plugin — recommended)

This repo is a Claude Code plugin marketplace. Inside Claude Code:

```
/plugin marketplace add abhijitbansal/cartoon
/plugin install cartoon@cartoon
```

The skill is model-invoked (Claude wraps your test runs without being
asked) and available as a slash command:

```
/cartoon:cartoon    # load the usage/install guidance explicitly
```

Update later with `/plugin marketplace update cartoon`.

## Codex, Cursor, Windsurf, opencode, … (skills.sh)

The [skills.sh](https://skills.sh) CLI installs skills from any GitHub
repo into 40+ agents, auto-detecting which ones you have:

```bash
npx skills add abhijitbansal/cartoon              # interactive: pick agents
npx skills add abhijitbansal/cartoon --all        # all detected agents
npx skills add abhijitbansal/cartoon --skill cartoon -a codex -a cursor
npx skills list                                   # see what's installed where
```

This also works for Claude Code (`-a claude-code`) if you prefer plain
skills over the plugin.

## Guaranteed wrapping: the Claude Code hook

Skills and `AGENTS.md` only *ask* the agent to prefix `cartoon`; the model
complies probabilistically and sometimes forgets. A **`PreToolUse` hook**
removes the guesswork: it intercepts the Bash call and rewrites it
deterministically (`updatedInput`), so the wrapped command runs in place.
The hook is for **Claude Code**. Other agents use the skill, the
instructions block or the MCP server below.

```bash
cartoon setup                 # hook (user scope) + instructions block + doctor
cartoon setup --project       # hook in this repo's .claude/settings.json
cartoon hook status           # what's installed
cartoon hook uninstall [--project]
```

`cartoon setup` skips the `settings.json` entry when the cartoon plugin is
enabled, because the plugin already ships the hook.

Notes and knobs:

- **Narrow on purpose.** A rewrite is emitted as `permissionDecision:
  "allow"`, which skips the permission prompt for that call. Only test
  runners, linters, typecheckers and builds wrap. Infra CLIs (docker,
  kubectl, terraform, gh, aws), mutating subcommands and modes
  (`cargo publish`, `npm install`, `--fix`, `jest -u`, `--init`), xcodebuild
  modes other than build, test and clean, and the deploy, publish, sign and
  watch forms the hook recognises never do. A task name that avoids every
  word the hook knows is a residual (see
  [usage.md](usage.md#what-the-hook-will-not-auto-approve)). Output is never
  larger than the raw run: byte-identical at worst, and a dropped
  `| head -n N` / `| tail -n N` is applied when no report replaces it.
- **Which permission rules the hook honours.** Claude Code checks `ask` and
  `deny` rules against the command a hook returns, so a hook that rewrote
  `make deploy` into `cartoon -c 'make deploy'` would sidestep a
  `Bash(make deploy:*)` rule. cartoon's hook reads your Bash `ask` / `deny`
  rules from settings files (user: `$CLAUDE_CONFIG_DIR` or `~/.claude`;
  project; the managed-settings file) and stays silent when one could match
  the command, any segment of it, or the tool behind a launcher or path
  (`uv run pytest`, `xcrun xcodebuild`, `./gradlew`), or when it cannot
  tell. It also stays silent in `plan` permission mode. Measured 2026-11-15
  on Claude Code 2.1.325 (headless `claude -p`, hook installed): settings-file
  `deny` / `ask` rules, `--disallowedTools` and `dontAsk` mode all stopped
  the command. Not measured, and not readable by the hook: rules passed with
  `--settings`, remote managed policy, session rules added with
  `/permissions`, Agent SDK permission options, managed-settings locations
  other than the one file per OS (drop-in directory, MDM profile), and
  Windows. Keep a rule that must hold as a `deny` in a settings file the hook
  can read.
- **Disable without uninstalling:** set `CARTOON_NO_WRAP=1` in the
  environment your agent runs in (any non-empty value; unset to re-enable).
  Permanent: `cartoon hook uninstall`. One command raw: `cartoon --raw <cmd>`.
- **Overhead.** The per-command check is a tiny fail-open process. When
  wrapping, cartoon **buffers** output (the report prints when the command
  finishes, not live) and adds parse and encode time proportional to output
  size. Use `--raw` for live streaming.

The complete list of what the hook refuses is in
[usage.md](usage.md#what-the-hook-will-not-auto-approve).

## MCP server (Cursor, Codex, Windsurf, Claude Desktop)

`cartoon mcp` is a [Model Context Protocol](https://modelcontextprotocol.io)
server on stdio. Instead of asking the agent to prefix `cartoon`, it gives
the agent a `run` tool that *is* cartoon: every call goes through the same
pipeline as `cartoon -c '<command>'` (adapters, net-savings guard, raw-log
archive, stats ledger, signal forwarding — the server re-invokes its own
binary).

| Tool | Does | Like |
|---|---|---|
| `run` | `{command, cwd?, timeout_s?, compress?, max_tokens?}` → cartoon's compact report, stderr labelled, then `exit_code: N` | `cartoon -c '<command>'` |
| `logs_grep` | `{pattern, run_id?, context?}` → matching raw-log lines (default: last run) | `cartoon logs grep` |
| `logs_list` | `{limit?}` → recent runs: id, command, adapter, exit | `cartoon logs` |
| `last` | `{cmd?}` → the newest run's report, without re-running | `cartoon last` |
| `diff` | `{cmd?, id_a?, id_b?}` → fixed / still failing / new since the previous run | `cartoon diff` |
| `stats` | `{since?}` → tokens saved per adapter | `cartoon stats` |

A failing test run is a normal result (`isError: false`) whose text ends in
`exit_code: 1`; `isError: true` means the command could not run at all (a
bad `cwd`, say). `timeout_s` defaults to 600 (max 3600): on expiry cartoon
is sent SIGTERM, forwards it to the command, and still returns (and
archives) what was printed, marked `timed_out: true`; anything still alive
5 s later is SIGKILLed. Cancelling the call from the client does the same.

> **Security:** `run` executes arbitrary shell commands with your user's
> privileges — that is its purpose. The only gate is your MCP client's tool
> approval prompt; cartoon adds no allowlist and no auto-approval of its
> own. Don't put `run` on an "always allow" list you wouldn't give a plain
> shell tool. `logs_grep`, `logs_list`, `last`, `diff` and `stats` are read-only.

Register it (the binary must be on `PATH`, or give its absolute path):

```bash
# Claude Code (it has the hook too — MCP is optional there)
claude mcp add cartoon -- cartoon mcp
```

```jsonc
// Cursor: .cursor/mcp.json (project) or ~/.cursor/mcp.json (global)
// Claude Desktop: claude_desktop_config.json
// Windsurf: ~/.codeium/windsurf/mcp_config.json
{
  "mcpServers": {
    "cartoon": { "command": "cartoon", "args": ["mcp"] }
  }
}
```

```toml
# Codex: ~/.codex/config.toml
[mcp_servers.cartoon]
command = "cartoon"
args = ["mcp"]
tool_timeout_sec = 900   # Codex's default (60 s) cuts off real test suites
```

Most clients time tool calls out on their own clock; raise it (Claude Code:
`MCP_TOOL_TIMEOUT` in ms) above the longest `timeout_s` you expect. Pair the
server with the [instructions directive](#instructions-directive-one-command-or-copy-paste)
or a line in `AGENTS.md` ("run tests, builds and linters with the cartoon
`run` tool") — the agent still chooses between `run` and its own shell. The
server speaks protocol revisions 2025-06-18, 2025-03-26 and 2024-11-05;
stdout carries JSON-RPC only, diagnostics go to stderr.

## Instructions directive (one command, or copy-paste)

A directive in your instruction file (`CLAUDE.md` / `AGENTS.md`) is the
fallback for agents without the hook, and it also covers the case the hook
leaves alone: a piped compound such as `pytest | grep FAIL`. Of all pipes
the hook rewrites only a trailing `| head -n N`, `| tail -n N`,
`| xcbeautify` or `| xcpretty`, because an allowlisted segment must never
auto-approve the rest of a compound. So even with the hook installed, the
directive earns its place.

`cartoon setup` writes it for you. `cartoon instructions` manages the block
on its own (install, update, remove), wrapped in marker comments so it never
clobbers your own text:

```bash
cartoon instructions install            # → ./CLAUDE.md if present, else ./AGENTS.md
cartoon instructions install --agents   # → ./AGENTS.md (force the cross-agent file)
cartoon instructions install --claude   # → ./CLAUDE.md (force)
cartoon instructions status | uninstall | print
```

Prefer the command over hand-pasting so re-running stays idempotent.
`cartoon instructions print` emits the block to paste into any agent's
instruction file or system prompt. As of 0.8.0 it reads:

```markdown
## Noisy dev commands: run them bare under `cartoon`

Prefix tests, builds, linters and JSON CLIs with `cartoon` (`cartoon <cmd>`;
in Claude Code the hook adds it). Exit codes are mirrored and the raw output
is archived.

Never pipe them into `head`, `tail` or `grep`: the cut is lossy. The hook
only rewrites a trailing `| head -n N`, `| tail -n N`, `| xcbeautify` or
`| xcpretty`; any other pipe skips cartoon. Run them bare. `2>&1` is fine.

Need more than the report shows? Do not rerun. Read the archive:
- `cartoon logs grep "<pattern>" --last` searches the newest raw output
- `cartoon last` re-shows the newest report
- `cartoon diff` compares the last two runs (fixed, still failing, new)

Skip interactive and watch-mode commands.
```

For deterministic enforcement instead of instructions, use the hook (above):
it does not depend on the model remembering this block.

## Manual install (single agent, no tooling)

Skills are just directories with a `SKILL.md`. Copy them straight in:

```bash
# Claude Code (project-level)
cp -r skills/cartoon .claude/skills/

# Codex
cp -r skills/cartoon ~/.codex/skills/
```

## For maintainers: how the distribution works

- **One source of truth**: root `skills/<name>/SKILL.md` is the layout
  both Claude Code plugins and the skills.sh CLI consume. Edit a skill
  once; every channel picks it up.
- **Progressive disclosure**: supporting files in the skill dir (e.g.
  `skills/cartoon/install.md`) ship alongside `SKILL.md` and are loaded by
  the agent only when its link is followed — keep heavy, rarely-needed
  detail (install/setup matrix) there so it doesn't bloat always-on context.
- **Plugin + marketplace in one repo**: `.claude-plugin/plugin.json` makes
  the repo a plugin; `.claude-plugin/marketplace.json` lists that plugin
  with source `./`, so `marketplace add abhijitbansal/cartoon` is all
  users need. Bump `version` in `plugin.json` to ship an update.
- **skills.sh listing**: no submission step — the directory indexes public
  GitHub repos containing `skills/*/SKILL.md`, and CLI installs surface
  the repo on the [skills.sh](https://skills.sh) leaderboard. Discovery:
  `npx skills find cartoon`.
- **Skill frontmatter stays portable**: `name`, `description`, `license`
  only. Tool-specific keys (e.g. Claude's `disable-model-invocation`)
  would break or be ignored elsewhere; don't add them without checking
  the [Agent Skills spec](https://agentskills.io).
- **Claude community marketplace**: once the plugin has soaked, submit at
  [claude.ai/settings/plugins/submit](https://claude.ai/settings/plugins/submit)
  (run `claude plugin validate .` first) so users can install without
  adding this marketplace.
- Design and decisions:
  [docs/superpowers/specs/2026-06-10-cartoon-agent-integrations-design.md](superpowers/specs/2026-06-10-cartoon-agent-integrations-design.md).

## Troubleshooting

`cartoon doctor` reports hook status, config parse status, missing
`wrap_scripts` targets, allowlisted tools without an adapter, and ledger
health in one TOON document — paste it into a bug report. `cartoon hook
status` remains the narrow check.
