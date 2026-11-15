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
    /// `archive` / `-exportArchive`: same diagnostics as a build, but the
    /// action signs and packages — the hook never auto-approves it.
    Archive,
}

/// Flags whose following token is a VALUE, not an action — so a scheme,
/// target or path literally named "test"/"build" cannot trip detection. The
/// hook's xcodebuild allowlist (`hook::policy`) skips the same values.
pub const VALUE_FLAGS: &[&str] = &[
    "-scheme",
    "-target",
    "-project",
    "-workspace",
    "-destination",
    "-destination-timeout",
    "-resultBundlePath",
    "-resultStreamPath",
    "-configuration",
    "-sdk",
    "-arch",
    "-derivedDataPath",
    "-xcconfig",
    "-only-testing",
    "-skip-testing",
    "-only-test-configuration",
    "-skip-test-configuration",
    "-testPlan",
    "-testProductsPath",
    "-xctestrun",
    "-toolchain",
    "-jobs",
    "-archivePath",
    "-exportPath",
    "-exportOptionsPlist",
    "-localizationPath",
    "-exportLanguage",
    "-enableCodeCoverage",
    "-enableAddressSanitizer",
    "-enableThreadSanitizer",
    "-enableUndefinedBehaviorSanitizer",
    "-test-iterations",
    "-test-timeouts-enabled",
    "-test-repetition-relaunch-enabled",
    "-parallel-testing-enabled",
    "-parallel-testing-worker-count",
    "-maximum-parallel-testing-workers",
    "-maximum-concurrent-test-device-destinations",
    "-maximum-concurrent-test-simulator-destinations",
    "-default-test-execution-time-allowance",
    "-maximum-test-execution-time-allowance",
    "-collect-test-diagnostics",
    "-testLanguage",
    "-testRegion",
    "-clonedSourcePackagesDirPath",
    "-packageCachePath",
    "-scmProvider",
    "-resultBundleVersion",
    "-defaultLanguage",
    "-codesizeProfileOutputDir",
    "-enableCodesizeProfile",
    "-enablePerformanceTestsDiagnostics",
    "-packageAuthorizationProvider",
    "-packageFingerprintPolicy",
    "-packageSigningEntityPolicy",
];

/// A flag that switches xcodebuild out of its build/test actions into
/// another mode (export, import, download, delete, first-launch setup,
/// package resolution, framework packaging, project conversion). New Xcode releases add more
/// `-export*`/`-import*`/`-download*` modes, so those prefixes all count
/// (except the value flags `-exportPath`, `-exportOptionsPlist`, ...).
pub fn is_mode_switch(tok: &str) -> bool {
    let Some(name) = tok.strip_prefix('-') else {
        return false;
    };
    if VALUE_FLAGS.contains(&tok) {
        return false;
    }
    let lower = name.to_ascii_lowercase();
    ["export", "import", "download"]
        .iter()
        .any(|p| lower.starts_with(p))
        || matches!(
            lower.as_str(),
            "create-xcframework"
                | "runfirstlaunch"
                | "checkfirstlaunchstatus"
                | "checkfornewercomponents"
                | "resolvepackagedependencies"
                | "deletecomponent"
                | "showcomponent"
                | "preparedevicesupport"
                | "convert-project"
                | "find-executable"
                | "find-library"
                | "enumerate-tests"
                | "license"
        )
}

/// Classify an `xcodebuild` invocation. `Archive` wins when
/// `archive`/`-exportArchive`/`-exportNotarizedApp` appears anywhere, even
/// next to a test action, so an export is never summarized as a test run;
/// then `Test` when any test action is present (e.g. `clean test`); `Build`
/// only when a build action appears alone. Returns `None` for
/// non-xcodebuild commands, other mode switches (`-importLocalizations`,
/// `-downloadAllPlatforms`, ...) and actions we don't summarize
/// (`clean`-only, `analyze`, `-list`, ...).
pub fn action(argv: &[String]) -> Option<Action> {
    let argv = super::strip_xcrun(argv);
    let first = argv.first()?;
    if basename(first) != "xcodebuild" {
        return None;
    }
    let (mut has_test, mut has_build, mut has_archive) = (false, false, false);
    let mut skip_next = false;
    for tok in &argv[1..] {
        if skip_next {
            skip_next = false;
            continue;
        }
        if matches!(tok.as_str(), "-exportArchive" | "-exportNotarizedApp") {
            has_archive = true;
            continue;
        }
        if is_mode_switch(tok) {
            return None;
        }
        if tok.starts_with('-') {
            // `-flag=value` carries its own value; `-flag value` consumes the next.
            if VALUE_FLAGS.contains(&tok.as_str()) {
                skip_next = true;
            }
            continue;
        }
        match tok.as_str() {
            "test" | "test-without-building" => has_test = true,
            "build" | "build-for-testing" => has_build = true,
            "archive" => has_archive = true,
            _ => {}
        }
    }
    if has_archive {
        Some(Action::Archive)
    } else if has_test {
        Some(Action::Test)
    } else {
        has_build.then_some(Action::Build)
    }
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
    fn archive_and_export_archive_classify_as_archive() {
        assert_eq!(
            action(&argv(&["xcodebuild", "archive", "-scheme", "App"])),
            Some(Action::Archive)
        );
        assert_eq!(
            action(&argv(&[
                "xcodebuild",
                "-exportArchive",
                "-archivePath",
                "A.xcarchive"
            ])),
            Some(Action::Archive)
        );
        // Archive and export win over a stray `test` word: an export is never
        // summarized as a test run (final review C1).
        assert_eq!(
            action(&argv(&["xcodebuild", "clean", "archive", "test"])),
            Some(Action::Archive)
        );
        assert_eq!(
            action(&argv(&[
                "xcodebuild",
                "-exportArchive",
                "-archivePath",
                "a",
                "-exportPath",
                "exp",
                "test"
            ])),
            Some(Action::Archive)
        );
        assert_eq!(
            action(&argv(&["xcodebuild", "-exportNotarizedApp", "test"])),
            Some(Action::Archive)
        );
    }

    #[test]
    fn other_mode_switches_are_not_summarized() {
        for mode in [
            "-importLocalizations",
            "-exportLocalizations",
            "-downloadAllPlatforms",
            "-runFirstLaunch",
            "-resolvePackageDependencies",
            "-create-xcframework",
        ] {
            assert_eq!(action(&argv(&["xcodebuild", mode, "test"])), None, "{mode}");
        }
    }

    #[test]
    fn archive_path_value_is_not_an_action() {
        assert_eq!(
            action(&argv(&["xcodebuild", "build", "-archivePath", "test"])),
            Some(Action::Build)
        );
        assert_eq!(
            action(&argv(&["xcodebuild", "-enableCodeCoverage", "YES", "test"])),
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
