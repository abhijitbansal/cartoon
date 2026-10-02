//! Shared clang/swift compiler-diagnostic parsing. Both `swift build` and
//! `xcodebuild build` emit the same `path:line:col: error|warning: msg`
//! format, so the regex and collection logic live here once.
use super::{AdapterReport, ParseOutcome};
use crate::runner::Captured;
use regex::Regex;
use serde_json::{json, Value};
use std::sync::OnceLock;

fn diagnostic_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(?P<file>[^\s:][^:]*):(?P<line>\d+):(?P<col>\d+): (?P<sev>error|warning): (?P<msg>.*)$")
            .unwrap()
    })
}

/// Parsed diagnostics plus error/warning counts for one text stream.
pub fn collect(text: &str) -> (Vec<Value>, u64, u64) {
    let mut errors: u64 = 0;
    let mut warnings: u64 = 0;
    let mut diagnostics: Vec<Value> = Vec::new();

    for line in text.lines() {
        // Source echo + caret lines that follow a diagnostic never match the
        // location pattern, so they are naturally excluded.
        let Some(caps) = diagnostic_regex().captures(line) else {
            continue;
        };
        // A real path always has a separator or extension; this rejects bare
        // tokens like "1" that the loose pattern would otherwise accept.
        if !caps["file"].contains(['/', '\\', '.']) {
            continue;
        }
        let severity = &caps["sev"];
        if severity == "error" {
            errors += 1;
        } else {
            warnings += 1;
        }
        diagnostics.push(json!({
            "loc": format!("{}:{}:{}", &caps["file"], &caps["line"], &caps["col"]),
            "severity": severity,
            "msg": &caps["msg"],
        }));
    }
    (diagnostics, errors, warnings)
}

/// Location-less tool errors a build can fail on without any
/// `path:line:col: error:` diagnostic: the linker (`ld: symbol(s) not
/// found ...`, `clang: error: linker command failed ...`), code signing and
/// other driver-level `error: ...` lines. Deduplicated, in order; `ld:
/// warning:` lines are not errors and are skipped.
pub fn collect_tool_errors(text: &str) -> Vec<Value> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"^(?:(?P<tool>ld|ld64\.lld|clang|clang\+\+|swiftc|swift-frontend|swift-driver|xcodebuild): )?error: (?P<msg>.+)$|^ld: (?P<ld>.+)$",
        )
        .unwrap()
    });
    let mut seen: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        let Some(caps) = re.captures(line) else {
            continue;
        };
        if caps
            .name("ld")
            .is_some_and(|m| m.as_str().starts_with("warning:"))
        {
            continue;
        }
        let msg = line.to_string();
        if !seen.contains(&msg) {
            seen.push(msg);
        }
    }
    seen.into_iter()
        .map(|msg| json!({"loc": "", "severity": "error", "msg": msg}))
        .collect()
}

/// The shared parse for compiler-text build adapters (`swift build`,
/// `xcodebuild build`): scan both streams, separately (joining them could
/// weld a split line into a phantom diagnostic).
///
/// A failed build with no located error — a linker or signing failure,
/// possibly alongside warnings — must neither report `errors: 0` nor drop
/// the raw streams: its tool-level `error:` / `ld:` lines are counted as
/// errors and both streams pass through.
pub fn parse_build_streams(runner: &str, captured: &Captured) -> ParseOutcome {
    let (mut diags, mut errors, mut warnings) = collect(&captured.stdout);
    let (d2, e2, w2) = collect(&captured.stderr);
    diags.extend(d2);
    errors += e2;
    warnings += w2;
    let unexplained_failure = !captured.status.success() && errors == 0;
    if unexplained_failure {
        let mut tool = collect_tool_errors(&captured.stdout);
        for d in collect_tool_errors(&captured.stderr) {
            if !tool.contains(&d) {
                tool.push(d);
            }
        }
        errors += tool.len() as u64;
        diags.extend(tool);
    }
    let value = build_value(runner, diags, errors, warnings);
    ParseOutcome {
        report: AdapterReport::Value(value),
        passthrough_stdout: (unexplained_failure && !captured.stdout.is_empty())
            .then(|| captured.stdout.clone()),
        passthrough_stderr: (unexplained_failure && !captured.stderr.is_empty())
            .then(|| captured.stderr.clone()),
    }
}

/// Build the TOON `Value` for a diagnostics adapter.
pub fn build_value(runner: &str, diagnostics: Vec<Value>, errors: u64, warnings: u64) -> Value {
    let mut value = json!({
        "runner": runner,
        "summary": { "errors": errors, "warnings": warnings },
    });
    if !diagnostics.is_empty() {
        value["diagnostics"] = Value::Array(diagnostics);
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_error_and_warning_drops_caret_lines() {
        let text = "\
/Users/dev/proj/Sources/App/Auth.swift:12:5: error: cannot find 'foo' in scope
    foo()
    ^
/Users/dev/proj/Sources/App/Main.swift:3:10: warning: result of call to 'run()' is unused
    run()
    ~~~~~
";
        let (diags, errors, warnings) = collect(text);
        assert_eq!((errors, warnings), (1, 1));
        assert_eq!(diags.len(), 2);
        assert_eq!(
            diags[0]["loc"],
            "/Users/dev/proj/Sources/App/Auth.swift:12:5"
        );
        assert_eq!(diags[0]["msg"], "cannot find 'foo' in scope");
    }

    #[test]
    fn rejects_bare_number_file_token() {
        let (diags, errors, _) = collect("1:2:3: error: not a real path\n");
        assert_eq!(errors, 0);
        assert!(diags.is_empty());
    }

    #[test]
    fn note_lines_are_not_counted() {
        let (_, errors, warnings) =
            collect("/p/a.swift:1:1: note: add 'static' to make this declaration static\n");
        assert_eq!((errors, warnings), (0, 0));
    }

    #[test]
    fn tool_errors_capture_linker_and_signing_lines() {
        let text = "\
Undefined symbols for architecture arm64:
  \"_foo\", referenced from:
ld: warning: object file was built for newer macOS version
ld: symbol(s) not found for architecture arm64
clang: error: linker command failed with exit code 1 (use -v to see invocation)
clang: error: linker command failed with exit code 1 (use -v to see invocation)
error: Signing for \"App\" requires a development team.
note: Building targets in dependency order
";
        let errs = collect_tool_errors(text);
        let msgs: Vec<&str> = errs.iter().map(|d| d["msg"].as_str().unwrap()).collect();
        assert_eq!(
            msgs,
            vec![
                "ld: symbol(s) not found for architecture arm64",
                "clang: error: linker command failed with exit code 1 (use -v to see invocation)",
                "error: Signing for \"App\" requires a development team.",
            ]
        );
        assert!(errs
            .iter()
            .all(|d| d["severity"] == "error" && d["loc"] == ""));
    }

    #[test]
    fn build_value_omits_empty_diagnostics() {
        let v = build_value("swift-build", vec![], 0, 0);
        assert_eq!(v["runner"], "swift-build");
        assert_eq!(v["summary"]["errors"], 0);
        assert!(v.get("diagnostics").is_none());
    }
}
