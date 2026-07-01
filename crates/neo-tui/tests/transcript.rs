use neo_tui::transcript::TranscriptPane;

fn strip_ansi(text: &str) -> String {
    let mut out = String::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == 0x1b {
            index += 1;
            while index < bytes.len() {
                let byte = bytes[index];
                index += 1;
                if (0x40..=0x7e).contains(&byte) || byte == b'\x07' {
                    break;
                }
            }
            continue;
        }
        let Some(ch) = text[index..].chars().next() else {
            break;
        };
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

#[test]
fn transcript_render_frame_slices_rows_through_viewport() {
    let mut pane = TranscriptPane::new(80, 6);
    pane.set_live_chrome_height(0);
    let status_lines = (0..12)
        .map(|index| format!("status line {index:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    pane.push_status(status_lines);

    let bottom = pane
        .render_frame(80, 6)
        .expect("initial render should be dirty")
        .join("\n");
    let bottom_plain = strip_ansi(&bottom);
    assert!(!bottom_plain.contains("status line 00"));
    assert!(bottom_plain.contains("status line 11"));

    pane.scroll_transcript_up(4);
    let scrolled = pane
        .render_frame(80, 6)
        .expect("scrolling should dirty the pane")
        .join("\n");
    let scrolled_plain = strip_ansi(&scrolled);
    assert!(scrolled_plain.contains("status line 04"));
    assert!(scrolled_plain.contains("status line 07"));
    assert!(!scrolled_plain.contains("status line 11"));

    pane.push_status("status line 12");
    let grown = pane
        .render_frame(80, 6)
        .expect("new status should dirty the pane")
        .join("\n");
    let grown_plain = strip_ansi(&grown);
    assert!(grown_plain.contains("status line 04"));
    assert!(grown_plain.contains("status line 07"));
    assert!(!grown_plain.contains("status line 12"));
}
