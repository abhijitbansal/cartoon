#!/bin/sh
# SessionStart hook for the cartoon Claude Code plugin.
#
# The plugin's PreToolUse hook is a silent no-op without the cartoon binary,
# so a user who installed only the plugin gets nothing and is never told.
# This prints a one-time note (JSON additionalContext for the agent plus a
# systemMessage for the user) when the binary is missing, or when it is
# older than the plugin. Otherwise it prints nothing. It always exits 0 and
# never blocks the session.
#
# "One-time" = once per plugin version and condition: a marker under
# $XDG_STATE_HOME/cartoon/ records that the note was shown.

root=${CLAUDE_PLUGIN_ROOT:-$(dirname "$0")/..}
# Pure shell from here to the version check: each fork costs milliseconds.
want=
while IFS= read -r line; do
    case $line in
    *'"version"'*)
        want=${line#*'"version"'}
        want=${want#*'"'}
        want=${want%%'"'*}
        break
        ;;
    esac
done <"$root/.claude-plugin/plugin.json" 2>/dev/null
case "$want" in "" | *[!0-9A-Za-z.+-]*) exit 0 ;; esac

# Is version $1 older than $2? Compares major.minor.patch numerically;
# anything after a '-' or '+' is ignored.
older() {
    a=${1%%[-+]*}.0.0.0
    b=${2%%[-+]*}.0.0.0
    for _ in 1 2 3; do
        x=${a%%.*} a=${a#*.}
        y=${b%%.*} b=${b#*.}
        case "$x" in '' | *[!0-9]*) x=0 ;; esac
        case "$y" in '' | *[!0-9]*) y=0 ;; esac
        [ "$x" -lt "$y" ] && return 0
        [ "$x" -gt "$y" ] && return 1
    done
    return 1
}

if command -v cartoon >/dev/null 2>&1; then
    # `cartoon --version` prints "cartoon X.Y.Z".
    have=$(cartoon --version 2>/dev/null)
    have=${have#cartoon }
    have=${have%%[!0-9A-Za-z.+-]*}
    case "$have" in [0-9]*) ;; *) exit 0 ;; esac
    older "$have" "$want" || exit 0
    kind=outdated
    msg="cartoon plugin $want is active but the cartoon binary is $have. Upgrade with whichever tool installed it: uv tool upgrade cartoon / npm i -g cartoon-wrap@latest / cargo install cartoon / brew upgrade cartoon."
else
    kind=missing
    msg="The cartoon plugin is active but the cartoon binary is not on PATH, so its hook does nothing. Install it with one of: uv tool install cartoon / npm i -g cartoon-wrap / cargo install cartoon / brew tap abhijitbansal/cartoon https://github.com/abhijitbansal/cartoon && brew install cartoon. Do not block the user's task on this."
fi

state=${XDG_STATE_HOME:-${HOME:-/nonexistent}/.local/state}/cartoon
marker=$state/plugin-hint-$kind-$want
[ -e "$marker" ] && exit 0
# Print only when the marker can be written; otherwise the note would
# repeat every session.
mkdir -p "$state" 2>/dev/null && : >"$marker" 2>/dev/null || exit 0

printf '{"systemMessage":"%s","hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"%s"}}\n' "$msg" "$msg"
exit 0
