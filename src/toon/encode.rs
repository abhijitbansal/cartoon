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

fn container_lines(value: &Value, lines: &mut Vec<String>) {
    match value {
        Value::Object(map) => object_lines(map, 0, lines),
        Value::Array(arr) => array_lines(None, arr, 0, lines),
        _ => unreachable!(),
    }
}

fn indent(depth: usize) -> String {
    "  ".repeat(depth)
}

fn object_lines(map: &serde_json::Map<String, Value>, depth: usize, lines: &mut Vec<String>) {
    for (k, v) in map {
        let key = key_str(k);
        match v {
            Value::Object(m) if m.is_empty() => {
                lines.push(format!("{}{}: {{}}", indent(depth), key))
            }
            Value::Object(m) => {
                lines.push(format!("{}{}:", indent(depth), key));
                object_lines(m, depth + 1, lines);
            }
            Value::Array(arr) => array_lines(Some(&key), arr, depth, lines),
            v => lines.push(format!("{}{}: {}", indent(depth), key, scalar(v))),
        }
    }
}

fn array_lines(key: Option<&str>, arr: &[Value], depth: usize, lines: &mut Vec<String>) {
    let head = |suffix: &str| match key {
        Some(k) => format!("{}{}{}", indent(depth), k, suffix),
        None => format!("{}{}", indent(depth), suffix),
    };
    if arr.is_empty() {
        lines.push(head("[0]:"));
        return;
    }
    if arr.iter().all(is_scalar) {
        let row = arr.iter().map(scalar).collect::<Vec<_>>().join(",");
        lines.push(format!("{} {}", head(&format!("[{}]:", arr.len())), row));
        return;
    }
    if let Some(fields) = tabular_fields(arr) {
        let header = fields
            .iter()
            .map(|f| key_str(f))
            .collect::<Vec<_>>()
            .join(",");
        lines.push(head(&format!("[{}]{{{}}}:", arr.len(), header)));
        for item in arr {
            let obj = item.as_object().expect("tabular item is object");
            let row = fields
                .iter()
                .map(|f| scalar(&obj[f]))
                .collect::<Vec<_>>()
                .join(",");
            lines.push(format!("{}{}", indent(depth + 1), row));
        }
        return;
    }
    lines.push(head(&format!("[{}]:", arr.len())));
    for item in arr {
        let mut item_lines: Vec<String> = Vec::new();
        match item {
            Value::Object(m) => object_lines(m, 0, &mut item_lines),
            Value::Array(a) => array_lines(None, a, 0, &mut item_lines),
            v => item_lines.push(scalar(v)),
        }
        for (i, l) in item_lines.iter().enumerate() {
            let bullet = if i == 0 { "- " } else { "  " };
            lines.push(format!("{}{}{}", indent(depth + 1), bullet, l));
        }
    }
}

fn is_scalar(v: &Value) -> bool {
    !matches!(v, Value::Object(_) | Value::Array(_))
}

/// Same keys in same order, all values scalar → tabular form.
fn tabular_fields(arr: &[Value]) -> Option<Vec<String>> {
    let first = arr.first()?.as_object()?;
    if first.is_empty() {
        return None;
    }
    let fields: Vec<String> = first.keys().cloned().collect();
    for item in arr {
        let obj = item.as_object()?;
        if obj.len() != fields.len() {
            return None;
        }
        for f in &fields {
            if !obj.get(f).map(is_scalar).unwrap_or(false) {
                return None;
            }
        }
    }
    Some(fields)
}

fn key_str(k: &str) -> String {
    let plain = !k.is_empty()
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if plain {
        k.to_string()
    } else {
        quote(k)
    }
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

    #[test]
    fn flat_object() {
        let v = json!({"a": 1, "b": "hi", "c": true, "d": null});
        assert_eq!(encode(&v), "a: 1\nb: hi\nc: true\nd: null");
    }

    #[test]
    fn nested_objects_indent_two_spaces() {
        let v = json!({"outer": {"inner": {"k": "v"}}, "next": 1});
        assert_eq!(encode(&v), "outer:\n  inner:\n    k: v\nnext: 1");
    }

    #[test]
    fn empty_object_value() {
        assert_eq!(encode(&json!({"e": {}})), "e: {}");
    }

    #[test]
    fn keys_with_special_chars_are_quoted() {
        assert_eq!(encode(&json!({"a key": 1})), "\"a key\": 1");
    }

    #[test]
    fn primitive_array_inline() {
        assert_eq!(encode(&json!({"tags": ["a", "b", "c"]})), "tags[3]: a,b,c");
    }

    #[test]
    fn empty_array() {
        assert_eq!(encode(&json!({"xs": []})), "xs[0]:");
    }

    #[test]
    fn uniform_object_array_is_tabular() {
        let v = json!({"users": [{"id": 1, "name": "Alice"}, {"id": 2, "name": "Bob"}]});
        assert_eq!(encode(&v), "users[2]{id,name}:\n  1,Alice\n  2,Bob");
    }

    #[test]
    fn mixed_array_is_list() {
        let v = json!({"items": [1, {"a": 2}, [3]]});
        assert_eq!(encode(&v), "items[3]:\n  - 1\n  - a: 2\n  - [1]: 3");
    }

    #[test]
    fn root_array() {
        let v = json!([{"id": 1}, {"id": 2}]);
        assert_eq!(encode(&v), "[2]{id}:\n  1\n  2");
    }
}
