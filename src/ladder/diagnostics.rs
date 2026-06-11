use regex::Regex;
use serde_json::json;
use std::sync::OnceLock;

const MIN_DIAGNOSTICS: usize = 3;

/// Pull `file:line[:col]: severity: msg` lines into a TOON diagnostics
/// table appended to the remaining text. No-op below MIN_DIAGNOSTICS.
pub fn extract_diagnostics(text: &str) -> String {
    static PAT: OnceLock<Regex> = OnceLock::new();
    let pat = PAT.get_or_init(|| {
        Regex::new(
            r"^(?P<loc>\S+?:\d+(?::\d+)?):?\s+(?P<sev>error|warning|note)\b[:\[]?\s*(?P<msg>.*)$",
        )
        .unwrap()
    });
    let mut diags: Vec<(String, String, String)> = Vec::new();
    let mut rest: Vec<&str> = Vec::new();
    for line in text.lines() {
        match pat.captures(line) {
            Some(c) => diags.push((
                c["loc"].to_string(),
                c["sev"].to_string(),
                c["msg"].trim().to_string(),
            )),
            None => rest.push(line),
        }
    }
    if diags.len() < MIN_DIAGNOSTICS {
        return text.to_string();
    }
    let rows: Vec<_> = diags
        .iter()
        .map(|(loc, sev, msg)| json!({ "loc": loc, "severity": sev, "msg": msg }))
        .collect();
    let table = crate::toon::encode(&json!({ "diagnostics": rows }));
    let body = rest.join("\n");
    if body.trim().is_empty() {
        table
    } else {
        format!("{body}\n{table}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_gcc_shaped_diagnostics() {
        let input = "make: entering dir\nsrc/a.c:10:5: error: 'x' undeclared\nsrc/a.c:12:1: warning: unused variable 'y'\nsrc/b.c:3:9: error: expected ';'\nmake: *** [all] Error 1";
        let out = extract_diagnostics(input);
        assert!(out.contains("make: entering dir"));
        assert!(out.contains("diagnostics"));
        assert!(out.contains("src/a.c:10:5"));
        assert!(out.contains("make: *** [all] Error 1"));
    }

    #[test]
    fn below_threshold_unchanged() {
        let input = "src/a.c:10:5: error: oops\nall good otherwise";
        assert_eq!(extract_diagnostics(input), input);
    }

    #[test]
    fn prose_with_colons_unchanged() {
        let input = "note: this is prose\nsee: the manual\nhttp://example.com: a link";
        assert_eq!(extract_diagnostics(input), input);
    }
}
