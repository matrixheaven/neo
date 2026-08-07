//! materialize behavior (moved from `themes.rs`).

use super::super::*;
use super::*;

#[test]
fn materialize_complete_theme_round_trips_semantic_tokens() {
    let theme = TuiTheme {
        brand: Color::Rgb(0x12, 0x34, 0x56),
        status_error: Color::Red,
        text_muted: Color::Reset,
        ..Default::default()
    };
    let json = materialize_complete_theme("Round Trip", &theme).expect("materialize");
    let parsed: ThemeFile = serde_json::from_str(&json).expect("parse back");
    let mut round = TuiTheme::default();
    apply_colors(&mut round, &parsed.colors, Path::new("memory")).expect("apply");
    assert_eq!(round.brand, theme.brand);
    assert_eq!(round.status_error, theme.status_error);
    assert_eq!(round.text_muted, theme.text_muted);
    assert_eq!(parsed.name.as_deref(), Some("Round Trip"));
}

#[test]
fn materialize_theme_with_overrides_merges_base_and_overrides() {
    let id = ThemeId::new("draft.json").unwrap();
    let base = TuiTheme {
        brand: Color::Rgb(0xaa, 0xbb, 0xcc),
        status_ok: Color::Green,
        ..Default::default()
    };
    let overrides = ThemeOverrides {
        status_ok: Some("#010203".to_owned()),
        ..ThemeOverrides::default()
    };
    let json = materialize_theme_with_overrides(&id, &base, &overrides).expect("materialize");
    let parsed: ThemeFile = serde_json::from_str(&json).expect("parse");
    let mut merged = TuiTheme::default();
    apply_colors(&mut merged, &parsed.colors, Path::new("memory")).expect("apply");
    assert_eq!(merged.status_ok, Color::Rgb(1, 2, 3), "override wins");
    assert_eq!(merged.brand, base.brand, "base fills unset tokens");
}
