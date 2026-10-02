use serde_json::Value;

const MAX_PARSE_ATTEMPTS: usize = 20;

/// The generic path's JSON detection: the WHOLE trimmed output must be one
/// JSON object/array, or NDJSON (every non-empty line an object/array),
/// which becomes one array. Anything else — log lines before a payload, a
/// failure line before a coverage blob — is not JSON and goes to the ladder,
/// so no prefix is ever dropped. Lines of bare scalars (`seq`, a list of
/// numbers) are deliberately not NDJSON.
pub fn detect_document(stdout: &str) -> Option<Value> {
    let trimmed = stdout.trim();
    if !trimmed.starts_with(['{', '[']) {
        return None;
    }
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return (v.is_object() || v.is_array()).then_some(v);
    }
    let mut records = Vec::new();
    for line in trimmed.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if !line.starts_with(['{', '[']) {
            return None;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(v) if v.is_object() || v.is_array() => records.push(v),
            _ => return None,
        }
    }
    (records.len() > 1).then_some(Value::Array(records))
}

/// Adapter-side detection: a JSON object/array in stdout, either the whole
/// (trimmed) output or a trailing document starting at some line (runners
/// like jest log before the payload). Never used on the generic path, where
/// dropping the prefix would lose information — see `detect_document`.
pub fn detect_json(stdout: &str) -> Option<Value> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut attempts = 0;
    let mut offset = 0;
    for line in trimmed.split_inclusive('\n') {
        let candidate = &trimmed[offset..];
        offset += line.len();
        if !candidate.starts_with(['{', '[']) {
            continue;
        }
        attempts += 1;
        if attempts > MAX_PARSE_ATTEMPTS {
            return None;
        }
        if let Ok(v) = serde_json::from_str::<Value>(candidate) {
            if v.is_object() || v.is_array() {
                return Some(v);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_output_json_object() {
        assert!(detect_json("{\"a\": 1}").is_some());
    }

    #[test]
    fn trailing_json_after_log_lines() {
        let out = "warming up...\nconnecting...\n{\"result\": [1, 2]}";
        let v = detect_json(out).unwrap();
        assert_eq!(v["result"][0], 1);
    }

    #[test]
    fn plain_text_is_none() {
        assert!(detect_json("all 48 tests passed").is_none());
    }

    #[test]
    fn bare_scalar_json_is_none() {
        assert!(detect_json("42").is_none());
    }

    #[test]
    fn document_rejects_a_json_tail_after_text() {
        let out = "test_1 ... ok\ntest_case_51 ... FAILED\n{\"coverage\": 81.5}\n";
        assert!(detect_document(out).is_none());
    }

    #[test]
    fn document_accepts_whole_output() {
        let v = detect_document("  {\"a\": [1, 2]}\n").unwrap();
        assert_eq!(v["a"][1], 2);
        assert!(detect_document("42").is_none());
        assert!(detect_document("plain").is_none());
    }

    #[test]
    fn document_encodes_ndjson_as_an_array() {
        let v = detect_document("{\"n\": 1}\n{\"n\": 2}\n\n{\"n\": 3}\n").unwrap();
        assert_eq!(v.as_array().unwrap().len(), 3);
        assert_eq!(v[0]["n"], 1);
        assert_eq!(v[2]["n"], 3);
    }

    #[test]
    fn document_rejects_ndjson_with_a_text_line() {
        assert!(detect_document("{\"n\": 1}\nboom: FAILED\n{\"n\": 2}\n").is_none());
        assert!(detect_document("1\n2\n3\n").is_none());
    }

    #[test]
    fn malformed_single_line_is_none() {
        assert!(detect_json("{not json").is_none());
    }
}
