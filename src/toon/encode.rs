//! TOON encoder targeting toon-spec 4.1 (https://github.com/toon-format/spec).
//! The upstream encode conformance fixtures are vendored under
//! `tests/fixtures/toon/spec/` and run by `tests/toon_fixtures.rs`.
//!
//! Numeric domain: unbounded and lossless. serde_json is built with
//! `arbitrary_precision`, so a parsed number keeps its exact JSON text and
//! the encoder canonicalizes that text with decimal string arithmetic
//! (never through f64): big integers keep every digit and `1e400` stays
//! `1e+400` instead of overflowing. See `canonical_number`.
use serde_json::{Map, Value};

/// Encoder options (spec §13). `delimiter` is the document delimiter:
/// `,` (default), `\t` or `|`.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub delimiter: char,
    pub indent_size: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            delimiter: ',',
            indent_size: 2,
        }
    }
}

/// Encode a JSON value as TOON. Returns lines joined by '\n', no trailing newline.
pub fn encode(value: &Value) -> String {
    encode_with(value, &Options::default())
}

/// Encode with explicit options (delimiter, indent size).
pub fn encode_with(value: &Value, opts: &Options) -> String {
    let mut enc = Encoder {
        opts: *opts,
        lines: Vec::new(),
    };
    match value {
        Value::Object(map) => {
            if let Some(fields) = keyed_fields(map) {
                enc.keyed_lines(None, map, &fields, 0);
            } else {
                enc.object_lines(map, 0);
            }
        }
        Value::Array(arr) if arr.is_empty() => enc.lines.push("[]".into()),
        Value::Array(arr) => enc.array_lines(None, arr, 0, true),
        v => {
            let s = scalar(v, opts.delimiter);
            enc.lines.push(s);
        }
    }
    enc.lines.join("\n")
}

/// One column of a tabular header: a primitive leaf, or a nested field
/// group for a nested-uniform object column (spec §9.3).
enum Field {
    Leaf(String),
    Group(String, Vec<Field>),
}

struct Encoder {
    opts: Options,
    lines: Vec<String>,
}

impl Encoder {
    fn indent(&self, depth: usize) -> String {
        " ".repeat(depth * self.opts.indent_size)
    }

    /// `[N<delim?>]` (or `[N:<delim?>]` for a keyed header).
    fn bracket(&self, n: usize, keyed: bool) -> String {
        let colon = if keyed { ":" } else { "" };
        match self.opts.delimiter {
            ',' => format!("[{n}{colon}]"),
            d => format!("[{n}{colon}{d}]"),
        }
    }

    fn fields_seg(&self, fields: &[Field]) -> String {
        let d = self.opts.delimiter.to_string();
        let inner = fields
            .iter()
            .map(|f| match f {
                Field::Leaf(n) => key_str(n),
                Field::Group(n, sub) => format!("{}{}", key_str(n), self.fields_seg(sub)),
            })
            .collect::<Vec<_>>()
            .join(&d);
        format!("{{{inner}}}")
    }

    fn row(&self, obj: &Map<String, Value>, fields: &[Field]) -> String {
        let mut cells = Vec::new();
        collect_cells(obj, fields, self.opts.delimiter, &mut cells);
        cells.join(&self.opts.delimiter.to_string())
    }

    fn object_lines(&mut self, map: &Map<String, Value>, depth: usize) {
        let ind = self.indent(depth);
        for (k, v) in map {
            let key = key_str(k);
            match v {
                Value::Object(m) if m.is_empty() => self.lines.push(format!("{ind}{key}:")),
                Value::Object(m) => {
                    if let Some(fields) = keyed_fields(m) {
                        self.keyed_lines(Some(&key), m, &fields, depth);
                    } else {
                        self.lines.push(format!("{ind}{key}:"));
                        self.object_lines(m, depth + 1);
                    }
                }
                Value::Array(arr) if arr.is_empty() => self.lines.push(format!("{ind}{key}: []")),
                Value::Array(arr) => self.array_lines(Some(&key), arr, depth, true),
                v => self
                    .lines
                    .push(format!("{ind}{key}: {}", scalar(v, self.opts.delimiter))),
            }
        }
    }

    /// Keyed tabular form (spec §9.5): `key[N:]{fields}:` + `entry: cells`.
    fn keyed_lines(
        &mut self,
        key: Option<&str>,
        map: &Map<String, Value>,
        fields: &[Field],
        depth: usize,
    ) {
        let head = format!(
            "{}{}{}{}:",
            self.indent(depth),
            key.unwrap_or(""),
            self.bracket(map.len(), true),
            self.fields_seg(fields)
        );
        self.lines.push(head);
        let ind = self.indent(depth + 1);
        for (k, v) in map {
            let obj = v.as_object().expect("keyed entry is object");
            let row = self.row(obj, fields);
            self.lines.push(format!("{ind}{}: {row}", key_str(k)));
        }
    }

    /// A non-empty array with its header at `depth`. `tabular_ok` is false
    /// for a keyless array in list-item position, where a fields-bearing
    /// header is not allowed (spec §9.4).
    fn array_lines(&mut self, key: Option<&str>, arr: &[Value], depth: usize, tabular_ok: bool) {
        let prefix = format!("{}{}", self.indent(depth), key.unwrap_or(""));
        let bracket = self.bracket(arr.len(), false);
        if arr.iter().all(is_scalar) {
            let d = self.opts.delimiter;
            let row = arr
                .iter()
                .map(|v| scalar(v, d))
                .collect::<Vec<_>>()
                .join(&d.to_string());
            self.lines.push(format!("{prefix}{bracket}: {row}"));
            return;
        }
        if tabular_ok {
            if let Some(fields) = tabular_fields(arr.iter()) {
                self.lines
                    .push(format!("{prefix}{bracket}{}:", self.fields_seg(&fields)));
                let ind = self.indent(depth + 1);
                for item in arr {
                    let obj = item.as_object().expect("tabular item is object");
                    let row = self.row(obj, &fields);
                    self.lines.push(format!("{ind}{row}"));
                }
                return;
            }
        }
        self.lines.push(format!("{prefix}{bracket}:"));
        for item in arr {
            self.list_item(item, depth + 1);
        }
    }

    /// One `- …` list item at `depth` (spec §9.4, §10).
    fn list_item(&mut self, item: &Value, depth: usize) {
        let ind = self.indent(depth);
        match item {
            Value::Object(m) if m.is_empty() => self.lines.push(format!("{ind}-")),
            Value::Object(m) => {
                // Render the fields one level deeper, then put the first
                // field on the hyphen line: its nested content (or tabular
                // rows) already sits at depth + 2, siblings at depth + 1.
                let start = self.lines.len();
                self.object_lines(m, depth + 1);
                let first = &self.lines[start];
                let body = first[self.indent(depth + 1).len()..].to_string();
                self.lines[start] = format!("{ind}- {body}");
            }
            Value::Array(a) if a.is_empty() => {
                let b = self.bracket(0, false);
                self.lines.push(format!("{ind}- {b}:"))
            }
            Value::Array(a) => {
                let start = self.lines.len();
                self.array_lines(None, a, depth, false);
                let first = &self.lines[start];
                let body = first[ind.len()..].to_string();
                self.lines[start] = format!("{ind}- {body}");
            }
            v => self
                .lines
                .push(format!("{ind}- {}", scalar(v, self.opts.delimiter))),
        }
    }
}

fn collect_cells(obj: &Map<String, Value>, fields: &[Field], delim: char, out: &mut Vec<String>) {
    for f in fields {
        match f {
            Field::Leaf(n) => out.push(scalar(&obj[n.as_str()], delim)),
            Field::Group(n, sub) => {
                let inner = obj[n.as_str()].as_object().expect("group is object");
                collect_cells(inner, sub, delim, out);
            }
        }
    }
}

fn is_scalar(v: &Value) -> bool {
    !matches!(v, Value::Object(_) | Value::Array(_))
}

/// Tabular detection (spec §9.3): every element a non-empty object, one
/// shared key set, every column uniform-primitive or nested-uniform. Field
/// order is the first object's encounter order, applied recursively.
fn tabular_fields<'a>(mut items: impl Iterator<Item = &'a Value> + Clone) -> Option<Vec<Field>> {
    let first = items.clone().next()?.as_object()?;
    if first.is_empty() {
        return None;
    }
    let mut objs: Vec<&Map<String, Value>> = Vec::new();
    for item in items.by_ref() {
        let obj = item.as_object()?;
        if obj.len() != first.len() || !first.keys().all(|k| obj.contains_key(k)) {
            return None;
        }
        objs.push(obj);
    }
    let mut fields = Vec::new();
    for k in first.keys() {
        let col: Vec<&Value> = objs.iter().map(|o| &o[k.as_str()]).collect();
        if col.iter().all(|v| is_scalar(v)) {
            fields.push(Field::Leaf(k.clone()));
        } else {
            let sub = tabular_fields(col.into_iter())?;
            fields.push(Field::Group(k.clone(), sub));
        }
    }
    Some(fields)
}

/// Keyed tabular detection (spec §9.5): at least two entries whose values
/// pass tabular detection.
fn keyed_fields(map: &Map<String, Value>) -> Option<Vec<Field>> {
    if map.len() < 2 {
        return None;
    }
    tabular_fields(map.values())
}

/// Spec §7.3: unquoted only if `^[A-Za-z_][A-Za-z0-9_.]*$`.
fn key_str(k: &str) -> String {
    let mut chars = k.chars();
    let plain = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.'));
    if plain {
        k.to_string()
    } else {
        quote(k)
    }
}

/// Encode a primitive; `delim` is the delimiter that forces quoting here.
fn scalar(v: &Value, delim: char) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => number(n),
        Value::String(s) => {
            if needs_quotes(s, delim) {
                quote(s)
            } else {
                s.to_string()
            }
        }
        _ => unreachable!("scalar() called on container"),
    }
}

/// Canonical number form (spec §2), computed from the number's exact JSON
/// text (serde_json's `arbitrary_precision` keeps it), so nothing is
/// rounded through f64: no exponent in [1e-6, 1e21), no trailing
/// fractional zeros, `1.0` -> `1`, `-0` -> `0`. An integer literal (digits
/// only) is always plain digits, whatever its size. Other values outside
/// the range use JSON exponent form with a lowercase `e` and explicit sign.
fn number(n: &serde_json::Number) -> String {
    canonical_number(&n.to_string())
}

/// `text` is a JSON number literal (RFC 8259 §6 grammar).
fn canonical_number(text: &str) -> String {
    let (neg, body) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (mantissa, exp_text) = match body.split_once(['e', 'E']) {
        Some((m, e)) => (m, Some(e)),
        None => (body, None),
    };
    let (int_part, frac_part) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let integer_literal = exp_text.is_none() && frac_part.is_empty();
    let Some(exp) = exp_text.map_or(Some(0), |e| e.parse::<i128>().ok()) else {
        // An exponent beyond i128 is far outside the canonical range:
        // keep the literal, normalized to lowercase `e` and a signed
        // exponent (still lossless and valid JSON).
        let e = exp_text.unwrap_or_default();
        let sign = if e.starts_with(['-', '+']) { "" } else { "+" };
        return format!("{}{mantissa}e{sign}{e}", if neg { "-" } else { "" });
    };
    // value = digits × 10^scale
    let all = format!("{int_part}{frac_part}");
    let digits = all.trim_start_matches('0');
    if digits.is_empty() {
        return "0".into();
    }
    let trimmed = digits.trim_end_matches('0');
    let scale = exp - frac_part.len() as i128 + (digits.len() - trimmed.len()) as i128;
    let digits = trimmed;
    let sign = if neg { "-" } else { "" };
    // Decimal exponent of the leading digit: 10^lead <= |value| < 10^(lead+1).
    let lead = digits.len() as i128 - 1 + scale;
    if integer_literal || (-6..21).contains(&lead) {
        if scale >= 0 {
            return format!("{sign}{digits}{}", "0".repeat(scale as usize));
        }
        let point = digits.len() as i128 + scale;
        return if point > 0 {
            let (i, f) = digits.split_at(point as usize);
            format!("{sign}{i}.{f}")
        } else {
            format!("{sign}0.{}{digits}", "0".repeat((-point) as usize))
        };
    }
    let (first, rest) = digits.split_at(1);
    let dot = if rest.is_empty() { "" } else { "." };
    let esign = if lead >= 0 { "+" } else { "" };
    format!("{sign}{first}{dot}{rest}e{esign}{lead}")
}

/// Spec §4/§7.2 numeric-like: `^[+-]?[0-9]+(\.[0-9]+)?(e[+-]?[0-9]+)?$`i.
fn is_numeric_like(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    let digits = |i: &mut usize| {
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i > start
    };
    if i < b.len() && matches!(b[i], b'+' | b'-') {
        i += 1;
    }
    if !digits(&mut i) {
        return false;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        if !digits(&mut i) {
            return false;
        }
    }
    if i < b.len() && matches!(b[i], b'e' | b'E') {
        i += 1;
        if i < b.len() && matches!(b[i], b'+' | b'-') {
            i += 1;
        }
        if !digits(&mut i) {
            return false;
        }
    }
    i == b.len()
}

/// Spec §7.2 quoting rules for string values.
fn needs_quotes(s: &str, delim: char) -> bool {
    s.is_empty()
        || s.starts_with([' ', '\t'])
        || s.ends_with([' ', '\t'])
        || matches!(s, "true" | "false" | "null")
        || is_numeric_like(s)
        || s.chars().any(|c| {
            matches!(c, ':' | '"' | '\\' | '[' | ']' | '{' | '}') || (c as u32) < 0x20 || c == delim
        })
        || s.starts_with(['-', '#'])
}

/// Spec §7.1 escaping: `\\ \" \n \r \t`, other C0 controls as `\u00xx`.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
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
        // spec §8: an empty object is a bare `key:`
        assert_eq!(encode(&json!({"e": {}})), "e:");
    }

    #[test]
    fn keys_with_special_chars_are_quoted() {
        assert_eq!(encode(&json!({"a key": 1})), "\"a key\": 1");
    }

    #[test]
    fn keys_outside_the_unquoted_pattern_are_quoted() {
        assert_eq!(encode(&json!({"my-key": 1})), "\"my-key\": 1");
        assert_eq!(encode(&json!({"1abc": 1})), "\"1abc\": 1");
        assert_eq!(encode(&json!({"-x": 1})), "\"-x\": 1");
        assert_eq!(encode(&json!({"a.b_c1": 1})), "a.b_c1: 1");
    }

    #[test]
    fn primitive_array_inline() {
        assert_eq!(encode(&json!({"tags": ["a", "b", "c"]})), "tags[3]: a,b,c");
    }

    #[test]
    fn empty_array() {
        assert_eq!(encode(&json!({"xs": []})), "xs: []");
        assert_eq!(encode(&json!([])), "[]");
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

    #[test]
    fn tabular_header_quotes_field_names_when_needed() {
        let v = json!({"rows": [{"first name": "Alice"}, {"first name": "Bob"}]});
        assert_eq!(encode(&v), "rows[2]{\"first name\"}:\n  Alice\n  Bob");
    }

    #[test]
    fn carriage_return_and_tab_escapes() {
        assert_eq!(encode(&json!("a\rb")), "\"a\\rb\"");
        assert_eq!(encode(&json!("a\tb")), "\"a\\tb\"");
    }

    #[test]
    fn empty_objects_in_a_list_are_bare_hyphens() {
        assert_eq!(encode(&json!([{}, {}])), "[2]:\n  -\n  -");
    }

    #[test]
    fn control_characters_are_unicode_escaped() {
        assert_eq!(encode(&json!("a\u{1b}[31mb")), "\"a\\u001b[31mb\"");
        assert_eq!(encode(&json!("a\u{0}b")), "\"a\\u0000b\"");
    }

    #[test]
    fn brackets_and_braces_anywhere_force_quotes() {
        assert_eq!(encode(&json!("a[0]")), "\"a[0]\"");
        assert_eq!(encode(&json!("x {y} z")), "\"x {y} z\"");
    }

    #[test]
    fn numbers_are_canonical() {
        assert_eq!(encode(&json!(1e6)), "1000000");
        assert_eq!(encode(&json!(1.0)), "1");
        assert_eq!(encode(&json!(-0.0)), "0");
        assert_eq!(encode(&json!(1.5)), "1.5");
        assert_eq!(encode(&json!(1e-6)), "0.000001");
        assert_eq!(encode(&json!(1e20)), "100000000000000000000");
        assert_eq!(encode(&json!(1e21)), "1e+21");
        assert_eq!(encode(&json!(1e-7)), "1e-7");
        let v: Value = serde_json::from_str("1E6").unwrap();
        assert_eq!(encode(&v), "1000000");
    }

    fn parsed(text: &str) -> String {
        encode(&serde_json::from_str::<Value>(text).unwrap())
    }

    #[test]
    fn integers_beyond_u64_keep_every_digit() {
        assert_eq!(
            parsed(r#"{"n": 123456789012345678901234567890}"#),
            "n: 123456789012345678901234567890"
        );
        assert_eq!(parsed("18446744073709551617"), "18446744073709551617");
        assert_eq!(parsed("-18446744073709551617"), "-18446744073709551617");
        assert_eq!(encode(&json!(u64::MAX)), "18446744073709551615");
        assert_eq!(encode(&json!(i64::MIN)), "-9223372036854775808");
    }

    #[test]
    fn parsed_numbers_are_canonicalized_from_their_exact_text() {
        assert_eq!(parsed("1.50"), "1.5");
        assert_eq!(parsed("1.0"), "1");
        assert_eq!(parsed("-0"), "0");
        assert_eq!(parsed("-0.0e5"), "0");
        assert_eq!(parsed("1E6"), "1000000");
        assert_eq!(parsed("-1E+03"), "-1000");
        assert_eq!(parsed("1e-06"), "0.000001");
        assert_eq!(parsed("0.000123e2"), "0.0123");
        assert_eq!(parsed("12.5e-1"), "1.25");
        assert_eq!(parsed("1.5e20"), "150000000000000000000");
        // Beyond f64's precision: lossless rather than rounded.
        assert_eq!(
            parsed("0.1000000000000000000000001"),
            "0.1000000000000000000000001"
        );
        assert_eq!(
            parsed("3.141592653589793238462643383279"),
            "3.141592653589793238462643383279"
        );
    }

    #[test]
    fn out_of_range_non_integer_literals_use_exponent_form() {
        // Spec §2: outside [1e-6, 1e21) an encoder MAY use JSON exponent
        // form (lowercase e, explicit sign). Values f64 cannot hold stay
        // exact instead of overflowing to null.
        assert_eq!(parsed("1e400"), "1e+400");
        assert_eq!(parsed("-2.50e-400"), "-2.5e-400");
        assert_eq!(parsed("1.5e21"), "1.5e+21");
        assert_eq!(parsed("1.23e-7"), "1.23e-7");
        assert_eq!(parsed("10e20"), "1e+21");
        assert_eq!(
            parsed("1e999999999999999999999"),
            "1e+999999999999999999999"
        );
        // Exponent beyond i128: kept verbatim, sign made explicit.
        let huge = format!("2.5E{}", "9".repeat(40));
        assert_eq!(canonical_number(&huge), format!("2.5e+{}", "9".repeat(40)));
    }

    #[test]
    fn keyed_tabular_object() {
        let v = json!({"m": {"a": {"x": 1}, "b": {"x": 2}}});
        assert_eq!(encode(&v), "m[2:]{x}:\n  a: 1\n  b: 2");
    }
}
