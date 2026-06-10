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
