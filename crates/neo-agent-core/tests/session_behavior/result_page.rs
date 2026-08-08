use neo_agent_core::session::{JsonlSessionReader, JsonlSessionWriter};
use neo_agent_core::{AgentEvent, AgentMessage, Content, StopReason};

#[tokio::test]
async fn latest_turn_assistant_text_page_is_bounded_and_utf8_safe() {
    let workspace = tempfile::tempdir().expect("workspace");
    let path = workspace.path().join("session.jsonl");
    let mut writer = JsonlSessionWriter::create(&path)
        .await
        .expect("session writer");
    for message in [
        AgentMessage::user_text("old request"),
        AgentMessage::assistant(vec![Content::text("old answer")], [], StopReason::EndTurn),
        AgentMessage::user_text("current request"),
        AgentMessage::assistant(
            vec![Content::text("ab\u{4e16}\u{754c}c")],
            [],
            StopReason::EndTurn,
        ),
    ] {
        writer
            .append(&AgentEvent::MessageAppended { message })
            .await
            .expect("append message");
    }
    writer.flush().await.expect("flush session");

    let first = JsonlSessionReader::latest_turn_assistant_text_page(&path, 0, 5)
        .await
        .expect("read first page")
        .expect("assistant page");
    assert_eq!(first.text, "ab\u{4e16}");
    assert_eq!(first.total_chars, 5);
    assert_eq!(first.next_offset, Some(5));

    let second = JsonlSessionReader::latest_turn_assistant_text_page(
        &path,
        first.next_offset.expect("next offset"),
        5,
    )
    .await
    .expect("read second page")
    .expect("assistant page");
    assert_eq!(second.text, "\u{754c}c");
    assert_eq!(second.next_offset, None);
}
