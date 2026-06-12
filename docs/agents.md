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

## Tools without skill support (copy-paste)

Add this to your `AGENTS.md`, `.github/copilot-instructions.md`, or
system instructions:

```markdown
## CLI output: use cartoon

Prefix `cartoon` onto noisy commands to get token-optimized output:
test runs and JSON CLIs become compact TOON reports (~70% smaller,
failures keep full detail), and everything else compresses through a
safe deterministic tier automatically: `cartoon pytest`,
`cartoon npx jest`, `cartoon make`, `cartoon aws s3api list-buckets`.
Existing logs work too: `cartoon ingest build.log` or `cmd | cartoon -`.
Exit codes are mirrored exactly, parse failures pass the original output
through, and a net-savings guard means output never gets bigger — so
wrapping is always safe. Full raw output is archived per run; search it
with `cartoon logs grep <pattern> --last` instead of re-running or
cat-ing the log. Don't wrap interactive commands. If `cartoon` is not
installed: `uv tool install cartoon` (or `pipx install cartoon` /
`npm i -g cartoon-wrap` / `cargo install cartoon`).
```

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
