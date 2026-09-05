//! Golden corpus: real captured CLI logs with signal-retention and
//! token-floor assertions per compression level. Floors document measured
//! reality (see each manifest); run with --nocapture to see reductions.
use cartoon::ladder::{compress, CompressLevel};
use cartoon::stats::estimate_tokens;
use serde::Deserialize;

#[derive(Deserialize)]
struct Manifest {
    must_survive: Vec<String>,
    /// Lines the SAFE tier must keep verbatim but the (lossy, disclosed)
    /// aggressive tier may fold — e.g. rows of a table that differ only by
    /// numbers, which near-dup templating legitimately collapses.
    #[serde(default)]
    must_survive_safe: Vec<String>,
    min_reduction_safe: f64,
    min_reduction_aggressive: f64,
}

fn reduction(original: &str, compressed: &str) -> f64 {
    let before = estimate_tokens(original, "o200k") as f64;
    let after = estimate_tokens(compressed, "o200k") as f64;
    if before == 0.0 {
        return 0.0;
    }
    (before - after) / before
}

#[test]
fn corpus_signal_retention_and_token_floor() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut checked = 0;
    for entry in std::fs::read_dir(&root).expect("tests/corpus exists") {
        let dir = entry.unwrap().path();
        if !dir.is_dir() {
            continue;
        }
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let log = std::fs::read_to_string(dir.join("log.txt")).expect("log.txt");
        let manifest: Manifest =
            toml::from_str(&std::fs::read_to_string(dir.join("manifest.toml")).unwrap())
                .expect("manifest.toml");
        for (level, floor) in [
            (CompressLevel::Safe, manifest.min_reduction_safe),
            (CompressLevel::Aggressive, manifest.min_reduction_aggressive),
        ] {
            let out = compress(&log, level);
            for line in &manifest.must_survive {
                assert!(out.contains(line), "{name}@{level:?}: signal lost: {line}");
            }
            if level == CompressLevel::Safe {
                for line in &manifest.must_survive_safe {
                    assert!(
                        out.contains(line),
                        "{name}@Safe: non-lossy tier lost: {line}"
                    );
                }
            }
            let r = reduction(&log, &out);
            eprintln!("{name}@{level:?}: reduction {r:.3} (floor {floor:.3})");
            assert!(
                r >= floor,
                "{name}@{level:?}: reduction {r:.2} below floor {floor:.2}"
            );
        }
        checked += 1;
    }
    assert!(checked >= 2, "corpus must contain fixtures");
}
