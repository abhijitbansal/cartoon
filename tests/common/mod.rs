//! Tool-presence checks shared by the e2e tests.
//!
//! A real-tool e2e test returns early ("SKIP") when its tool is missing, so a
//! machine without jest used to report `e2e_jest_failing_suite` as passed
//! without running it. With `CARTOON_E2E_STRICT=1` (set in CI) a missing tool
//! is a panic instead. Apple-only tools stay skippable everywhere, and
//! `CARTOON_E2E_ALLOW_MISSING=a,b` exempts more by name.
#![allow(dead_code)]

/// Tools that only exist on macOS: strict mode never requires them, so the
/// Linux CI job can run strict.
const APPLE_ONLY: &[&str] = &["xcodebuild", "xcrun", "swift"];

/// Is `cmd` runnable (`cmd --version` exits 0)? Panics instead of returning
/// false when strict mode requires the tool.
pub fn have(cmd: &str) -> bool {
    let ok = std::process::Command::new(cmd)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        missing(cmd);
    }
    ok
}

/// Report a missing prerequisite (`what` is a tool or module name): panics
/// under strict mode, otherwise a no-op so the caller can skip.
pub fn missing(what: &str) {
    let strict = std::env::var("CARTOON_E2E_STRICT").ok();
    let allow = std::env::var("CARTOON_E2E_ALLOW_MISSING").ok();
    if strict_requires(what, strict.as_deref(), allow.as_deref()) {
        panic!(
            "CARTOON_E2E_STRICT=1 but {what} is not installed: install it, \
             or list it in CARTOON_E2E_ALLOW_MISSING"
        );
    }
}

/// Pure decision behind `missing`, kept separate so it can be tested without
/// touching the process environment.
pub fn strict_requires(what: &str, strict: Option<&str>, allow: Option<&str>) -> bool {
    if strict != Some("1") || APPLE_ONLY.contains(&what) {
        return false;
    }
    !allow
        .unwrap_or("")
        .split(',')
        .any(|name| name.trim() == what)
}
