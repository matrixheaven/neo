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
fn canonical_snapshot_retains_full_history_after_terminal_commit() {
    let mut pane = TranscriptPane::new(80, 6);
    pane.set_live_chrome_height(0);
    let status_lines = (0..12)
        .map(|index| format!("status line {index:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    pane.push_status(status_lines);

    let update = pane.render_terminal_update(80, 6);
    pane.acknowledge_history(&update.history);

    let canonical = pane.frame_ansi_lines().join("\n");
    let canonical = strip_ansi(&canonical);
    assert!(canonical.contains("status line 00"));
    assert!(canonical.contains("status line 11"));
}

#[test]
fn terminal_update_does_not_replay_committed_history() {
    let mut pane = TranscriptPane::new(80, 6);
    pane.set_live_chrome_height(0);
    pane.push_status("committed status");

    let first = pane.render_terminal_update(80, 6);
    let first_history = first
        .history
        .iter()
        .flat_map(|block| block.lines.iter())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    assert!(strip_ansi(&first_history).contains("committed status"));

    pane.acknowledge_history(&first.history);
    let second = pane.render_terminal_update(80, 6);
    assert!(second.history.is_empty());
    assert!(!strip_ansi(&second.live.join("\n")).contains("committed status"));
}
