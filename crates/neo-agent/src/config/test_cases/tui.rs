//! tui behavior (moved from `mod.rs`).

use crate::config::TuiConfig;

#[test]
fn tui_config_parses_notification_fields() {
    use neo_tui::notify::NotificationMode;

    let toml = r#"
            completion_notification = "all"
            question_notification = "bell"
        "#;
    let tui: TuiConfig = toml::from_str(toml).unwrap();
    assert_eq!(tui.completion_notification, NotificationMode::All);
    assert_eq!(tui.question_notification, NotificationMode::Bell);
}

#[test]
fn tui_config_rejects_removed_sixel_protocol() {
    let result = toml::from_str::<TuiConfig>("image_protocol = \"sixel\"");

    assert!(result.is_err());
}

#[test]
fn tui_config_defaults_notification_fields() {
    use neo_tui::notify::NotificationMode;

    let tui = TuiConfig::default();
    assert_eq!(tui.completion_notification, NotificationMode::Bell);
    assert_eq!(tui.question_notification, NotificationMode::None);
}
