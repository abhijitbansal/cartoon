//! XCTest failure lines in a `swift test` / `xcodebuild test` log:
//! `/path/FooTests.swift:6: error: -[Module.FooTests testBar] : XCTAssertEqual failed: ...`
//! (macOS) or `/path/FooTests.swift:6: error: FooTests.testBar : ...`
//! (swift-corelibs-xctest). SwiftPM's xunit file and the xcresult summary
//! drop the location (and the xunit file the message), so the report fills
//! them from here.
use super::report::Failure;
use regex::Regex;
use std::sync::OnceLock;

/// One failure line: the test it names (`Module.Class.method`), its
/// `file:line` and its message.
struct LogFailure {
    test: String,
    loc: String,
    msg: String,
}

fn failure_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^(?P<file>[^\s:][^:]*):(?P<line>\d+): error: (?:-\[(?P<class>\S+) (?P<method>[^\]\s]+)\]|(?P<test>[\w.]+\.\w+)) : (?P<msg>.*)$",
        )
        .unwrap()
    })
}

fn log_failures(text: &str) -> Vec<LogFailure> {
    let text = crate::ladder::strip_ansi(text);
    text.lines()
        .filter_map(|line| {
            let c = failure_regex().captures(line.trim_end())?;
            let test = match (c.name("class"), c.name("method"), c.name("test")) {
                (Some(class), Some(method), _) => format!("{}.{}", class.as_str(), method.as_str()),
                (_, _, Some(t)) => t.as_str().to_string(),
                _ => return None,
            };
            Some(LogFailure {
                test,
                loc: format!("{}:{}", &c["file"], &c["line"]),
                msg: c["msg"].trim().to_string(),
            })
        })
        .collect()
}

/// `Module.Class/method()` (xcresult) and `Module.Class.method` (xunit, log)
/// in one comparable form.
fn normalise_id(id: &str) -> String {
    id.replace('/', ".").trim_end_matches("()").to_string()
}

/// The same test, allowing one side to carry a module prefix the other
/// lacks (`Class.method` vs `Module.Class.method`).
fn same_test(a: &str, b: &str) -> bool {
    a == b || a.ends_with(&format!(".{b}")) || b.ends_with(&format!(".{a}"))
}

/// A message SwiftPM writes when it has none (`message="failed"`).
fn is_placeholder(msg: &str) -> bool {
    matches!(msg.trim(), "" | "failed" | "failure")
}

/// Fill each failure's empty `loc` and empty or placeholder `msg` from the
/// first matching XCTest failure line in `text`. Nothing else changes.
pub fn fill_from_log(failures: &mut [Failure], text: &str) {
    if !failures
        .iter()
        .any(|f| f.loc.is_empty() || is_placeholder(&f.msg))
    {
        return;
    }
    let lines = log_failures(text);
    for f in failures.iter_mut() {
        let id = normalise_id(&f.id);
        let Some(hit) = lines.iter().find(|l| same_test(&id, &l.test)) else {
            continue;
        };
        if f.loc.is_empty() {
            f.loc = hit.loc.clone();
        }
        if is_placeholder(&f.msg) {
            f.msg = hit.msg.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(id: &str, loc: &str, msg: &str) -> Failure {
        Failure {
            id: id.into(),
            loc: loc.into(),
            msg: msg.into(),
            trace: Vec::new(),
        }
    }

    const LOG: &str = "\
Test Case '-[DemoTests.GreeterXCTests testGreetingExact]' started.
/Users/dev/Demo/Tests/DemoTests/DemoTests.swift:6: error: -[DemoTests.GreeterXCTests testGreetingExact] : XCTAssertEqual failed: (\"Hello\") is not equal to (\"Hi\")
Test Case '-[DemoTests.GreeterXCTests testGreetingExact]' failed (0.001 seconds).
/Users/dev/Demo/Tests/DemoTests/DemoTests.swift:9: error: GreeterXCTests.testLinux : XCTAssertTrue failed
";

    #[test]
    fn fills_a_swiftpm_xunit_failure_from_the_log() {
        let mut fs = vec![failure(
            "DemoTests.GreeterXCTests.testGreetingExact",
            "",
            "failed",
        )];
        fill_from_log(&mut fs, LOG);
        assert_eq!(
            fs[0].loc,
            "/Users/dev/Demo/Tests/DemoTests/DemoTests.swift:6"
        );
        assert_eq!(
            fs[0].msg,
            "XCTAssertEqual failed: (\"Hello\") is not equal to (\"Hi\")"
        );
    }

    #[test]
    fn fills_only_the_location_of_an_xcresult_failure() {
        let mut fs = vec![failure(
            "DemoTests.GreeterXCTests/testGreetingExact()",
            "",
            "XCTAssertEqual failed: (\"Hello\") is not equal to (\"Hi\")",
        )];
        fill_from_log(&mut fs, LOG);
        assert_eq!(
            fs[0].loc,
            "/Users/dev/Demo/Tests/DemoTests/DemoTests.swift:6"
        );
        assert!(fs[0].msg.starts_with("XCTAssertEqual failed"));
    }

    #[test]
    fn corelibs_form_and_coloured_lines_match() {
        let mut fs = vec![failure("DemoTests.GreeterXCTests.testLinux", "", "")];
        fill_from_log(&mut fs, LOG);
        assert_eq!(fs[0].msg, "XCTAssertTrue failed");
        let coloured = "/p/T.swift:3: \x1b[1;31merror: \x1b[0m-[M.C testX] : boom\x1b[0m\n";
        let mut fs = vec![failure("M.C.testX", "", "failed")];
        fill_from_log(&mut fs, coloured);
        assert_eq!(
            (fs[0].loc.as_str(), fs[0].msg.as_str()),
            ("/p/T.swift:3", "boom")
        );
    }

    #[test]
    fn unmatched_or_complete_failures_are_left_alone() {
        let mut fs = vec![
            failure("Other.Tests.testY", "", "failed"),
            failure(
                "DemoTests.GreeterXCTests.testGreetingExact",
                "a.swift:1",
                "real message",
            ),
        ];
        fill_from_log(&mut fs, LOG);
        assert_eq!((fs[0].loc.as_str(), fs[0].msg.as_str()), ("", "failed"));
        assert_eq!(
            (fs[1].loc.as_str(), fs[1].msg.as_str()),
            ("a.swift:1", "real message")
        );
    }
}
