use cartoon::toon::encode::{encode_with, Options};
use serde_json::Value;
use std::fs;

#[test]
fn toon_fixtures() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/toon");
    let mut checked = 0;
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let case: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let expected = case["expected"].as_str().unwrap();
        let got = cartoon::toon::encode(&case["input"]);
        assert_eq!(got, expected, "fixture {:?}", path.file_name().unwrap());
        checked += 1;
    }
    assert!(checked >= 8, "only {checked} fixtures ran");
}

fn options(case: &Value) -> Options {
    let mut opts = Options::default();
    if let Some(o) = case.get("options").and_then(Value::as_object) {
        if let Some(d) = o.get("delimiter").and_then(Value::as_str) {
            opts.delimiter = d.chars().next().expect("delimiter char");
        }
        if let Some(n) = o.get("indentSize").and_then(Value::as_u64) {
            opts.indent_size = n as usize;
        }
    }
    opts
}

/// Upstream toon-spec encode conformance fixtures (vendored, see
/// tests/fixtures/toon/spec/README). Every case must pass.
#[test]
fn toon_spec_conformance_fixtures() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/toon/spec");
    let mut checked = 0;
    let mut failures = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let file: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(file["category"], "encode", "{path:?}");
        for case in file["tests"].as_array().unwrap() {
            if case.get("shouldError").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            let expected = case["expected"].as_str().unwrap();
            let got = encode_with(&case["input"], &options(case));
            if got != expected {
                failures.push(format!(
                    "{}: {}\n  expected: {expected:?}\n  got:      {got:?}",
                    path.file_name().unwrap().to_string_lossy(),
                    case["name"]
                ));
            }
            checked += 1;
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(checked >= 150, "only {checked} spec fixtures ran");
}

/// Numbers outside f64 reach the encoder as their exact JSON text
/// (serde_json `arbitrary_precision`) and come out canonical, not rounded.
#[test]
fn numbers_beyond_f64_are_lossless() {
    let cases = [
        (
            r#"{"n": 123456789012345678901234567890}"#,
            "n: 123456789012345678901234567890",
        ),
        (r#"{"n": 1e400}"#, "n: 1e+400"),
        (r#"{"n": -0}"#, "n: 0"),
        (r#"{"n": 1.50}"#, "n: 1.5"),
        (
            r#"{"xs": [9007199254740993, 0.10000000000000000001]}"#,
            "xs[2]: 9007199254740993,0.10000000000000000001",
        ),
    ];
    for (input, expected) in cases {
        let v: Value = serde_json::from_str(input).unwrap();
        assert_eq!(cartoon::toon::encode(&v), expected, "{input}");
    }
}
