# Using cartoon with coding agents

cartoon's whole point is agents, so the skill that teaches an agent to use
it lives in this repo under [`skills/`](../skills/) and installs with one
command into every major tool.

The [`cartoon` skill](../skills/cartoon/SKILL.md) teaches the agent to
prefix `cartoon` onto test runs and JSON CLIs, to install the binary if
missing, and when not to wrap.

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

## Codex, Copilot, Cursor, Windsurf, opencode, … (skills.sh)

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

## Guaranteed wrapping: hooks (and shims)

Skills and `AGENTS.md` / `copilot-instructions.md` only *ask* the agent to
prefix `cartoon`; the model complies probabilistically and sometimes
forgets. A **`PreToolUse` hook** removes the guesswork — it intercepts the
tool call and rewrites it deterministically. cartoon ships one
`cartoon hook rewrite` that auto-detects the agent from the event shape, so
the same command works across agents; you just install it where each agent
looks.

| Agent | Install | Mechanism | Config location |
|---|---|---|---|
| Claude Code | `cartoon hook install` | transparent rewrite (`updatedInput`) | `~/.claude/settings.json` |
| VS Code Copilot Chat | `cartoon hook install --vscode` | deny + "re-run wrapped" suggestion¹ | `~/.claude/settings.json`² |
| Copilot CLI (≥ v1.0.24) | `cartoon hook install --copilot` | transparent rewrite (`updatedInput`) | `~/.copilot/hooks/cartoon.json` |
| Copilot CLI / coding agent (repo-shared) | `cartoon hook install --copilot --project` | same | `.github/hooks/cartoon.json`³ |

¹ VS Code Copilot Chat exposes no command-rewrite field, only
allow/ask/deny, so the hook blocks the raw command and tells the agent to
re-run it wrapped — deterministic, with one extra round-trip.

² VS Code Copilot Chat reads the **same Claude-format settings file** (per
Microsoft's own hooks reference; the feature is Preview), so `--vscode`
installs there. One entry already covers both VS Code Chat and Claude Code —
`--vscode` is the explicit, VS-Code-labelled form of `cartoon hook install`,
so don't run both into the same scope.

³ The repo-shared file is **Copilot-CLI format** (`{"version":1,"hooks":
{"preToolUse":[…]}}`, camelCase) for the Copilot CLI and coding agent. VS
Code Chat expects the Claude format (`{"hooks":{"PreToolUse":[…]}}`,
PascalCase), so it does **not** read this file — use `--vscode` for Chat.

```bash
cartoon hook status     # what's installed, per agent
cartoon hook uninstall [--copilot | --vscode] [--project]
```

Notes and knobs:

- **Same conservative allowlist as Claude Code**: test runners, linters,
  typecheckers, builds only. Infra CLIs (docker, kubectl, terraform, gh,
  aws) and mutating subcommands (`cargo publish`, `npm install`) are never
  wrapped; the net-savings guard means worst case is byte-identical output.
- **Copilot confirmation-dialog bug**: some Copilot CLI builds prompt on
  every rewritten command even when the hook approves it. If you hit that,
  install with `--deny` (`cartoon hook install --copilot --deny`) for the
  smoother deny-and-suggest path. `cartoon hook rewrite --deny-mode` forces
  deny anywhere.
- **Install to `~/.copilot/hooks` or `.github/hooks`**, not a plugin
  directory — plugin-defined Copilot hooks currently don't fire.
- **Disable without uninstalling**: set `CARTOON_NO_WRAP=1` (hook) or
  `CARTOON_NO_SHIM=1` (shims) in the environment your agent runs in — any
  non-empty value turns auto-wrap off for that session; unset to re-enable.
  Permanent: `cartoon hook uninstall` / `cartoon shim uninstall`. One command
  raw: `cartoon --raw <cmd>`.
- **Overhead**: the per-command check is a tiny fail-open process
  (negligible). When wrapping, cartoon **buffers** output — the report prints
  when the command finishes, not live — and adds parse/encode time
  proportional to output size (milliseconds in practice). Use `--raw` for
  live streaming.

### No hook available? Shell shims

For agents without hooks, or as a belt-and-suspenders layer, `cartoon shim`
writes shell functions that shadow the bare tool names and re-invoke them
through cartoon. Functions take precedence over PATH *and* venv-local
binaries, so wrapping happens with zero agent cooperation:

```bash
cartoon shim install     # writes ~/.config/cartoon/shims.sh + activation help
cartoon shim print       # functions to stdout: eval "$(cartoon shim print)"
```

Activate for the non-interactive shells agents spawn:
`export BASH_ENV=~/.config/cartoon/shims.sh`. Disable per-shell with
`CARTOON_NO_SHIM=1`. Shims reuse the hook's allowlist, but a shell function
sees only its own argv (not surrounding pipes/redirection), so keep them to
tools the agent runs bare; path-invoked binaries (`./gradlew`) aren't
caught. Tools with non-identifier names (`pre-commit`) and `python -m` /
`xcodebuild` invocations aren't shimmed either — the hook covers those.

## Instructions directive (one command, or copy-paste)

A directive in your instruction file (`CLAUDE.md` / `AGENTS.md`) is not just
the no-hook fallback — it's also the **only** thing that covers the
piped-command case, because the hook won't
rewrite `pytest | tail` (an allowlisted segment must never auto-approve the
rest of a compound) and VS Code Copilot Chat can only deny, not rewrite. So
even with a hook installed, the directive earns its place.

`cartoon instructions` writes (and updates, and cleanly removes) the block
below for you, wrapped in marker comments so it never clobbers your own text:

```bash
cartoon instructions install            # → ./CLAUDE.md if present, else ./AGENTS.md
cartoon instructions install --agents   # → ./AGENTS.md (force the cross-agent file)
cartoon instructions install --copilot  # → ./.github/copilot-instructions.md
cartoon instructions install --claude   # → ./CLAUDE.md (force)
cartoon instructions status | uninstall | print
```

`cartoon hook install --instructions` does both layers in one step, and a
plain `cartoon hook install` hints about the gap (and, on a terminal, offers
to write the directive for you). Prefer the command over hand-pasting so
re-running stays idempotent. To paste it yourself, add this to your
`AGENTS.md`, `.github/copilot-instructions.md`, or system instructions —
phrased as an unconditional rule so the model treats it as a hard directive,
not a suggestion:

```markdown
## CLI output: ALWAYS run noisy commands through cartoon

This is a hard rule, not a suggestion. Before running any test, build,
lint, typecheck, or JSON-emitting CLI, prefix it with `cartoon` — there is
no case where the bare command is preferable. Wrapping is always safe: exit
codes are mirrored exactly, parse failures pass the original output
through, args are never reordered, and a net-savings guard means the output
never gets bigger.

ALWAYS prefix these (run them bare and you have made a mistake):
`pytest`, `python -m pytest`, `unittest`, `jest`, `vitest`, `tsc`,
`eslint`, `ruff`, `mypy`, `make`, `cargo build|test|check|clippy`,
`go test|build|vet`, `npm test`, `swift test|build`, `xcodebuild
test|build`, the same tools run through uv (`uv run pytest`, `uvx ruff
check`, `uv run -m pytest`), and any `... --output json` CLI (`aws`, `gh`,
`kubectl`). Examples: `cartoon pytest -q`, `cartoon uv run pytest`,
`cartoon npx jest src/`, `cartoon make`,
`cartoon aws ec2 describe-instances --output json`. Existing logs:
`cartoon ingest build.log` or `cmd | cartoon -`.

Do NOT pipe a noisy command to `head`/`tail`/`grep` to shrink it — wrap it
instead, then search the archived raw log with
`cartoon logs grep <pattern> --last`. Don't wrap interactive/TTY commands.
If `cartoon` is not installed: `uv tool install cartoon` (or `pipx install
cartoon` / `npm i -g cartoon-wrap` / `cargo install cartoon`).
```

For deterministic enforcement instead of instructions, prefer a hook or
shim (above) — they don't depend on the model remembering this block.

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
