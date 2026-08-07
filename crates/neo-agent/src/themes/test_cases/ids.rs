//! ids behavior (moved from `themes.rs`).

use super::*;
use tempfile::TempDir;

#[test]
fn theme_id_accepts_nested_and_cjk_ids() {
    let id = ThemeId::new("组/主题.json").expect("cjk nested id");
    assert_eq!(id.as_str(), "组/主题.json");
    assert!(
        ThemeId::new("very-long-name-that-exceeds-any-reasonable-limit-and-still-works-fine.json")
            .is_ok()
    );
}

#[test]
fn theme_id_rejects_traversal_absolute_and_empty_components() {
    for raw in [
        "/abs/theme.json",
        "C:\\abs\\theme.json",
        "../theme.json",
        "a/../../b/theme.json",
        "a//b/theme.json",
        "./theme.json",
        "a/./b/theme.json",
        "",
        "a/",
    ] {
        assert!(ThemeId::new(raw).is_err(), "accepted {raw:?}");
    }
}

#[test]
fn theme_id_rejects_control_characters_and_reserved_names() {
    assert!(ThemeId::new("bad\u{1}theme.json").is_err());
    assert!(ThemeId::new("CON.json").is_err());
    assert!(ThemeId::new("con.json").is_err());
    assert!(ThemeId::new("aux/theme.json").is_err());
    assert!(ThemeId::new("nul").is_err());
    assert!(ThemeId::new("lpt1/theme.json").is_err());
    assert!(ThemeId::new("com9.json").is_err());
    assert!(ThemeId::new("ok.json").is_ok());
}

#[test]
fn theme_id_normalizes_backslash_to_forward_slash() {
    let id = ThemeId::new("nested\\theme.json").expect("backslash id");
    assert_eq!(id.as_str(), "nested/theme.json");
}

#[test]
fn invalid_entry_fallback_id_never_collides_with_real_sibling() {
    let temp = TempDir::new().expect("tempdir");
    let repo = repository_in(&temp);
    std::fs::create_dir_all(repo.root()).expect("create repo root");
    // A real theme whose id equals the naive sanitized form of the
    // malformed entry's path below.
    write_theme(&repo, "a_b.json", "Real Sibling", "blue");
    // A path whose component cannot be a valid ThemeId (control character),
    // forcing the invalid-entry fallback id derivation.
    let malformed = repo.root().join("a").join("b\u{1}.json");
    std::fs::create_dir_all(malformed.parent().expect("parent")).expect("create dirs");
    std::fs::write(&malformed, "{ not json").expect("write malformed theme");

    let catalog = repo.catalog().expect("catalog");
    let real = catalog
        .by_id(&ThemeId::new("a_b.json").unwrap())
        .expect("real sibling keeps its exact id");
    assert!(real.is_valid(), "real sibling must stay valid");

    let invalid_entries = catalog
        .entries
        .iter()
        .filter(|entry| !entry.is_valid())
        .collect::<Vec<_>>();
    assert_eq!(invalid_entries.len(), 1, "malformed entry is listed");
    let invalid = invalid_entries[0];
    assert_ne!(
        invalid.id.as_str(),
        "a_b.json",
        "fallback id must not collide with the real sibling"
    );
    assert!(
        invalid.id.as_str().starts_with("invalid-"),
        "fallback id: {}",
        invalid.id.as_str()
    );
    assert_eq!(invalid.name, "a/b\u{1}.json", "display name keeps raw path");
    assert!(
        catalog.by_id(&invalid.id).is_some(),
        "fallback id must be resolvable by id"
    );
}
