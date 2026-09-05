use regex::Regex;
use serde_json::json;
use std::sync::OnceLock;

const MIN_DIAGNOSTICS: usize = 3;
/// rustc prints `error[E0308]: msg` with the location on a following
/// ` --> file:line:col` line, within this many lines of the header.
const RUSTC_ARROW_LOOKAHEAD: usize = 2;

fn gcc_pat() -> &'static Regex {
    static PAT: OnceLock<Regex> = OnceLock::new();
    PAT.get_or_init(|| {
        Regex::new(
            r"^(?P<loc>\S+?:\d+(?::\d+)?):?\s+(?P<sev>error|warning|note)\b[:\[]?\s*(?P<msg>.*)$",
        )
        .unwrap()
    })
}

fn rustc_header_pat() -> &'static Regex {
    static PAT: OnceLock<Regex> = OnceLock::new();
    PAT.get_or_init(|| {
        Regex::new(r"^(?P<sev>error|warning)(?:\[(?P<code>[A-Z]+\d+)\])?: (?P<msg>.+)$").unwrap()
    })
}

fn rustc_arrow_pat() -> &'static Regex {
    static PAT: OnceLock<Regex> = OnceLock::new();
    PAT.get_or_init(|| Regex::new(r"^\s*-->\s+(?P<loc>\S+:\d+(?::\d+)?)\s*$").unwrap())
}

/// True for a line the extractor would treat as a compiler diagnostic
/// (single-line gcc/clang shape or a rustc `error[E…]:` header). Used by
/// `collapse_near_dups` so three same-message diagnostics that differ only
/// by line number are never templated into one before extraction sees them.
pub fn is_diagnostic_line(line: &str) -> bool {
    gcc_pat().is_match(line) || rustc_header_pat().is_match(line)
}

struct Diag {
    loc: String,
    sev: String,
    msg: String,
}

/// Pull compiler diagnostics into a TOON table appended to the remaining
/// text. Two shapes: single-line `file:line[:col]: severity: msg`
/// (gcc/clang/eslint) and rustc's multi-line block (`error[Exxxx]: msg`
/// followed by ` --> loc`). rustc snippet/help lines until the blank line
/// are elided: the aggressive tier is lossy and raw_log keeps them.
/// No-op below MIN_DIAGNOSTICS total.
pub fn extract_diagnostics(text: &str) -> String {
    let sep = super::safe::line_sep(text);
    let lines: Vec<&str> = text.lines().collect();
    let mut diags: Vec<Diag> = Vec::new();
    let mut rest: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(c) = gcc_pat().captures(line) {
            diags.push(Diag {
                loc: c["loc"].to_string(),
                sev: c["sev"].to_string(),
                msg: c["msg"].trim().to_string(),
            });
            i += 1;
            continue;
        }
        if let Some(c) = rustc_header_pat().captures(line) {
            // Only a diagnostic if a `--> loc` arrow follows shortly;
            // plain `error: could not compile ...` summaries stay in body.
            let arrow = (i + 1..=(i + RUSTC_ARROW_LOOKAHEAD).min(lines.len() - 1))
                .find_map(|j| rustc_arrow_pat().captures(lines[j]).map(|a| (j, a)));
            if let Some((aj, a)) = arrow {
                let sev = match c.name("code") {
                    Some(code) => format!("{}[{}]", &c["sev"], code.as_str()),
                    None => c["sev"].to_string(),
                };
                diags.push(Diag {
                    loc: a["loc"].to_string(),
                    sev,
                    msg: c["msg"].trim().to_string(),
                });
                // Skip the block (snippet/help/note) until the blank line.
                let mut j = aj + 1;
                while j < lines.len() && !lines[j].trim().is_empty() {
                    j += 1;
                }
                i = j;
                continue;
            }
        }
        rest.push(line);
        i += 1;
    }
    if diags.len() < MIN_DIAGNOSTICS {
        return text.to_string();
    }
    let rows: Vec<_> = diags
        .iter()
        .map(|d| json!({ "loc": d.loc, "severity": d.sev, "msg": d.msg }))
        .collect();
    let table = crate::toon::encode(&json!({ "diagnostics": rows }));
    let body = rest.join(sep);
    if body.trim().is_empty() {
        table
    } else {
        format!("{body}{sep}{table}")
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

    #[test]
    fn extracts_rustc_blocks_keeps_summary() {
        let input = "   Compiling demo v0.1.0\nerror[E0425]: cannot find value `c` in this scope\n --> src/lib.rs:2:9\n  |\n2 |     a + c\n  |         ^\n\nerror[E0308]: mismatched types\n  --> src/lib.rs:10:5\n   |\n10 |     \"oops\"\n\nwarning: unused variable: `y`\n --> src/lib.rs:4:9\n  |\n\nerror: could not compile `demo` (lib) due to 2 previous errors";
        let out = extract_diagnostics(input);
        assert!(out.contains("diagnostics"));
        assert!(out.contains("src/lib.rs:2:9"));
        assert!(out.contains("error[E0425]"));
        assert!(out.contains("cannot find value `c` in this scope"));
        // summary line is not file:line shaped and has no arrow: stays in body
        assert!(out.contains("error: could not compile `demo` (lib) due to 2 previous errors"));
        // snippet lines elided
        assert!(!out.contains("a + c"));
    }

    #[test]
    fn rustc_summary_without_arrow_not_extracted() {
        let input = "error: could not compile `demo`\nplain line\nanother plain line";
        assert_eq!(extract_diagnostics(input), input);
    }
}
