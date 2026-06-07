use neo_agent_core::session::{JsonlSessionReader, JsonlSessionWriter};
use neo_agent_core::{AgentContext, AgentEvent, AgentMessage, StopReason};

#[tokio::test]
async fn jsonl_session_appends_reads_and_replays_events() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let mut writer = JsonlSessionWriter::create(&path)
        .await
        .expect("create session");

    writer
        .append(&AgentEvent::MessageAppended {
            message: AgentMessage::user_text("remember this"),
        })
        .await
        .expect("append user");
    writer
        .append(&AgentEvent::TurnFinished {
            turn: 1,
            stop_reason: StopReason::EndTurn,
        })
        .await
        .expect("append finish");
    writer.flush().await.expect("flush");

    let events = JsonlSessionReader::read_all(&path).await.expect("read all");
    assert_eq!(
        events,
        vec![
            AgentEvent::MessageAppended {
                message: AgentMessage::user_text("remember this"),
            },
            AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::EndTurn,
            },
        ]
    );

    let replayed = JsonlSessionReader::replay_messages(&path)
        .await
        .expect("replay");
    assert_eq!(replayed, vec![AgentMessage::user_text("remember this")]);
}

#[tokio::test]
async fn jsonl_session_replays_runtime_context_with_turns_and_terminal_state() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let mut writer = JsonlSessionWriter::create(&path)
        .await
        .expect("create session");

    for event in [
        AgentEvent::MessageAppended {
            message: AgentMessage::user_text("start"),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant([], Vec::new(), StopReason::EndTurn),
        },
        AgentEvent::TurnFinished {
            turn: 1,
            stop_reason: StopReason::EndTurn,
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::user_text("stop"),
        },
        AgentEvent::TurnFinished {
            turn: 2,
            stop_reason: StopReason::Cancelled,
        },
    ] {
        writer.append(&event).await.expect("append event");
    }
    writer.flush().await.expect("flush");

    let context = JsonlSessionReader::replay_context(&path)
        .await
        .expect("replay context");

    assert_eq!(
        context.messages(),
        &[
            AgentMessage::user_text("start"),
            AgentMessage::assistant([], Vec::new(), StopReason::EndTurn),
            AgentMessage::user_text("stop"),
        ]
    );
    assert_eq!(context.turns(), 2);
    assert!(context.is_cancelled());

    let events = JsonlSessionReader::read_all(&path).await.expect("read all");
    assert_eq!(AgentContext::from_replay(events.iter()), context);
}
