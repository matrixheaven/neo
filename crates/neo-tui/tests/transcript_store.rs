use neo_tui::ansi::strip_ansi;
use neo_tui::chrome::TuiTheme;
use neo_tui::transcript::{TranscriptEntry, TranscriptStore};

fn plain_rows(store: &TranscriptStore) -> Vec<String> {
    store
        .render_rows(80, &TuiTheme::default())
        .into_iter()
        .map(|row| strip_ansi(&row.to_ansi()).trim_end().to_owned())
        .collect()
}

#[test]
fn transcript_store_renders_entries_without_draining_them() {
    let mut store = TranscriptStore::new();

    store.push(TranscriptEntry::banner("Welcome to neo"));
    store.push(TranscriptEntry::user_message("hello"));

    let first = plain_rows(&store);
    let second = plain_rows(&store);

    assert!(first.iter().any(|row| row.contains("Welcome to neo")));
    assert!(
        first
            .iter()
            .any(|row| row.contains("✨") && row.contains("hello"))
    );
    assert_eq!(first, second);
    assert_eq!(store.entries().len(), 2);
}

#[test]
fn streaming_assistant_uses_the_same_rows_after_finish() {
    let mut store = TranscriptStore::new();

    store.push(TranscriptEntry::user_message("hello"));
    store.start_assistant();
    store.append_assistant_delta("working");
    let streaming = plain_rows(&store);

    store.finish_assistant();
    let complete = plain_rows(&store);

    assert_eq!(streaming, complete);
    assert!(
        complete
            .iter()
            .any(|row| row.contains("●") && row.contains("working"))
    );
}

#[test]
fn transcript_store_uses_explicit_entry_names_and_tool_runs() {
    let mut store = TranscriptStore::new();

    store.push(TranscriptEntry::user_message("hello"));
    store.push(TranscriptEntry::assistant_message("world"));
    store.push(TranscriptEntry::status("ready"));
    store.push_tool_run("tool-1", "Bash", Some(r#"{"command":"pwd"}"#.to_owned()));

    assert!(matches!(
        store.entries()[0],
        TranscriptEntry::UserMessage(_)
    ));
    assert!(matches!(
        store.entries()[1],
        TranscriptEntry::AssistantMessage { .. }
    ));
    assert!(matches!(store.entries()[2], TranscriptEntry::Status { .. }));
    assert!(matches!(
        store.entries()[3],
        TranscriptEntry::ToolRun { .. }
    ));
}

#[test]
fn thinking_finishes_in_place_without_creating_a_second_entry() {
    let mut store = TranscriptStore::new();

    store.start_thinking();
    store.append_thinking_delta("alpha\nbeta\ngamma");
    assert_eq!(store.entries().len(), 1);

    store.finish_thinking();
    let rows = plain_rows(&store);

    assert_eq!(store.entries().len(), 1);
    assert!(rows.iter().any(|row| row.contains("● alpha")));
    assert!(rows.iter().any(|row| row.contains("1 more lines")));
}
