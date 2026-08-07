use neo_tui::primitive::{truncate_width, visible_width, wrap_width};

fn strip_ansi_escapes(text: &str) -> String {
    let mut visible = String::new();
    let mut index = 0;
    while index < text.len() {
        if text.as_bytes().get(index).copied() == Some(0x1b)
            && let Some(end) = text[index..].find('m')
        {
            index += end + 1;
            continue;
        }

        let Some(character) = text[index..].chars().next() else {
            break;
        };
        visible.push(character);
        index += character.len_utf8();
    }
    visible
}

#[test]
fn truncate_width_does_not_split_ansi_or_osc_sequences() {
    let input = "\x1b]133;A\x07\x1b[32mabcdef\x1b[0m";
    let truncated = truncate_width(input, 4, "..", false);

    assert!(truncated.starts_with("\x1b]133;A\x07\x1b[32m"));
    assert_eq!(visible_width(&truncated), 4);
    assert_eq!(truncated, "\x1b]133;A\x07\x1b[32mab..");
}

#[test]
fn truncate_width_is_display_width_safe_and_can_pad() {
    assert_eq!(truncate_width("abcdef", 4, "...", false), "a...");
    assert_eq!(truncate_width("abcdef", 4, "", false), "abcd");

    let truncated = truncate_width("ab界🙂cd", 6, "..", true);
    assert_eq!(unicode_width::UnicodeWidthStr::width(truncated.as_str()), 6);
    assert!(truncated.contains(".."));
}

#[test]
fn wrap_width_breaks_long_words_and_keeps_blank_lines() {
    let lines = wrap_width("alpha\n\nsuperwide", 4);

    assert_eq!(lines[0], "alph");
    assert_eq!(lines[1], "a");
    assert_eq!(lines[2], "");
    assert!(
        lines
            .iter()
            .all(|line| unicode_width::UnicodeWidthStr::width(line.as_str()) <= 4)
    );
}

#[test]
fn wrap_width_preserves_ansi_sequences_without_counting_them() {
    let red = "\x1b[31m";
    let reset = "\x1b[0m";
    let input = format!("{red}abcdef{reset}");
    let lines = wrap_width(&input, 3);

    assert_eq!(
        lines
            .iter()
            .map(|line| strip_ansi_escapes(line))
            .collect::<String>(),
        "abcdef"
    );
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with(red));
    assert!(lines[1].ends_with(reset));
    assert!(lines.iter().all(|line| visible_width(line) <= 3));
}

#[test]
fn wrap_width_preserves_display_width_for_wide_text() {
    let lines = wrap_width("ab界cd🙂ef", 5);

    assert_eq!(lines.concat(), "ab界cd🙂ef");
    assert!(
        lines
            .iter()
            .all(|line| unicode_width::UnicodeWidthStr::width(line.as_str()) <= 5)
    );
}

#[test]
fn wrap_width_rehydrates_active_ansi_style_on_continuation_lines() {
    let red_bold = "\x1b[31;1m";
    let reset = "\x1b[0m";
    let input = format!("{red_bold}abcdef{reset}");
    let lines = wrap_width(&input, 3);

    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines
            .iter()
            .map(|line| strip_ansi_escapes(line))
            .collect::<String>(),
        "abcdef"
    );
    assert!(lines[0].starts_with(red_bold));
    assert!(lines[1].starts_with(red_bold));
    assert!(lines[1].ends_with(reset));
    assert!(lines.iter().all(|line| visible_width(line) <= 3));
}

#[test]
fn wrap_width_rehydrates_multiple_active_ansi_styles_on_continuation_lines() {
    let red = "\x1b[31m";
    let bold = "\x1b[1m";
    let reset = "\x1b[0m";
    let input = format!("{red}{bold}abcdef{reset}");
    let lines = wrap_width(&input, 3);

    assert_eq!(lines.len(), 2);
    assert!(lines[1].starts_with(&format!("{red}{bold}")));
    assert_eq!(visible_width(&lines[1]), 3);
}

#[test]
fn wrap_width_rehydrates_sgr_sequences_that_reset_then_set_style() {
    let reset_then_red = "\x1b[0;31m";
    let reset = "\x1b[0m";
    let input = format!("{reset_then_red}abcdef{reset}");
    let lines = wrap_width(&input, 3);

    assert_eq!(lines.len(), 2);
    assert!(lines[1].starts_with(reset_then_red));
    assert_eq!(visible_width(&lines[1]), 3);
}

#[test]
fn wrap_width_stops_rehydrating_style_after_reset() {
    let red = "\x1b[31m";
    let reset = "\x1b[0m";
    let input = format!("{red}ab{reset}cdef");
    let lines = wrap_width(&input, 3);

    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines
            .iter()
            .map(|line| strip_ansi_escapes(line))
            .collect::<String>(),
        "abcdef"
    );
    assert!(!lines[1].starts_with(red));
    assert_eq!(visible_width(&lines[1]), 3);
}
