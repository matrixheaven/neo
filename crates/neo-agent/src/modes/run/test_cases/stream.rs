//! stream behavior (moved from `mod.rs`).

use super::*;
use std::sync::Arc;

use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, CompactionSettings, Content,
    StopReason as AgentStopReason,
    session::{JsonlSessionReader, JsonlSessionWriter},
};
use neo_ai::{AiStreamEvent, ChatMessage, StopReason, providers::fake::FakeModelClient};
use tokio_util::sync::CancellationToken;

use super::super::run_prompt_with_runtime;

#[tokio::test]
async fn run_prompt_with_runtime_appends_continuation_to_existing_session_context() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session_dir = temp
        .path()
        .join("session_00000000-0000-4000-8000-000000000501");
    let session_path = neo_agent_core::session::main_agent_wire_path(&session_dir);
    tokio::fs::create_dir_all(session_path.parent().expect("wire parent"))
        .await
        .expect("create wire dir");
    let mut seed = JsonlSessionWriter::create(&session_path)
        .await
        .expect("create session");
    seed.append_event(&AgentEvent::MessageAppended {
        message: AgentMessage::user_text("hello"),
    })
    .await
    .expect("append user");
    seed.append_event(&AgentEvent::MessageAppended {
        message: AgentMessage::assistant(
            [Content::text("hi back")],
            Vec::new(),
            AgentStopReason::EndTurn,
        ),
    })
    .await
    .expect("append assistant");
    seed.append_event(&AgentEvent::TurnFinished {
        turn: 1,
        stop_reason: AgentStopReason::EndTurn,
    })
    .await
    .expect("append turn finish");
    seed.flush().await.expect("flush seed");
    drop(seed);

    let context = JsonlSessionReader::replay_context(&session_path)
        .await
        .expect("replay context");
    let fake = FakeModelClient::new(vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "msg-2".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "continued answer".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]);
    let runtime = super::super::AgentRuntime::new(
        AgentConfig::for_model(fake_model()),
        Arc::new(fake.clone()),
    );
    let mut writer = JsonlSessionWriter::open_append(&session_path)
        .await
        .expect("append session");

    let turn = run_prompt_with_runtime("continue".to_owned(), context, &mut writer, runtime)
        .await
        .expect("run continuation");

    assert_eq!(turn.assistant_text, "continued answer");
    let requests = fake.requests();
    assert_eq!(requests.len(), 1);
    let contents = requests[0]
        .messages
        .iter()
        .filter(|message| !matches!(message, ChatMessage::System { .. }))
        .map(chat_message_text)
        .collect::<Vec<_>>();
    assert_eq!(contents, vec!["hello", "hi back", "continue"]);

    let messages = JsonlSessionReader::replay_messages(&session_path)
        .await
        .expect("replay appended messages");
    assert_eq!(messages.len(), 4);
    assert!(matches!(
        &messages[2],
        AgentMessage::User { content, .. } if content[0].as_text() == Some("continue")
    ));
    assert!(matches!(
        &messages[3],
        AgentMessage::Assistant { content, .. }
            if content[0].as_text() == Some("continued answer")
    ));
}

#[test]
fn streaming_event_effects_persist_user_message() {
    let user_message = AgentMessage::user_text("hello");
    let event = AgentEvent::MessageAppended {
        message: user_message,
    };

    let effect = super::super::streaming_event_effect(&event);

    assert!(effect.persist);
    assert!(effect.forward);
    assert_eq!(effect.assistant_text.as_deref(), None);
}

#[test]
fn retry_scheduled_notice_is_plain_non_tty_stderr() {
    let mut stderr = Vec::new();

    super::super::write_retry_notice(
        &AgentEvent::RetryScheduled {
            turn: 1,
            retry: 1,
            max_retries: 5,
            delay_ms: 500,
            error_code: "provider.transport_error".to_owned(),
            message: "transport error: \u{1b}[31mbody closed\u{1b}[0m\r\nretry detail".to_owned(),
        },
        &mut stderr,
    )
    .expect("write retry notice");
    super::super::write_retry_notice(
        &AgentEvent::RetryStarted {
            turn: 1,
            retry: 1,
            max_retries: 5,
        },
        &mut stderr,
    )
    .expect("ignore non-scheduled retry event");

    let stderr = String::from_utf8(stderr).expect("plain UTF-8 notice");
    assert_eq!(
        stderr,
        "Reconnecting 1/5 in 500ms: Network error: body closed retry detail\n"
    );
    assert!(!stderr.contains('\u{1b}'));
}

#[tokio::test]
async fn append_streaming_event_persists_user_message_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let session_path = dir.path().join("session.jsonl");
    let mut writer = JsonlSessionWriter::create(&session_path)
        .await
        .expect("create session writer");
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    let user_message = AgentMessage::user_text("hello");
    let event = AgentEvent::MessageAppended {
        message: user_message.clone(),
    };
    let mut assistant_text = String::new();
    let mut events = Vec::new();
    let mut persistence = super::super::SessionEventPersistence::default();

    super::super::append_streaming_event(
        &event,
        &mut writer,
        &mut assistant_text,
        &event_tx,
        &mut events,
        &mut persistence,
    )
    .await
    .expect("append streaming event");
    writer.flush().await.expect("flush writer");

    let forwarded = event_rx
        .try_recv()
        .expect("forwarded event")
        .expect("successful event");
    assert_eq!(forwarded, event);
    assert_eq!(events, vec![event]);
    assert!(assistant_text.is_empty());
    assert_eq!(
        JsonlSessionReader::replay_messages(&session_path)
            .await
            .expect("replay messages"),
        vec![user_message]
    );
}

#[test]
fn streaming_event_effects_persist_assistant_text() {
    let event = AgentEvent::MessageAppended {
        message: AgentMessage::assistant(
            [Content::text("answer")],
            Vec::new(),
            AgentStopReason::EndTurn,
        ),
    };

    let effect = super::super::streaming_event_effect(&event);

    assert!(effect.persist);
    assert!(effect.forward);
    assert_eq!(effect.assistant_text.as_deref(), Some("answer"));
}

#[test]
fn streaming_event_effects_persist_non_message_events_without_text() {
    let event = AgentEvent::TurnStarted { turn: 1 };

    let effect = super::super::streaming_event_effect(&event);

    assert!(effect.persist);
    assert!(effect.forward);
    assert_eq!(effect.assistant_text.as_deref(), None);
}

#[tokio::test]
async fn failed_streaming_compaction_flushes_error_terminal_event() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("wire.jsonl");
    let mut writer = JsonlSessionWriter::create(&path)
        .await
        .expect("session writer");
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(fake_model())
            .with_compaction(CompactionSettings::new(usize::MAX, 4)),
        Arc::new(FakeModelClient::default()),
    );
    let (event_tx, _events) = tokio::sync::mpsc::unbounded_channel();

    let error = match super::super::finish_compaction_turn_streaming(
        AgentContext::new(),
        &mut writer,
        runtime,
        Vec::new(),
        super::super::StreamingTurnIo {
            event_tx,
            session_id: "session-test".to_owned(),
            cancel_token: CancellationToken::new(),
        },
    )
    .await
    {
        Ok(_) => panic!("empty context must fail compaction"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("compaction"));

    let stored = JsonlSessionReader::read_all(&path)
        .await
        .expect("read flushed terminal event");
    let terminal = stored
        .iter()
        .filter_map(|event| match event {
            AgentEvent::RunFinished { stop_reason, .. } => Some(*stop_reason),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(terminal, [AgentStopReason::Error]);
}
