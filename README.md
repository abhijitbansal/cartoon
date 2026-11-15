# cartoon

Your coding agent gets the failing test, plus file:line where the tool reports one, from xcodebuild, swift test, pytest or cargo test — not a wall of log. Exit code and full log kept.

## Before and after

Illustrative (hand-written to show the shape, not captured from one run). An
`xcodebuild test` run with two failing tests, as the agent would see it:

```text
$ xcodebuild test -scheme App -destination 'platform=iOS Simulator,name=iPhone 16'
Command line invocation:
    /Applications/Xcode.app/Contents/Developer/usr/bin/xcodebuild test -scheme App ...
Build settings from command line:
    ...
CompileSwift normal arm64 /Users/dev/proj/App/Greeter.swift (in target 'App' ...)
Test Case '-[AppTests.GreeterXCTests testGreetingExact]' started.
Test Case '-[AppTests.GreeterXCTests testGreetingExact]' failed (0.002 seconds).
... thousands more lines: build steps, one line per passing test, warnings ...
** TEST FAILED **
```

```text
$ xcodebuild test -scheme App ...      # run through cartoon
runner: xcodebuild-test
summary:
 total: 212
 passed: 210
 failed: 2
 skipped: 0
 duration_s: 41.7
failures[2]{id,loc,msg}:
 "AppTests.GreeterXCTests/testGreetingExact()","AppTests/GreeterTests.swift:14","XCTAssertEqual failed: (\"hello, world!\") is not equal to (\"hello, worldz!\")"
 "AppTests.greetingMatches()","","Expectation failed: (greeting(for: \"world\") == \"hello, worldz!\")"
raw_log: ~/.local/state/cartoon/runs/20261115-104713-736d
```

A compile error during `xcodebuild test` or `swift test` comes back as a
diagnostics table with `file:line:col` per error. The exit code is the one
xcodebuild returned, and `raw_log` points at the complete output on disk.

## Install

```bash
brew tap abhijitbansal/cartoon https://github.com/abhijitbansal/cartoon
brew install cartoon
```

or `cargo install cartoon`, or `uv tool install cartoon`. Then:

```bash
cartoon setup      # Claude Code hook + instructions block, then a health check
```

npm, `cargo binstall`, the shell installer and the plugin:
[docs/usage.md](docs/usage.md#install).

## How it compares

| | keeps failing test id | keeps file:line | exit code honest | auto-wraps bare command |
|---|---|---|---|---|
| Claude Code native preview¹ | yes below the persistence threshold; above it, only if in a ~2 KB head | same as the test id | yes | built in |
| `xcodebuild ... \| tail -40` | only if in the last 40 lines | only if in the last 40 lines | no, returns `tail`'s status unless `set -o pipefail` | you type it |
| [rtk](https://github.com/rtk-ai/rtk) | cargo/pytest/go/npm: yes; Xcode: not stated² | location for `cargo test`; others not stated² | yes³ | yes, hook rewrites |
| [xcsift](https://github.com/ldomaradzki/xcsift) | test failures reported⁴ | yes⁴ | `--exit-on-failure` flag⁴ | no, you pipe `... 2>&1 \| xcsift`⁴ |
| cartoon | yes (from the xcresult bundle, or xunit for `swift test`) | compile errors and XCTest failures yes; a Swift Testing failure carries its message, not a line | yes | yes, incl. `2>&1`, `\| tail -N`, `cd App &&` forms |

¹ Observed on Claude Code 2.1.325, one measurement each way: a 20.5 KB output was shown whole, a 102.6 KB one was replaced by a ~2 KB head, so the threshold lies between 20 KB and 100 KB. Not a documented guarantee.
² The project's README does not claim it; absence is not a claim that it cannot.
³ rtk README: "the command's exit code is passed through unchanged".
⁴ xcsift README: "test failures with file/line locations", the quick start pipes through `xcsift`,
`--exit-on-failure` is shown for CI. xcsift also ships an MCP proxy and a Claude Code installer; we have not checked what the installer rewrites.
Read from each README on 2026-11-15; check them for changes.

The honest summary: if you already pipe through xcsift or run rtk, you have
most of the effect. cartoon's difference is reading the xcresult bundle,
wrapping the bare command so the agent does not have to remember, and keeping
the whole run on disk for `cartoon logs grep`.

## Safety

The hook rewrites a command with `permissionDecision: "allow"`, which skips
Claude Code's permission prompt for that call, so it is deliberately narrow.

- It auto-approves only read-mostly dev-loop commands: test runners, linters,
  typecheckers and builds. It refuses the deploy, publish, sign and watch
  forms it recognises, xcodebuild modes other than build, test and clean, and
  modes that rewrite files (`--fix`, `-u`). A task or target name that avoids
  every word it knows (a custom gradle task, a make target) can still slip
  through. Infra CLIs (docker, kubectl, terraform, gh, aws) are never wrapped.
- It reads your Bash `ask` and `deny` rules from settings files (user,
  project and managed settings) and stays silent when one could match the
  command, any `&&` segment of it, or the tool behind a launcher
  (`uv run pytest`, `xcrun xcodebuild`). When it cannot tell, it stays
  silent. Measured on Claude Code 2.1.325: `--disallowedTools` and `dontAsk`
  mode are honoured with the hook installed. Rules the hook cannot read and
  that were not measured (`--settings`, session `/permissions` rules, Agent
  SDK options, other managed-settings locations, Windows) may not stop an
  auto-approved command.
- It never rewrites in `plan` permission mode.
- Silent means the command goes through Claude Code's normal permission flow,
  exactly as without cartoon. A project script you declare in `.cartoon.toml`
  is never auto-approved.

The full list, and how to turn it off (`CARTOON_NO_WRAP=1`, `cartoon hook
uninstall`): [docs/usage.md](docs/usage.md#what-the-hook-will-not-auto-approve).

## More

- [docs/usage.md](docs/usage.md): every flag, `-c` pipes, `--junit`, `--max-tokens`, `last` / `diff`, config, adapters, upgrading from 0.7
- [docs/agents.md](docs/agents.md): the Claude Code plugin, skills for other agents, the MCP server, the instructions block
- [benchmarks/](benchmarks/README.md): measured token reduction, with each baseline named. Against an already-quiet command such as `pytest -q` the gain is small.
- [CONTRIBUTING.md](CONTRIBUTING.md): adapters are one trait impl plus fixtures

## License

MIT
