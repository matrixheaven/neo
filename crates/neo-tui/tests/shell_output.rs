use neo_tui::utils::shell_output::sanitize_shell_output;

#[test]
fn strips_common_terminal_sequences() {
    assert_eq!(sanitize_shell_output("\x1b[31mred\x1b[0m"), "red");
    assert_eq!(sanitize_shell_output("\x1b]0;title\x07hello"), "hello");
    assert_eq!(
        sanitize_shell_output("\x1b[?1049hhello\x1b[?1049l"),
        "hello"
    );
    assert_eq!(sanitize_shell_output("\x1bcreset"), "reset");
}

#[test]
fn preserves_newline_and_tab_but_strips_other_c0_controls() {
    assert_eq!(sanitize_shell_output("a\x00b\x07c\n\t"), "abc\n\t");
}
