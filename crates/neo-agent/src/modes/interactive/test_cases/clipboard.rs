//! Interactive clipboard behavior (moved from `tests.rs`).

use neo_agent_core::AgentEvent;
use neo_tui::{
    input::{InputEvent, KeybindingAction},
    transcript::TranscriptEntry,
};

use super::super::*;
use super::*;

#[tokio::test]
async fn event_loop_copy_action_writes_prompt_to_injected_clipboard() {
    let copied = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = std::sync::Arc::clone(&copied);
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        PickerCatalogs {
            session_items: vec![test_session_summary(
                "alpha",
                "Alpha",
                test_workspace_root(),
                "session",
            )],
            session_error: None,
            model_items: Vec::new(),
        },
        |session_id| async move {
            Ok(LoadedSessionTranscript::new(
                session_id,
                Vec::new(),
                Vec::new(),
            ))
        },
    );
    controller.set_clipboard_writer(Arc::new(move |text| {
        let recorded = Arc::clone(&recorded);
        Box::pin(async move {
            recorded.lock().expect("record clipboard text").push(text);
            Ok(())
        })
    }));

    controller.type_text("copy to system clipboard");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputCopy))
        .await
        .expect("copy action succeeds");
    wait_for_clipboard_idle(&mut controller).await;

    assert_eq!(
        copied.lock().expect("clipboard writes").as_slice(),
        ["copy to system clipboard"]
    );
    assert_eq!(
        controller.chrome().copy_buffer(),
        Some("copy to system clipboard")
    );
    let frame = controller.tui.render_terminal_frame(80, 24);
    assert!(
        frame
            .lines
            .last()
            .is_some_and(|line| neo_tui::primitive::strip_ansi(line).contains("copied")),
        "successful system clipboard write shows footer confirmation"
    );
}

#[tokio::test]
async fn event_loop_clipboard_failure_keeps_internal_copy_buffer() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.set_clipboard_writer(Arc::new(|_text| {
        Box::pin(async { Err(anyhow::anyhow!("clipboard unavailable")) })
    }));

    controller.type_text("copy fallback");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputCopy))
        .await
        .expect("clipboard failure is non-fatal");
    wait_for_clipboard_idle(&mut controller).await;

    assert_eq!(controller.chrome().copy_buffer(), Some("copy fallback"));
    assert!(transcript_entries(&controller).iter().any(|entry| {
        matches!(
            entry,
            TranscriptEntry::Status { text, .. }
                if text.contains("Clipboard copy failed")
                    && text.contains("clipboard unavailable")
        )
    }));
}

#[tokio::test]
async fn event_loop_clipboard_timeout_does_not_block_input() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.set_clipboard_writer(Arc::new(|_text| {
        Box::pin(async {
            tokio::time::sleep(Duration::from_mins(1)).await;
            Ok(())
        })
    }));

    controller.type_text("block-free copy");
    let started = Instant::now();
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputCopy))
        .await
        .expect("copy starts without waiting for helper");
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "clipboard helper must not block input handling"
    );
    // Internal buffer updates immediately even while the helper is still running.
    assert_eq!(controller.chrome().copy_buffer(), Some("block-free copy"));
    assert!(controller.pending_clipboard.is_some());

    controller
        .handle_input_event(InputEvent::Insert('!'))
        .await
        .expect("further input still works while clipboard is pending");
    assert_eq!(controller.chrome().prompt().text, "block-free copy!");
    assert!(controller.pending_clipboard.is_some());

    // Cancel the hanging helper so the test runtime can shut down promptly.
    controller.cancel_pending_clipboard();
    assert!(controller.pending_clipboard.is_none());
}

#[tokio::test]
async fn new_clipboard_copy_cancels_previous_write() {
    let first_started = Arc::new(tokio::sync::Notify::new());
    let first_started_flag = Arc::clone(&first_started);
    let second_ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let second_ran_flag = Arc::clone(&second_ran);

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls = Arc::clone(&call_count);
    controller.set_clipboard_writer(Arc::new(move |_text| {
        let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let first_started_flag = Arc::clone(&first_started_flag);
        let second_ran_flag = Arc::clone(&second_ran_flag);
        Box::pin(async move {
            if n == 0 {
                first_started_flag.notify_one();
                // Hang until cancelled by the replacement copy.
                std::future::pending::<()>().await;
                #[allow(unreachable_code)]
                Ok(())
            } else {
                second_ran_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }
        })
    }));

    controller.type_text("first");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputCopy))
        .await
        .expect("first copy starts");
    tokio::time::timeout(Duration::from_secs(1), first_started.notified())
        .await
        .expect("first clipboard write should start");
    assert!(controller.pending_clipboard.is_some());

    controller.type_text(" second");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputCopy))
        .await
        .expect("second copy cancels first");
    wait_for_clipboard_idle(&mut controller).await;

    assert!(
        second_ran.load(std::sync::atomic::Ordering::SeqCst),
        "replacement clipboard write should complete"
    );
    assert_eq!(
        call_count.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "both writes should be scheduled"
    );
    // Latest internal buffer is the second copy.
    assert_eq!(controller.chrome().copy_buffer(), Some("first second"));
    // Cancelled first write must not surface a failure status.
    assert!(!transcript_entries(&controller).iter().any(|entry| {
        matches!(
            entry,
            TranscriptEntry::Status { text, .. } if text.contains("Clipboard copy failed")
        )
    }));
}
