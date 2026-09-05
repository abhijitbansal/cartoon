//! Every manifest that carries a version must agree with Cargo.toml. GitHub
//! CI is disabled by decision (runner minutes cost), so `cargo test` is the
//! gate that catches drift; scripts/check-versions.mjs does the same plus the
//! git tag at release time.
#[test]
fn plugin_manifest_version_matches_cargo() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let plugin = std::fs::read_to_string(root.join(".claude-plugin/plugin.json")).unwrap();
    let json: serde_json::Value = serde_json::from_str(&plugin).unwrap();
    assert_eq!(
        json["version"].as_str(),
        Some(env!("CARGO_PKG_VERSION")),
        "run: node scripts/check-versions.mjs --write"
    );
}

#[test]
fn site_version_marker_matches_cargo() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let html = std::fs::read_to_string(root.join("docs/index.html")).unwrap();
    let marker = format!(
        "<span id=\"version\" data-cartoon-version>{}</span>",
        env!("CARGO_PKG_VERSION")
    );
    assert!(
        html.contains(&marker),
        "docs/index.html version marker is out of sync — run: node scripts/sync-site-version.mjs"
    );
}
