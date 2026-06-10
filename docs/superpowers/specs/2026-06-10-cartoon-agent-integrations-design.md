# cartoon — agent skill, Claude Code plugin, and editor integrations (design)

Date: 2026-06-10
Status: implemented with this spec
Owner: Abhijit Bansal

## Problem

`cartoon` saves tokens only if the agent actually prefixes it onto commands.
Today that requires the human to paste instructions into every project's
CLAUDE.md / AGENTS.md by hand. There is no one-command install for agent
users, no guidance for the agent on when (and when not) to wrap, and no
story for tools other than Claude Code.

Separately: cartoon compresses what the agent *reads* (input tokens).
Nothing addresses what the agent *writes* (output tokens) — verbose
preambles, restated code, narrated diffs.

## Goals

1. An **agent skill** that teaches any agent to use cartoon — including
   installing it when missing — maintained in this repo as the single
   source of truth.
2. **One-command install** for Claude Code users (plugin) and for users of
   other tools (skills.sh CLI), plus copy-paste fallbacks for tools with
   no skill support (AGENTS.md, copilot-instructions.md).
3. A **caveman mode** that cuts agent *output* tokens — the complement to
   cartoon's input-token savings.
4. Documentation for all of the above.

## Decisions (made during brainstorming)

| Topic | Decision |
|---|---|
| Skill location | Root `skills/<name>/SKILL.md` — the layout both Claude Code plugins and the skills.sh CLI consume; one source of truth, zero duplication |
| Claude Code distribution | This repo doubles as plugin **and** marketplace: `.claude-plugin/plugin.json` + `.claude-plugin/marketplace.json` with plugin source `./`. Install: `/plugin marketplace add abhijitbansal/cartoon` then `/plugin install cartoon@cartoon` |
| Other tools (Codex, Copilot, Cursor, …) | skills.sh CLI: `npx skills add abhijitbansal/cartoon` — detects installed agents and routes skills to each tool's directory (40+ agents supported) |
| No-skill-support fallback | Documented copy-paste blocks for `AGENTS.md` and `.github/copilot-instructions.md` in `docs/agents.md` |
| Skills shipped | `cartoon` (use + install + when-not-to-use) and `caveman` (terse output mode) |
| caveman mechanism | A skill, not a hook/output style: invoked it injects terse-output rules into context for the rest of the session. In Claude Code it's `/cartoon:caveman` (slash invocation of plugin skill); elsewhere model-invoked via description match or the tool's own skill command. Output styles were considered and rejected (Claude-only, deprecated path); a skill is portable everywhere |
| `commands/` directory | Not used — Claude Code docs mark it legacy ("Skills as flat Markdown files. Use `skills/` for new plugins") |
| Skill self-install policy | Skill instructs the agent to check `command -v cartoon` and install via whichever toolchain exists (`uv`/`pipx`/`npm`/`cargo`), ask-first when the agent's environment requires permission anyway |
| skills.sh listing | No submission step: directory indexes public GitHub repos with `skills/*/SKILL.md`; installs via the CLI surface it on the leaderboard. Documented in `docs/agents.md` |
| Community marketplace | Submit later via claude.ai/settings/plugins/submit once the plugin has soaked; `claude plugin validate` wired into CI is out of scope for now |
| Docs home | `docs/agents.md` (full integration guide) + "For agents" section in README |

## Architecture

```
cartoon/  (this repo = plugin = marketplace = skills source)
├── .claude-plugin/
│   ├── plugin.json          # plugin manifest (name: cartoon)
│   └── marketplace.json     # marketplace listing this repo (source: ./)
├── skills/
│   ├── cartoon/SKILL.md     # wrap CLIs in cartoon; install if missing
│   └── caveman/SKILL.md     # terse output mode (output-token savings)
└── docs/agents.md           # install matrix for every tool
```

Install paths, by tool:

| Tool | Path |
|---|---|
| Claude Code (plugin) | `/plugin marketplace add abhijitbansal/cartoon` → `/plugin install cartoon@cartoon` → skills available as `/cartoon:cartoon`, `/cartoon:caveman` and model-invoked |
| Claude Code (skills only) | `npx skills add abhijitbansal/cartoon -a claude-code` |
| Codex / Copilot / Cursor / Windsurf / opencode / … | `npx skills add abhijitbansal/cartoon` (CLI auto-detects agents) |
| Anything else | Copy-paste block from `docs/agents.md` into AGENTS.md / system instructions |

## Skill design

### `skills/cartoon/SKILL.md`

Frontmatter: `name`, `description` only (portable subset of the Agent
Skills spec; tool-specific keys avoided). Description front-loads trigger
phrases: running tests, pytest/jest/unittest, JSON CLIs, token-heavy
output.

Body covers, in order of agent need:

1. **Use**: prefix `cartoon` onto test runs and JSON CLIs; flag reference.
2. **Install if missing**: `command -v cartoon` gate, then first available
   of `uv tool install cartoon` / `pipx install cartoon` /
   `npm install -g cartoon-wrap` / `cargo install cartoon`; verify with
   `cartoon adapters`.
3. **Guarantees** (why it's safe to wrap): exit codes mirrored, parse
   failure = passthrough, args never reordered.
4. **When not to wrap**: interactive/TTY commands, commands whose full
   human output the user asked to see, already-terse commands;
   `--raw` escape hatch.
5. **Reporting**: `cartoon stats` for savings.

### `skills/caveman/SKILL.md`

Caveman speak: few token, much meaning. Invoked (by user slash command or
model match on "be terse / save output tokens"), the agent adopts
output rules for the rest of the session: lead with the answer, no
preamble/recap/restated code, fragments over prose, no pleasantries —
correctness and code quality explicitly exempt. Includes an off switch
("caveman off"). This is prompt-context, not configuration: nothing
persists beyond the session, which is the right blast radius for a
style mode.

## Out of scope (explicit)

- Publishing skills to the npm/PyPI packages (`cartoon skill install`
  subcommand emitting SKILL.md) — attractive later; needs the binary to
  carry the markdown. Revisit on demand.
- Hooks that auto-rewrite Bash tool calls to prefix cartoon (PreToolUse
  rewrite) — powerful but surprising; opt-in skill guidance first.
- Output style / statusline integrations — Claude-only, low value vs skill.
- `claude plugin validate` in CI and community-marketplace submission —
  follow-up once structure settles.

## Success criteria

- Fresh Claude Code session: marketplace add + install yields
  `/cartoon:cartoon` and `/cartoon:caveman`, and the model wraps a pytest
  run without being told.
- `npx skills add abhijitbansal/cartoon` lists both skills and installs
  into at least Claude Code, Codex, and Cursor layouts.
- A tool with no skill support can be onboarded from `docs/agents.md`
  with one copy-paste.
