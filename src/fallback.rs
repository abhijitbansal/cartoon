use serde_json::Value;

const MAX_PARSE_ATTEMPTS: usize = 20;

/// Detect a JSON object/array in stdout: either the whole (trimmed) output,
/// or a trailing document starting at some line (CLIs often log before the payload).
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
}
