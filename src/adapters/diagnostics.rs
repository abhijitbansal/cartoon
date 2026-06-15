//! Shared clang/swift compiler-diagnostic parsing. Both `swift build` and
//! `xcodebuild build` emit the same `path:line:col: error|warning: msg`
//! format, so the regex and collection logic live here once.
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
    fn build_value_omits_empty_diagnostics() {
        let v = build_value("swift-build", vec![], 0, 0);
        assert_eq!(v["runner"], "swift-build");
        assert_eq!(v["summary"]["errors"], 0);
        assert!(v.get("diagnostics").is_none());
    }
}
