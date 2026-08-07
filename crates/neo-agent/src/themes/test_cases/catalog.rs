//! catalog behavior (moved from `themes.rs`).

use super::super::*;
use super::*;
use tempfile::TempDir;

#[test]
fn catalog_lists_invalid_entries_without_hiding_valid_siblings() {
    let temp = TempDir::new().expect("tempdir");
    let repo = repository_in(&temp);
    write_theme(&repo, "a-good.json", "Good", "blue");
    write_theme(&repo, "b-bad.json", "Bad", "blue");
    let bad_path = ThemeId::new("b-bad.json")
        .expect("id")
        .path_under(repo.root());
    std::fs::write(&bad_path, "{ not json").expect("write malformed theme");

    let catalog = repo.catalog().expect("catalog");
    let ids = catalog
        .entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["a-good.json", "b-bad.json"]);
    assert!(
        catalog
            .by_id(&ThemeId::new("a-good.json").unwrap())
            .is_some()
    );
    let bad = catalog
        .by_id(&ThemeId::new("b-bad.json").unwrap())
        .expect("invalid entry still listed");
    assert!(!bad.is_valid());
    assert!(
        bad.error
            .as_deref()
            .expect("error")
            .contains("failed to parse")
    );
    assert_eq!(catalog.valid_entries().count(), 1);
}

#[test]
fn catalog_skips_symlink_entries() {
    let temp = TempDir::new().expect("tempdir");
    let repo = repository_in(&temp);
    write_theme(&repo, "real.json", "Real", "blue");
    let outside = temp.path().join("outside.json");
    std::fs::write(&outside, r#"{"name": "Out", "colors": {}}"#).expect("write outside");
    let link = repo.root().join("link.json");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &link).expect("symlink");
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&outside, &link).expect("symlink");

    let catalog = repo.catalog().expect("catalog");
    let ids = catalog
        .entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec!["real.json"],
        "symlink target must not be catalogued"
    );
}

#[test]
fn catalog_skips_symlinked_directories() {
    let temp = TempDir::new().expect("tempdir");
    let repo = repository_in(&temp);
    write_theme(&repo, "real.json", "Real", "blue");
    let outside_dir = temp.path().join("outside-dir");
    std::fs::create_dir_all(&outside_dir).expect("create outside dir");
    std::fs::write(
        outside_dir.join("escaped.json"),
        r#"{"name": "Escaped", "colors": {}}"#,
    )
    .expect("write outside theme");
    let link = repo.root().join("sub");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside_dir, &link).expect("symlink");
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&outside_dir, &link).expect("symlink");

    let catalog = repo.catalog().expect("catalog");
    let ids = catalog
        .entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec!["real.json"],
        "a symlinked directory must never be followed into the catalog"
    );
}

#[test]
fn exact_display_name_resolution_returns_ambiguity_error() {
    let temp = TempDir::new().expect("tempdir");
    let repo = repository_in(&temp);
    write_theme(&repo, "one.json", "Same Name", "blue");
    write_theme(&repo, "two.json", "Same Name", "red");

    let catalog = repo.catalog().expect("catalog");
    assert!(catalog.by_display_name("Same Name").is_err());
    assert_eq!(
        catalog
            .by_display_name("Same Name")
            .expect_err("ambiguous")
            .to_string(),
        "theme name \"Same Name\" is ambiguous; use its id instead"
    );
    assert_eq!(
        catalog
            .by_display_name("Missing")
            .expect_err("missing")
            .to_string(),
        "no theme named \"Missing\""
    );
    let resolved = repo.resolve_ref("one.json").expect("exact id resolution");
    assert_eq!(resolved.id.as_str(), "one.json");
}

#[test]
fn resolve_themes_explicit_id_missing_uses_default_with_diagnostic() {
    let temp = TempDir::new().expect("tempdir");
    let config_path = temp.path().join("config.toml");
    let resolution = resolve_themes(&config_path, Some("missing.json")).expect("resolve explicit");
    match &resolution {
        ThemeResolution::Fallback { id, reason } => {
            assert_eq!(id.as_str(), "missing.json");
            assert!(reason.contains("no theme file exists"));
        }
        other => panic!("expected fallback, got {other:?}"),
    }
    assert_eq!(resolution.to_resolved().theme, TuiTheme::default());
    assert!(
        resolution
            .diagnostic()
            .expect("diagnostic")
            .contains("missing.json")
    );
}

#[test]
fn resolve_themes_explicit_invalid_id_never_enters_sorted_fallback() {
    let temp = TempDir::new().expect("tempdir");
    let repo = repository_in(&temp);
    write_theme(&repo, "aaa.json", "First", "blue");
    let config_path = temp.path().join("config.toml");
    let resolution = resolve_themes(&config_path, Some("../escape.json")).expect("resolve");
    match &resolution {
        ThemeResolution::Fallback { reason, .. } => {
            assert!(reason.contains("invalid theme id"));
        }
        other => panic!("expected fallback, got {other:?}"),
    }
    assert!(
        !matches!(resolution, ThemeResolution::Discovered(_)),
        "explicit invalid id must not fall back to discovery"
    );
    assert_eq!(resolution.to_resolved().theme, TuiTheme::default());
}

#[test]
fn resolve_themes_discovery_is_sorted_first_and_bounded_to_absent_field() {
    let temp = TempDir::new().expect("tempdir");
    let repo = repository_in(&temp);
    write_theme(&repo, "zz.json", "Zed", "blue");
    write_theme(&repo, "aa.json", "Alpha", "red");
    let config_path = temp.path().join("config.toml");
    let resolution = resolve_themes(&config_path, None).expect("resolve");
    match resolution {
        ThemeResolution::Discovered(entry) => {
            assert_eq!(entry.id.as_str(), "aa.json");
            assert_eq!(entry.name, "Alpha");
        }
        other => panic!("expected discovered, got {other:?}"),
    }
}
