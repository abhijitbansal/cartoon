# Using cartoon with coding agents

cartoon's whole point is agents, so the skills that teach an agent to use
it live in this repo under [`skills/`](../skills/) and install with one
command into every major tool.

Two skills ship today:

| Skill | Saves | What it does |
|---|---|---|
| [`cartoon`](../skills/cartoon/SKILL.md) | input tokens | Teaches the agent to prefix `cartoon` onto test runs and JSON CLIs, and to install the binary if missing |
| [`caveman`](../skills/caveman/SKILL.md) | output tokens | Terse-reply mode: answer first, no narration, no restated code. "caveman off" disables |

They're complementary: cartoon compresses what the agent reads, caveman
compresses what it writes.

## Claude Code (plugin — recommended)

This repo is a Claude Code plugin marketplace. Inside Claude Code:

```
/plugin marketplace add abhijitbansal/cartoon
/plugin install cartoon@cartoon
```

You get both skills, model-invoked (Claude wraps your test runs without
being asked) and as slash commands:

```
/cartoon:cartoon    # load the usage/install guidance explicitly
/cartoon:caveman    # terse output mode for the rest of the session
```

Update later with `/plugin marketplace update cartoon`.

## Codex, Copilot, Cursor, Windsurf, opencode, … (skills.sh)

The [skills.sh](https://skills.sh) CLI installs skills from any GitHub
repo into 40+ agents, auto-detecting which ones you have:

```bash
npx skills add abhijitbansal/cartoon              # interactive: pick skills + agents
npx skills add abhijitbansal/cartoon --all        # both skills, all detected agents
npx skills add abhijitbansal/cartoon --skill cartoon -a codex -a cursor
npx skills list                                   # see what's installed where
```

This also works for Claude Code (`-a claude-code`) if you prefer plain
skills over the plugin.

## Tools without skill support (copy-paste)

Add this to your `AGENTS.md`, `.github/copilot-instructions.md`, or
system instructions:

```markdown
## CLI output: use cartoon

Prefix `cartoon` onto test runs and JSON-emitting CLIs to get
token-optimized TOON output (~70% smaller, failures keep full detail):
`cartoon pytest`, `cartoon npx jest`, `cartoon aws s3api list-buckets`.
Exit codes are mirrored exactly and parse failures pass the original
output through, so wrapping is always safe. Don't wrap interactive
commands. If `cartoon` is not installed: `uv tool install cartoon`
(or `pipx install cartoon` / `npm i -g cartoon-wrap` / `cargo install cartoon`).
```

## Manual install (single agent, no tooling)

Skills are just directories with a `SKILL.md`. Copy them straight in:

```bash
# Claude Code (project-level)
cp -r skills/cartoon skills/caveman .claude/skills/

# Codex
cp -r skills/cartoon skills/caveman ~/.codex/skills/
```

## For maintainers: how the distribution works

- **One source of truth**: root `skills/<name>/SKILL.md` is the layout
  both Claude Code plugins and the skills.sh CLI consume. Edit a skill
  once; every channel picks it up.
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
