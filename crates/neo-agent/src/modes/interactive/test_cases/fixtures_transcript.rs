//! Interactive test fixtures: scripted input, transcript rendering, and slash
//! completion scaffolding (moved from `mod.rs`).

use crossterm::event::{KeyModifiers, MouseButton};
use neo_tui::transcript::TranscriptEntry;
use std::{collections::VecDeque, fs, path::Path};

use super::super::*;
use super::fixtures_config::*;
use super::fixtures_sessions::*;

pub struct OptionalScriptedEvents {
    pub events: VecDeque<Option<InputEvent>>,
}

impl TerminalEvents for OptionalScriptedEvents {
    fn next_input_event(&mut self) -> Result<InputEvent> {
        self.poll_input_event(Duration::from_millis(0))?
            .ok_or_else(|| anyhow::anyhow!("expected scripted input"))
    }

    fn poll_input_event(&mut self, _timeout: Duration) -> Result<Option<InputEvent>> {
        Ok(self
            .events
            .pop_front()
            .unwrap_or(Some(InputEvent::Interrupt)))
    }
}

pub fn transcript_entries(controller: &InteractiveController) -> &[TranscriptEntry] {
    controller.transcript().transcript().entries()
}

pub async fn wait_for_file_completion(controller: &mut InteractiveController) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if controller.poll_pending_file_completion().await {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("file completion did not finish");
}

pub async fn wait_for_clipboard_idle(controller: &mut InteractiveController) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while controller.pending_clipboard.is_some() {
        assert!(Instant::now() < deadline, "clipboard helper did not finish");
        let _ = controller.poll_pending_clipboard().await;
        tokio::task::yield_now().await;
    }
}

pub fn transcript_has_status(controller: &InteractiveController, expected: &str) -> bool {
    transcript_entries(controller).iter().any(
        |entry| matches!(entry, TranscriptEntry::Status { text, .. } if text.contains(expected)),
    )
}

pub fn transcript_view_locked(controller: &InteractiveController) -> bool {
    !controller.transcript().document().view().following_tail
}

/// Replay the active session's JSONL to recover `AgentMessage` values for
/// assertions in tests that use a real session-backed driver.
pub async fn replay_session_messages(controller: &InteractiveController) -> Vec<AgentMessage> {
    let config = controller.local_config.as_ref().expect("config");
    let session_id = controller.active_session_id.as_ref().expect("session id");
    let path = crate::modes::sessions::session_path(session_id, config).expect("session path");
    neo_agent_core::session::JsonlSessionReader::replay_context(&path)
        .await
        .map(|context| context.messages().to_vec())
        .unwrap_or_default()
}

pub fn render_tui_snapshot(tui: &neo_tui::NeoTui) -> String {
    let mut transcript = tui.transcript().clone();
    render_transcript_snapshot(tui.chrome(), &mut transcript, 80, 24)
}

pub fn slash_test_catalog() -> CompletionCatalog {
    CompletionCatalog {
        slash_prompts: vec![PickerItem::new(
            "/review",
            "/review",
            Some("Review project changes"),
        )],
        prompt_packages: vec![PickerItem::new(
            "/review-package",
            "/review-package",
            Some("Packaged review prompt"),
        )],
        session_commands: vec![
            PickerItem::new("/resume", "/resume", Some("Resume a local session")),
            PickerItem::new("/new", "/new", Some("Start a fresh local session")),
            PickerItem::new("/clear", "/clear", Some("Alias for /new")),
            PickerItem::new("/fork", "/fork", Some("Fork the current session")),
            PickerItem::new("/help", "/help", Some("Show help information")),
            PickerItem::new("/model", "/model", Some("Switch active model")),
            PickerItem::new("/provider", "/provider", Some("View configured providers")),
            PickerItem::new("/mcp", "/mcp", Some("View and manage MCP servers")),
            PickerItem::new("/tasks", "/tasks", Some("View active background tasks")),
            PickerItem::new("/plan", "/plan", Some("Toggle plan mode")),
            PickerItem::new(
                "/compact",
                "/compact",
                Some("Request manual context compaction"),
            ),
            PickerItem::new(
                "/permissions",
                "/permissions",
                Some("select permission mode"),
            ),
            PickerItem::new("/ask", "/ask", Some("ask permission mode")),
            PickerItem::new("/auto", "/auto", Some("auto permission mode")),
            PickerItem::new("/yolo", "/yolo", Some("yolo permission mode")),
            PickerItem::new("/btw", "/btw", Some("Open a temporary side-question panel")),
            PickerItem::new(
                "/skill:code-simplifier",
                "/skill:code-simplifier",
                Some("Simplify and refine code"),
            ),
        ],
        theme_items: Vec::new(),
    }
}

pub fn slash_values_for(prefix: &str, catalog: &CompletionCatalog) -> Vec<String> {
    completion_source_candidates(&test_workspace_root(), prefix, catalog)
        .expect("slash completions")
        .into_iter()
        .map(|candidate| candidate.value)
        .collect()
}

pub fn wheel_event(kind: MouseKind) -> InputEvent {
    InputEvent::Mouse(MouseEvent {
        kind,
        button: MouseButton::Left,
        column: 10,
        row: 3,
        modifiers: KeyModifiers::NONE,
    })
}

pub async fn capture_configured_interactive_turn_reasoning(
    reasoning: neo_ai::ReasoningSelection,
) -> neo_ai::ReasoningSelection {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    config.runtime.reasoning = reasoning;
    let captured = std::sync::Arc::new(std::sync::Mutex::new(None));
    let captured_request = std::sync::Arc::clone(&captured);
    let mut controller = controller_for_config(&config);
    controller.run_turn = Arc::new(move |request, _channels| {
        let captured_request = std::sync::Arc::clone(&captured_request);
        Box::pin(async move {
            *captured_request.lock().expect("capture request") = Some(request);
            Ok(TurnOutcome::default())
        })
    });

    controller.type_text("hello");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    captured
        .lock()
        .expect("captured request")
        .take()
        .expect("turn request captured")
        .reasoning
}

pub fn write_test_theme(project_dir: &Path, id: &str, name: &str, color: &str) {
    let path = project_dir.join(".neo/themes").join(id);
    fs::create_dir_all(path.parent().expect("theme parent")).expect("create theme dirs");
    fs::write(
        &path,
        format!(r#"{{"name": "{name}", "colors": {{"brand": "{color}"}}}}"#),
    )
    .expect("write theme");
}

pub fn theme_manager_overlay_text(controller: &InteractiveController) -> String {
    controller
        .chrome()
        .render_focused_full_screen_overlay(80, 24)
        .unwrap_or_default()
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn theme_manager_selected_id(controller: &InteractiveController) -> Option<String> {
    controller
        .chrome()
        .theme_manager_state()
        .and_then(|state| state.selected_id().map(ToOwned::to_owned))
}
