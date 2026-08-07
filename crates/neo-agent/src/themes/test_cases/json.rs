//! json behavior (moved from `themes.rs`).

use super::super::*;
use tempfile::TempDir;

#[test]
fn theme_json_uses_role_color_keys() {
    let temp = TempDir::new().expect("tempdir");
    let path = temp.path().join("role-theme.json");
    fs::write(
        &path,
        r##"
{
  "name": "Role Theme",
  "colors": {
    "text_primary": "#010203",
    "text_muted": "#040506",
    "brand": "#070809",
    "status_ok": "#0a0b0c",
    "status_warn": "#0d0e0f",
    "status_error": "#101112",
    "status_pending": "#131415",
    "status_cancelled": "darkgray",
    "user_message": "#161718"
  }
}
"##,
    )
    .expect("write theme");

    let resolved = load_theme_file(&path).expect("load theme");

    assert_eq!(resolved.theme.text_primary, Color::Rgb(1, 2, 3));
    assert_eq!(resolved.theme.text_muted, Color::Rgb(4, 5, 6));
    assert_eq!(resolved.theme.brand, Color::Rgb(7, 8, 9));
    assert_eq!(resolved.theme.status_ok, Color::Rgb(10, 11, 12));
    assert_eq!(resolved.theme.status_warn, Color::Rgb(13, 14, 15));
    assert_eq!(resolved.theme.status_error, Color::Rgb(16, 17, 18));
    assert_eq!(resolved.theme.status_pending, Color::Rgb(19, 20, 21));
    assert_eq!(resolved.theme.status_cancelled, Color::DarkGray);
    assert_eq!(resolved.theme.user_message, Color::Rgb(22, 23, 24));
}

#[test]
fn theme_json_rejects_old_color_keys() {
    let temp = TempDir::new().expect("tempdir");
    let path = temp.path().join("old-theme.json");
    fs::write(
        &path,
        r##"
{
  "name": "Old Theme",
  "colors": {
    "accent": "#070809"
  }
}
"##,
    )
    .expect("write theme");

    let error = load_theme_file(&path).expect_err("old key should fail");
    assert!(error.to_string().contains("failed to parse theme"));
}
