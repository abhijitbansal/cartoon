use serde_json::Value;

/// Encode a JSON value as TOON. Returns lines joined by '\n', no trailing newline.
pub fn encode(value: &Value) -> String {
    let mut lines: Vec<String> = Vec::new();
    match value {
        Value::Object(_) | Value::Array(_) => container_lines(value, &mut lines),
        v => lines.push(scalar(v)),
    }
    lines.join("\n")
}

fn container_lines(_value: &Value, _lines: &mut Vec<String>) {
    unimplemented!("Tasks 5 and 6")
}

pub(crate) fn scalar(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => {
            if needs_quotes(s) {
                quote(s)
            } else {
                s.to_string()
            }
        }
        _ => unreachable!("scalar() called on container"),
    }
}

fn needs_quotes(s: &str) -> bool {
    s.is_empty()
        || s.trim() != s
        || matches!(s, "true" | "false" | "null")
        || s.parse::<f64>().is_ok()
        || s.contains([',', ':', '"', '\\', '\n', '\r', '\t'])
        || s.starts_with(['-', '[', ']', '{', '}', '#'])
}

pub(crate) fn quote(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scalars() {
        assert_eq!(encode(&json!(42)), "42");
        assert_eq!(encode(&json!(3.5)), "3.5");
        assert_eq!(encode(&json!(true)), "true");
        assert_eq!(encode(&json!(null)), "null");
        assert_eq!(encode(&json!("plain")), "plain");
    }

    #[test]
    fn strings_quoted_when_ambiguous() {
        assert_eq!(encode(&json!("")), "\"\"");
        assert_eq!(encode(&json!("42")), "\"42\"");
        assert_eq!(encode(&json!("true")), "\"true\"");
        assert_eq!(encode(&json!("a, b")), "\"a, b\"");
        assert_eq!(encode(&json!("k: v")), "\"k: v\"");
        assert_eq!(encode(&json!(" padded")), "\" padded\"");
        assert_eq!(encode(&json!("line\nbreak")), "\"line\\nbreak\"");
        assert_eq!(encode(&json!("say \"hi\"")), "\"say \\\"hi\\\"\"");
    }
}
