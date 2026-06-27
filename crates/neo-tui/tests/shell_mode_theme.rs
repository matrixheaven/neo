use neo_tui::primitive::Color;
use neo_tui::shell::TuiTheme;

#[test]
fn shell_mode_color_defaults_to_cyan() {
    assert_eq!(TuiTheme::default().shell_mode, Color::Rgb(86, 182, 194));
}

#[test]
fn shell_mode_color_has_builder() {
    let theme = TuiTheme::default().with_shell_mode(Color::Rgb(1, 2, 3));
    assert_eq!(theme.shell_mode, Color::Rgb(1, 2, 3));
}
