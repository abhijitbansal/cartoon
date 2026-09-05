//! Shared `xcodebuild` invocation analysis. xcodebuild "actions" (`test`,
//! `build`, ...) are position-flexible — `xcodebuild -project X test` and
//! `xcodebuild test -project X` are both valid — so detection scans every
//! token rather than checking argv[1] like the git-style subcommand gate.
use super::basename;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Action {
    /// Produces a `.xcresult` test bundle.
    Test,
    /// Produces compiler diagnostics only.
    Build,
}

/// Flags whose following token is a VALUE, not an action — so a scheme or
/// target literally named "test"/"build" cannot trip detection.
const VALUE_FLAGS: &[&str] = &[
    "-scheme",
    "-target",
    "-project",
    "-workspace",
    "-destination",
    "-resultBundlePath",
    "-configuration",
    "-sdk",
    "-arch",
    "-derivedDataPath",
    "-xcconfig",
    "-only-testing",
    "-skip-testing",
    "-testPlan",
    "-xctestrun",
    "-toolchain",
];

/// Classify an `xcodebuild` invocation. `Test` wins when any test action is
/// present (e.g. `clean test`); `Build` only when a build action appears with
/// no test action. Returns `None` for non-xcodebuild commands or actions we
/// don't summarize (`archive`, `clean`-only, `analyze`, `-list`, ...).
pub fn action(argv: &[String]) -> Option<Action> {
    let argv = super::strip_xcrun(argv);
    let first = argv.first()?;
    if basename(first) != "xcodebuild" {
        return None;
    }
    let mut has_build = false;
    let mut skip_next = false;
    for tok in &argv[1..] {
        if skip_next {
            skip_next = false;
            continue;
        }
        if tok.starts_with('-') {
            // `-flag=value` carries its own value; `-flag value` consumes the next.
            if VALUE_FLAGS.contains(&tok.as_str()) {
                skip_next = true;
            }
            continue;
        }
        match tok.as_str() {
            "test" | "test-without-building" => return Some(Action::Test),
            "build" | "build-for-testing" => has_build = true,
            _ => {}
        }
    }
    has_build.then_some(Action::Build)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn detects_test_action_any_position() {
        assert_eq!(action(&argv(&["xcodebuild", "test"])), Some(Action::Test));
        assert_eq!(
            action(&argv(&["xcodebuild", "-project", "X.xcodeproj", "test"])),
            Some(Action::Test)
        );
        assert_eq!(
            action(&argv(&["/usr/bin/xcodebuild", "test", "-scheme", "App"])),
            Some(Action::Test)
        );
    }

    #[test]
    fn xcrun_prefix_is_transparent() {
        assert_eq!(
            action(&argv(&["xcrun", "xcodebuild", "test", "-scheme", "A"])),
            Some(Action::Test)
        );
        assert_eq!(action(&argv(&["xcrun", "simctl", "list"])), None);
    }

    #[test]
    fn test_wins_over_build_in_compound_action() {
        assert_eq!(
            action(&argv(&["xcodebuild", "clean", "test"])),
            Some(Action::Test)
        );
        assert_eq!(
            action(&argv(&[
                "xcodebuild",
                "build-for-testing",
                "test-without-building"
            ])),
            Some(Action::Test)
        );
    }

    #[test]
    fn detects_build_action() {
        assert_eq!(action(&argv(&["xcodebuild", "build"])), Some(Action::Build));
        assert_eq!(
            action(&argv(&[
                "xcodebuild",
                "-scheme",
                "App",
                "build-for-testing"
            ])),
            Some(Action::Build)
        );
    }

    #[test]
    fn value_named_like_action_does_not_trip() {
        // A scheme literally named "test" must not register as the test action.
        assert_eq!(
            action(&argv(&["xcodebuild", "-scheme", "test", "build"])),
            Some(Action::Build)
        );
        assert_eq!(action(&argv(&["xcodebuild", "-scheme", "build"])), None);
        assert_eq!(
            action(&argv(&["xcodebuild", "-only-testing", "test"])),
            None
        );
        // -xctestrun takes a path value; a file named "build" must not trip.
        assert_eq!(action(&argv(&["xcodebuild", "-xctestrun", "build"])), None);
    }

    #[test]
    fn non_summarized_actions_return_none() {
        assert_eq!(action(&argv(&["xcodebuild", "archive"])), None);
        assert_eq!(action(&argv(&["xcodebuild", "clean"])), None);
        assert_eq!(action(&argv(&["xcodebuild", "analyze"])), None);
        assert_eq!(action(&argv(&["xcodebuild", "-list"])), None);
        assert_eq!(action(&argv(&["xcodebuild", "-showBuildSettings"])), None);
        assert_eq!(action(&argv(&["swift", "test"])), None);
        assert_eq!(action(&argv(&["cargo", "build"])), None);
    }

    #[test]
    fn equals_form_value_flag_is_handled() {
        // `-resultBundlePath=foo build` — equals form does not consume next tok.
        assert_eq!(
            action(&argv(&["xcodebuild", "-resultBundlePath=foo", "build"])),
            Some(Action::Build)
        );
    }
}
