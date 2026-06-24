use std::borrow::Borrow;
use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;
use std::path::PathBuf;

use neo_agent_core::{
    AgentEvent, AgentMessage, AgentToolCall, Content, ImageRef, PermissionOperation, ToolResult,
    skills::SkillStore,
};

use crate::ansi::{Color, Style, paint, truncate_to_width, visible_width};
use crate::chrome::{
    DevelopmentMode, GoalModeStatus, MAX_PROMPT_VISIBLE_LINES, NeoChromeState, PromptState,
    ToolStatusKind, TuiTheme,
};
use crate::components::wrap_width;
use crate::core::{Expandable, Line};
use crate::image::{ImageRenderPolicy, ImageSource, InlineImage, TerminalImageCapabilities};
use crate::terminal::{CURSOR_MARKER, CursorPos};
use crate::transcript::entry::GoalCardKind;
use crate::transcript::{
    ApprovalPromptData, InlineImageRender, ToolCallComponent, ToolCallState, ToolGroup,
    TranscriptEntry, TranscriptStore, render_tool_group,
};
use crate::widgets::box_draw::{ROUNDED, repeat_char};
use crate::widgets::{PendingInputPreview, TodoPanel, box_draw};

const DEFAULT_LIVE_CHROME_HEIGHT: usize = 4;
const GITHUB_YELLOW: Color = Color::Rgb(191, 135, 0);
const GITHUB_GREEN: Color = Color::Rgb(26, 127, 55);
const GITHUB_RED: Color = Color::Rgb(207, 34, 46);
const GITHUB_BLUE: Color = Color::Rgb(9, 105, 218);

/// Uniform 1-column left/right gutter applied to ALL chrome (body, banner,
/// prompt box, footer). Matches Neo's `CHROME_GUTTER = 1`. Applied once
/// by [`apply_gutter`] after body + chrome are merged, so nothing renders
/// flush against the screen edge.
pub const CHROME_GUTTER: usize = 1;

/// Prepend `CHROME_GUTTER` spaces to every non-empty line. Empty separator
/// lines stay empty so vertical spacing isn't shifted.
pub fn apply_gutter(lines: &mut [String]) {
    if CHROME_GUTTER == 0 {
        return;
    }
    let lead = " ".repeat(CHROME_GUTTER);
    for line in lines.iter_mut() {
        if !line.is_empty() {
            line.insert_str(0, &lead);
        }
    }
}

#[derive(Debug, Clone)]
pub struct TranscriptPane {
    width: usize,
    height: usize,
    live_chrome_height: usize,
    transcript: TranscriptStore,
    dirty: bool,
    tool_output_expanded: bool,
    streaming_tool_args: BTreeMap<String, String>,
    queued_approvals: VecDeque<ApprovalPromptData>,
    completed_tool_result_ids: Vec<String>,
    next_image_id: u64,
    activity_frame: usize,
    workspace_root: Option<PathBuf>,
    /// Cache of the last composed body frame (ANSI strings, no chrome), so
    /// tests can inspect rendered output via [`frame_ansi_lines`] without
    /// recomposing unchanged rows.
    last_frame: Vec<String>,
    /// Theme used to color the live transcript body. Mirrors [`NeoChromeState`]'s
    /// theme; kept here (rather than borrowed) so the runtime can render
    /// without holding a reference to the app. The interactive mode keeps it
    /// in sync via [`Self::set_theme`].
    theme: TuiTheme,
    skill_store: Option<SkillStore>,
}

impl TranscriptPane {
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            live_chrome_height: DEFAULT_LIVE_CHROME_HEIGHT,
            transcript: TranscriptStore::new(),
            dirty: false,
            tool_output_expanded: false,
            streaming_tool_args: BTreeMap::new(),
            queued_approvals: VecDeque::new(),
            completed_tool_result_ids: Vec::new(),
            next_image_id: 0,
            activity_frame: 0,
            workspace_root: None,
            last_frame: Vec::new(),
            theme: TuiTheme::default(),
            skill_store: None,
        }
    }

    /// Set the skill store used to enrich runtime skill events with metadata.
    pub fn set_skill_store(&mut self, store: SkillStore) {
        self.skill_store = Some(store);
    }

    /// Update the theme used to color the live transcript body. Called by the
    /// interactive mode whenever the app's theme changes (e.g. from a
    /// `~/.neo/themes/*.json` file).
    pub fn set_theme(&mut self, theme: TuiTheme) {
        if self.theme == theme {
            return;
        }
        self.theme = theme;
        self.mark_dirty();
    }

    pub fn set_workspace_root(&mut self, workspace_root: impl Into<PathBuf>) {
        let path = workspace_root.into();
        if self.workspace_root.as_deref() == Some(&path) {
            return;
        }
        self.workspace_root = Some(path);
        for entry in self.transcript.entries_mut() {
            if let TranscriptEntry::ToolRun { component } = entry {
                component.set_workspace_dir(self.workspace_root.clone().unwrap_or_default());
            }
        }
        self.mark_dirty();
    }

    #[must_use]
    pub const fn theme(&self) -> TuiTheme {
        self.theme
    }

    pub fn push_transcript(&mut self, entry: TranscriptEntry) {
        self.transcript
            .push(self.apply_expand_state_to_entry(entry));
        self.mark_dirty();
    }

    pub fn push_user_message(&mut self, content: impl Into<String>) {
        self.push_transcript(TranscriptEntry::user_message(content));
    }

    /// Push a queued (Enter while busy) or steered (Ctrl+S) message preview
    /// into the transcript. Rendered with a distinct prefix so the user sees
    /// visual feedback that their input was captured mid-turn.
    pub fn push_queued_message(&mut self, content: impl Into<String>, is_steer: bool) {
        self.push_transcript(TranscriptEntry::queued_message(content, is_steer));
    }

    /// Pop the most recent queued follow-up entry from the transcript. Used
    /// when the user presses Ctrl+S with an empty composer to promote the
    /// oldest queued follow-up to a steer. Returns the text if found.
    pub fn pop_pending_follow_up(&mut self) -> Option<String> {
        let index = self
            .transcript
            .entries()
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, entry)| match entry {
                TranscriptEntry::QueuedMessage {
                    is_steer: false, ..
                } => Some(i),
                _ => None,
            })?;
        let entry = self.transcript.remove(index)?;
        match entry {
            TranscriptEntry::QueuedMessage { text, .. } => {
                self.mark_dirty();
                Some(text)
            }
            _ => None,
        }
    }

    pub fn push_assistant_message(&mut self, content: impl Into<String>) {
        self.push_transcript(TranscriptEntry::assistant_message(content));
    }

    pub fn push_banner(&mut self, title: impl Into<String>) {
        self.push_transcript(TranscriptEntry::banner(title));
    }

    /// Push a rich welcome banner (rounded box + logo + metadata) built from
    /// the app's title/session/model/workspace info.
    pub fn push_welcome_banner(
        &mut self,
        title: &str,
        session: &str,
        model: &str,
        directory: &str,
        version: &str,
        mcp: Option<String>,
    ) {
        use crate::transcript::BannerData;
        let data = BannerData {
            title: format!("Welcome to {title}!"),
            subtitle: "Send /help for help information.".to_owned(),
            directory: directory.to_owned(),
            session: session.to_owned(),
            model: model.to_owned(),
            version: version.to_owned(),
            mcp,
        };
        self.push_transcript(TranscriptEntry::welcome_banner(data));
    }

    pub fn replay_user_message(&mut self, content: impl Into<String>) {
        self.push_user_message(content);
    }

    pub fn replay_assistant_message(&mut self, content: impl Into<String>) {
        self.push_assistant_message(content);
    }

    pub fn push_status(&mut self, content: impl Into<String>) {
        self.push_transcript(TranscriptEntry::status(content));
    }

    pub fn replay_message(&mut self, message: &AgentMessage) {
        match message {
            AgentMessage::User { content } => {
                let text = content_display_text(content);
                if !text.is_empty() {
                    self.replay_user_message(text);
                }
            }
            AgentMessage::Assistant {
                content,
                tool_calls,
                ..
            } => {
                self.replay_assistant_content(content);
                for tool_call in tool_calls {
                    self.apply_agent_event(&AgentEvent::ToolExecutionStarted {
                        turn: 0,
                        id: tool_call.id.clone(),
                        name: tool_call.name.clone(),
                        arguments: tool_call.arguments.clone(),
                    });
                }
            }
            AgentMessage::ToolResult {
                tool_call_id,
                tool_name,
                content,
                is_error,
            } => {
                if take_completed_tool_result(&mut self.completed_tool_result_ids, tool_call_id) {
                    return;
                }
                let text = content_display_text(content);
                self.apply_agent_event(&AgentEvent::ToolExecutionFinished {
                    turn: 0,
                    id: tool_call_id.clone(),
                    name: tool_name.clone(),
                    result: neo_agent_core::ToolResult {
                        content: text,
                        is_error: *is_error,
                        details: None,
                        terminate: false,
                    },
                });
            }
            AgentMessage::System { content } => {
                let text = content_display_text(content);
                if !text.is_empty() {
                    self.push_status(text);
                }
            }
        }
    }

    pub fn replay_assistant_content(&mut self, content: &[Content]) {
        let mut text = String::new();
        for part in content {
            match part {
                Content::Text { text: part_text } => {
                    text.push_str(part_text);
                }
                Content::Thinking { .. } => self.replay_thinking_content(part, &mut text),
                Content::Image { mime_type, data } => {
                    self.flush_replayed_assistant_text(&mut text);
                    self.push_image(mime_type, data);
                }
            }
        }
        if !text.is_empty() {
            self.replay_assistant_message(text);
        }
    }

    fn replay_thinking_content(&mut self, part: &Content, text: &mut String) {
        let Content::Thinking {
            text: thinking_text,
            redacted,
            signature: _,
        } = part
        else {
            return;
        };
        self.flush_replayed_assistant_text(text);
        if !thinking_text.is_empty() {
            self.push_transcript(TranscriptEntry::thinking_complete(thinking_text.clone()));
        } else if *redacted {
            self.push_transcript(TranscriptEntry::thinking_complete("[Reasoning redacted]"));
        }
    }

    fn flush_replayed_assistant_text(&mut self, text: &mut String) {
        if !text.is_empty() {
            self.replay_assistant_message(std::mem::take(text));
        }
    }

    pub fn push_image(&mut self, mime_type: &str, data: &ImageRef) {
        self.next_image_id = self.next_image_id.saturating_add(1);
        let id = format!("image-{}", self.next_image_id);
        let entry = match data {
            ImageRef::Base64(encoded) => {
                let bytes = decode_base64(encoded).unwrap_or_else(|| encoded.as_bytes().to_vec());
                let inline = InlineImage::bytes(
                    id.clone(),
                    mime_type.to_owned(),
                    bytes,
                    None::<String>,
                    ImageSource::Base64,
                );
                TranscriptEntry::image(
                    id,
                    mime_type.to_owned(),
                    inline.size_bytes(),
                    None::<String>,
                    ImageSource::Base64,
                    inline.metadata_summary(),
                    inline.into_payload_bytes(),
                )
            }
            ImageRef::Url(url) => {
                let inline = InlineImage::remote_url(
                    id.clone(),
                    mime_type.to_owned(),
                    sanitized_image_url(url),
                    None::<String>,
                );
                TranscriptEntry::image(
                    id,
                    mime_type.to_owned(),
                    None,
                    None::<String>,
                    ImageSource::RemoteUrl,
                    inline.metadata_summary(),
                    None,
                )
            }
            ImageRef::Blob(sha256) => {
                // Blobs should be resolved to base64 before rendering. If a
                // blob reference reaches the transcript, render a placeholder.
                TranscriptEntry::image(
                    id,
                    mime_type.to_owned(),
                    None,
                    Some(format!("[image blob {}]", sha256)),
                    ImageSource::Base64,
                    format!("blob:{sha256}"),
                    None,
                )
            }
        };
        self.push_transcript(entry);
    }

    #[must_use]
    pub fn inline_image_renders(
        &self,
        policy: ImageRenderPolicy,
        capabilities: TerminalImageCapabilities,
    ) -> Vec<InlineImageRender> {
        self.transcript
            .entries()
            .iter()
            .filter_map(|entry| entry.inline_image_render(policy, capabilities))
            .collect()
    }

    #[must_use]
    pub fn inline_image_sequences(
        &self,
        policy: ImageRenderPolicy,
        capabilities: TerminalImageCapabilities,
    ) -> Vec<String> {
        self.inline_image_renders(policy, capabilities)
            .into_iter()
            .map(|render| render.escape_sequence)
            .collect()
    }

    pub fn scroll_transcript_up(&mut self, rows: usize) {
        self.transcript.viewport_mut().scroll_up(rows);
    }

    pub fn scroll_transcript_down(&mut self, rows: usize) {
        self.transcript.viewport_mut().scroll_down(rows);
    }

    pub fn sync_transcript_view(&mut self, content_rows: usize, viewport_rows: usize) {
        self.transcript
            .viewport_mut()
            .sync(content_rows, viewport_rows);
    }

    pub fn select_visible_transcript_entry(&mut self) {
        self.transcript.select_visible_entry();
    }

    pub fn clear_transcript_selection(&mut self) {
        self.transcript.clear_selection();
    }

    pub fn extend_transcript_selection_up(&mut self, rows: usize) {
        self.transcript.extend_selection_up(rows);
    }

    pub fn extend_transcript_selection_down(&mut self, rows: usize) {
        self.transcript.extend_selection_down(rows);
    }

    #[must_use]
    pub fn has_transcript_selection(&self) -> bool {
        self.transcript.has_selection()
    }

    #[must_use]
    pub fn copy_selected_transcript_text(&self) -> Option<String> {
        self.transcript.copy_selection()
    }

    pub fn start_assistant_message(&mut self) {
        self.transcript.start_assistant();
        self.mark_dirty();
    }

    pub fn append_assistant_delta(&mut self, text: &str) {
        self.transcript.finish_thinking();
        self.transcript.append_assistant_delta(text);
        self.mark_dirty();
    }

    pub fn finish_assistant_message(&mut self) {
        self.transcript.finish_assistant();
        self.mark_dirty();
    }

    pub fn set_tool_output_expanded(&mut self, expanded: bool) {
        self.tool_output_expanded = expanded;
        for entry in self.transcript.entries_mut() {
            match entry {
                TranscriptEntry::ToolRun { component } => component.set_expanded(expanded),
                TranscriptEntry::ThinkingBlock {
                    expanded: thinking_expanded,
                    ..
                } => *thinking_expanded = expanded,
                _ => {}
            }
        }
        self.mark_dirty();
    }

    pub fn toggle_tool_output_expanded(&mut self) -> bool {
        if !self.transcript.entries().iter().any(|entry| {
            matches!(
                entry,
                TranscriptEntry::ToolRun { .. } | TranscriptEntry::ThinkingBlock { .. }
            )
        }) {
            return false;
        }
        self.set_tool_output_expanded(!self.tool_output_expanded);
        true
    }

    #[must_use]
    pub const fn tool_output_expanded(&self) -> bool {
        self.tool_output_expanded
    }

    pub fn set_live_chrome_height(&mut self, height: usize) {
        self.live_chrome_height = height;
        self.mark_dirty();
    }

    #[must_use]
    pub const fn live_chrome_height(&self) -> usize {
        self.live_chrome_height
    }

    pub fn apply_agent_event<E>(&mut self, event: E)
    where
        E: Borrow<AgentEvent>,
    {
        let event = event.borrow();
        if self.apply_message_event(event) {
            return;
        }
        if self.apply_thinking_event(event) {
            return;
        }
        if self.apply_tool_event(event) {
            return;
        }
        if self.apply_queue_event(event) {
            return;
        }
        if self.apply_compaction_event(event) {
            return;
        }
        self.apply_skill_goal_event(event);
    }

    fn apply_message_event(&mut self, event: &AgentEvent) -> bool {
        match event {
            AgentEvent::MessageStarted { .. } => {
                self.mark_dirty();
                true
            }
            AgentEvent::TextDelta { text, .. } => {
                self.append_assistant_delta(text);
                true
            }
            AgentEvent::MessageFinished { .. } | AgentEvent::TurnFinished { .. } => {
                self.finish_active_text_blocks();
                true
            }
            _ => false,
        }
    }

    fn apply_thinking_event(&mut self, event: &AgentEvent) -> bool {
        match event {
            AgentEvent::ThinkingStarted { .. } => {
                self.start_thinking_block();
                true
            }
            AgentEvent::ThinkingDelta { text, .. } => {
                self.append_thinking_block(text);
                true
            }
            AgentEvent::ThinkingFinished { .. } => {
                self.finish_thinking_block();
                true
            }
            _ => false,
        }
    }

    fn apply_tool_event(&mut self, event: &AgentEvent) -> bool {
        match event {
            AgentEvent::ToolCallStarted { id, name, .. } => {
                self.start_tool_call(id, name.clone());
                true
            }
            AgentEvent::ToolCallArgumentsDelta {
                id, json_fragment, ..
            } => {
                self.append_tool_call_arguments(id, json_fragment);
                true
            }
            AgentEvent::ToolCallFinished { tool_call, .. } => {
                self.finish_tool_call(tool_call.clone());
                true
            }
            AgentEvent::ToolExecutionStarted {
                id,
                name,
                arguments,
                ..
            } => {
                self.start_tool_execution(id, name.clone(), arguments);
                true
            }
            AgentEvent::ApprovalRequested {
                id,
                operation,
                subject,
                arguments,
                session_scope,
                prefix_rule,
                ..
            } => {
                let mut session_label = session_scope
                    .as_ref()
                    .filter(|scope| !scope.is_empty())
                    .map(|scope| scope.label.clone());
                // Tool and shell approvals always offer a session-approval option,
                // even when no explicit session scope was derived. Use the default
                // label so the modal keeps its four-option layout.
                if session_label.is_none()
                    && matches!(
                        operation,
                        PermissionOperation::Tool | PermissionOperation::Shell
                    )
                {
                    session_label = Some("Approve for this session".to_owned());
                }
                let prefix_label = prefix_rule
                    .as_ref()
                    .map(|rule| format!("Approve commands starting with {}", rule.label));
                self.request_approval(
                    id.clone(),
                    *operation,
                    subject,
                    arguments,
                    session_label,
                    prefix_label,
                );
                true
            }
            AgentEvent::ToolExecutionUpdate {
                id,
                name,
                partial_result,
                ..
            } => {
                self.update_tool_execution(id, name.clone(), partial_result.clone());
                true
            }
            AgentEvent::ToolExecutionFinished {
                id, name, result, ..
            } => {
                self.finish_tool_execution(id.clone(), name.clone(), result.clone());
                true
            }
            AgentEvent::ShellCommandStarted {
                id, command, cwd, ..
            } => {
                self.start_shell_command(id, command, cwd);
                true
            }
            AgentEvent::ShellCommandFinished {
                id,
                exit_code,
                stdout,
                stderr,
                truncated,
                ..
            } => {
                self.finish_shell_command(id.clone(), *exit_code, stdout, stderr, *truncated);
                true
            }
            _ => false,
        }
    }

    fn apply_queue_event(&mut self, event: &AgentEvent) -> bool {
        match event {
            // Queue events are now rendered in the dedicated Pending Input
            // Preview panel above the composer, not as transcript status lines.
            AgentEvent::SteeringQueued { .. }
            | AgentEvent::FollowUpQueued { .. }
            | AgentEvent::QueueDrained { .. } => true,
            AgentEvent::Error { message, .. } => {
                self.push_status(format!("Error: {message}"));
                true
            }
            AgentEvent::RunFinished { turn, stop_reason } => {
                if let Some(notice) = run_finished_notice(*turn, *stop_reason) {
                    self.push_status(notice);
                }
                true
            }
            _ => false,
        }
    }

    fn apply_compaction_event(&mut self, event: &AgentEvent) -> bool {
        match event {
            AgentEvent::CompactionStarted {
                tokens_before,
                message_count,
                ..
            } => {
                self.upsert_compaction(
                    Some(neo_agent_core::CompactionPhase::Estimating),
                    0,
                    *message_count,
                    *tokens_before,
                    0,
                );
                true
            }
            AgentEvent::CompactionProgress { phase, percent } => {
                self.update_compaction_progress(*phase, (*percent).min(99));
                true
            }
            AgentEvent::CompactionApplied { summary } => {
                self.upsert_compaction(
                    Some(neo_agent_core::CompactionPhase::Applying),
                    100,
                    summary.first_kept_message_index,
                    summary.tokens_before,
                    summary.tokens_after,
                );
                true
            }
            _ => false,
        }
    }

    fn apply_skill_goal_event(&mut self, event: &AgentEvent) {
        if let AgentEvent::SkillActivated { name, .. } = event {
            self.push_skill_activation(name.clone());
            return;
        }
        self.apply_goal_event(event);
    }

    fn apply_goal_event(&mut self, event: &AgentEvent) {
        if self.apply_goal_state_event(event) {
            return;
        }
        self.apply_goal_terminal_event(event);
    }

    fn apply_goal_state_event(&mut self, event: &AgentEvent) -> bool {
        match event {
            AgentEvent::GoalStarted { objective, .. } => {
                self.push_goal_state_card(GoalCardKind::Started, objective);
            }
            AgentEvent::GoalPaused { objective, .. } => {
                self.push_goal_state_card(GoalCardKind::Paused, objective);
            }
            AgentEvent::GoalResumed { objective, .. } => {
                self.push_goal_state_card(GoalCardKind::Resumed, objective);
            }
            _ => return false,
        }
        true
    }

    fn apply_goal_terminal_event(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::GoalBlocked { .. } => self.push_goal_blocked_card(event),
            AgentEvent::GoalFinished { .. } => self.push_goal_finished_card(event),
            _ => {}
        }
    }

    fn push_goal_blocked_card(&mut self, event: &AgentEvent) {
        let AgentEvent::GoalBlocked {
            objective, reason, ..
        } = event
        else {
            return;
        };
        self.push_goal_card(
            GoalCardKind::Blocked,
            objective.clone(),
            Some(reason.clone()),
            None,
        );
    }

    fn push_goal_finished_card(&mut self, event: &AgentEvent) {
        let AgentEvent::GoalFinished {
            objective,
            outcome,
            turn,
            ..
        } = event
        else {
            return;
        };
        self.push_goal_card(
            GoalCardKind::Finished,
            objective.clone(),
            Some(outcome.clone()),
            Some(*turn),
        );
    }

    fn push_goal_state_card(&mut self, kind: GoalCardKind, objective: &str) {
        self.push_goal_card(kind, objective.to_owned(), None, None);
    }

    fn start_thinking_block(&mut self) {
        self.finish_assistant_message();
        self.transcript.start_thinking();
        self.apply_expand_state_to_active_thinking();
        self.mark_dirty();
    }

    fn append_thinking_block(&mut self, text: &str) {
        self.transcript.append_thinking_delta(text);
        self.mark_dirty();
    }

    fn finish_thinking_block(&mut self) {
        self.transcript.finish_thinking();
        self.mark_dirty();
    }

    fn start_tool_call(&mut self, id: &str, name: String) {
        self.upsert_tool(id, name, None, ToolStatusKind::Pending);
        self.mark_dirty();
    }

    fn append_tool_call_arguments(&mut self, id: &str, json_fragment: &str) {
        let arguments = self.streaming_tool_args.entry(id.to_owned()).or_default();
        arguments.push_str(json_fragment);
        if let Some(tool) = self.transcript.tool_mut(id) {
            tool.update_call(Some(arguments.clone()));
            self.mark_dirty();
        }
    }

    fn finish_tool_call(&mut self, tool_call: AgentToolCall) {
        let arguments = tool_call.arguments.to_string();
        self.streaming_tool_args
            .insert(tool_call.id.clone(), arguments.clone());
        self.upsert_tool(
            &tool_call.id,
            tool_call.name,
            Some(arguments),
            ToolStatusKind::Pending,
        );
        self.mark_dirty();
    }

    fn start_tool_execution(&mut self, id: &str, name: String, arguments: &serde_json::Value) {
        let arguments = self
            .streaming_tool_args
            .get(id)
            .cloned()
            .unwrap_or_else(|| arguments.to_string());
        self.upsert_tool(id, name, Some(arguments), ToolStatusKind::Running);
        self.mark_dirty();
    }

    fn request_approval(
        &mut self,
        id: String,
        operation: PermissionOperation,
        subject: &str,
        arguments: &serde_json::Value,
        session_option_label: Option<String>,
        prefix_option_label: Option<String>,
    ) {
        self.upsert_approval(
            id,
            operation,
            subject,
            arguments,
            session_option_label,
            prefix_option_label,
        );
        self.mark_dirty();
    }

    fn update_tool_execution(&mut self, id: &str, name: String, partial_result: ToolResult) {
        self.upsert_tool(id, name, None, ToolStatusKind::Running);
        if let Some(tool) = self.transcript.tool_mut(id) {
            tool.append_live_output(partial_result.content);
        }
        self.mark_dirty();
    }

    fn finish_tool_execution(&mut self, id: String, name: String, result: ToolResult) {
        self.upsert_tool(&id, name, None, ToolStatusKind::Running);
        self.streaming_tool_args.remove(&id);
        if let Some(tool) = self.transcript.tool_mut(&id) {
            let details = result.details;
            let exit_code = details
                .as_ref()
                .and_then(|details| details.get("exit_code"))
                .and_then(serde_json::Value::as_i64)
                .and_then(|code| i32::try_from(code).ok());
            tool.set_result(Some(result.content), details, result.is_error, exit_code);
        }
        self.completed_tool_result_ids.push(id);
        self.mark_dirty();
    }

    fn start_shell_command(&mut self, id: &str, command: &str, cwd: &std::path::Path) {
        self.upsert_tool(
            id,
            "Bash".to_owned(),
            Some(format!("{command} ({})", cwd.display())),
            ToolStatusKind::Running,
        );
        self.mark_dirty();
    }

    fn finish_shell_command(
        &mut self,
        id: String,
        exit_code: Option<i32>,
        stdout: &str,
        stderr: &str,
        truncated: bool,
    ) {
        let detail = shell_finished_detail(exit_code, stdout, stderr, truncated);
        self.upsert_tool(&id, "Bash".to_owned(), None, ToolStatusKind::Running);
        if let Some(tool) = self.transcript.tool_mut(&id) {
            tool.set_result(Some(detail), None, exit_code != Some(0), exit_code);
        }
        self.completed_tool_result_ids.push(id);
        self.mark_dirty();
    }

    fn push_skill_activation(&mut self, name: String) {
        let description = self
            .skill_store
            .as_ref()
            .and_then(|store| store.get(&name))
            .map(|skill| skill.manifest.description.clone());
        self.push_transcript(TranscriptEntry::skill_activated(
            name,
            description,
            None::<String>,
        ));
    }

    fn push_goal_card(
        &mut self,
        kind: GoalCardKind,
        objective: String,
        detail: Option<String>,
        turns: Option<u32>,
    ) {
        self.push_transcript(TranscriptEntry::goal_card(kind, objective, detail, turns));
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        if self.width == width && self.height == height {
            return;
        }
        self.width = width;
        self.height = height;
        self.dirty = true;
    }

    pub fn render_tick(&mut self) {
        self.activity_frame = self.activity_frame.wrapping_add(1);
        if self.has_streaming_thinking() {
            self.mark_dirty();
        }
        let _ = self.render_frame(self.width, self.height);
    }

    /// Render a single flat frame of all non-chrome content lines as ANSI
    /// strings.
    ///
    /// The chrome (prompt box + footer) depends on [`NeoChromeState`] state and is
    /// appended by the caller via [`render_chrome_lines`] before the
    /// whole frame is handed to [`crate::terminal::TuiRenderer::render`].
    /// This uses the single-buffer model: every screen line lives in one
    /// `Vec<String>`, so the renderer can diff the whole frame and rewrite only
    /// what changed.
    ///
    /// Returns `None` when the transcript pane has no pending body changes.
    #[must_use]
    pub fn render_frame(&mut self, width: usize, height: usize) -> Option<Vec<String>> {
        if !self.dirty {
            return None;
        }
        self.dirty = false;
        self.width = width;
        self.height = height;

        let lines = self.render_body_lines(width);
        self.last_frame.clone_from(&lines);
        Some(lines)
    }

    /// Build the non-chrome body lines without consuming the dirty flag.
    /// Shared between [`render_frame`] (live path) and [`frame_ansi_lines`]
    /// (read-only snapshot for tests).
    ///
    fn render_body_lines(&mut self, width: usize) -> Vec<String> {
        let content_width = frame_content_width(width);
        self.render_transcript_rows(content_width)
            .into_iter()
            .map(|line| line.to_ansi())
            .collect()
    }

    /// Read-only snapshot of the most recently rendered body frame (ANSI
    /// strings, no chrome). Returns an empty vec before the first render.
    /// Used by tests that need to inspect what the runtime would draw.
    #[must_use]
    pub fn frame_ansi_lines(&self) -> Vec<String> {
        self.last_frame.clone()
    }

    #[must_use]
    pub const fn transcript(&self) -> &TranscriptStore {
        &self.transcript
    }

    pub fn transcript_mut(&mut self) -> &mut TranscriptStore {
        self.mark_dirty();
        &mut self.transcript
    }

    pub fn select_approval(&mut self, id: &str, selected: usize, feedback_input: &str) {
        if let Some(approval) = self.transcript.approval_mut(id) {
            approval.selected = selected;
            feedback_input.clone_into(&mut approval.feedback_input);
            self.mark_dirty();
        }
    }

    pub fn resolve_approval(&mut self, id: &str, label: impl Into<String>) {
        if let Some(approval) = self.transcript.approval_mut(id) {
            approval.resolved = Some(label.into());
            approval.queued_count = 0;
            self.advance_queued_approval();
            self.mark_dirty();
        }
    }

    pub fn resolve_unresolved_approvals(&mut self, label: impl Into<String>) {
        let label = label.into();
        let mut changed = false;
        for entry in self.transcript.entries_mut() {
            if let TranscriptEntry::ApprovalPrompt(data) = entry
                && data.resolved.is_none()
            {
                data.resolved = Some(label.clone());
                data.queued_count = 0;
                changed = true;
            }
        }
        if !self.queued_approvals.is_empty() {
            self.queued_approvals.clear();
            changed = true;
        }
        if changed {
            self.mark_dirty();
        }
    }

    #[must_use]
    pub const fn dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    fn upsert_tool(
        &mut self,
        id: &str,
        name: String,
        arguments: Option<String>,
        status: ToolStatusKind,
    ) {
        use crate::core::Expandable as _;

        if let Some(tool) = self.transcript.tool_mut(id) {
            tool.update_call_state(name, arguments, status);
            return;
        }

        self.finish_active_text_blocks();
        let mut component = ToolCallComponent::new(ToolCallState {
            id: id.to_owned(),
            name,
            arguments,
            result: None,
            details: None,
            status,
            exit_code: None,
        });
        component.set_expanded(self.tool_output_expanded);
        if let Some(workspace_root) = &self.workspace_root {
            component.set_workspace_dir(workspace_root.clone());
        }
        self.transcript.push(TranscriptEntry::tool_run(component));
    }

    fn apply_expand_state_to_entry(&self, mut entry: TranscriptEntry) -> TranscriptEntry {
        if let TranscriptEntry::ThinkingBlock { expanded, .. } = &mut entry {
            *expanded = self.tool_output_expanded;
        }
        entry
    }

    fn apply_expand_state_to_active_thinking(&mut self) {
        for entry in self.transcript.entries_mut().iter_mut().rev() {
            if let TranscriptEntry::ThinkingBlock { expanded, .. } = entry {
                *expanded = self.tool_output_expanded;
                break;
            }
        }
    }

    fn finish_active_text_blocks(&mut self) {
        self.finish_assistant_message();
        self.transcript.finish_thinking();
    }

    fn upsert_approval(
        &mut self,
        id: String,
        operation: PermissionOperation,
        subject: &str,
        arguments: &serde_json::Value,
        session_option_label: Option<String>,
        prefix_option_label: Option<String>,
    ) {
        let prompt = approval_prompt(operation, subject, arguments);

        if let Some(approval) = self.transcript.approval_mut(&id) {
            approval.title = prompt.title;
            approval.details = prompt.details;
            approval.queued_label = prompt.queued_label;
            approval.queued_count = self.queued_approvals.len();
            approval.resolved = None;
            approval
                .session_option_label
                .clone_from(&session_option_label);
            approval
                .prefix_option_label
                .clone_from(&prefix_option_label);
            return;
        }

        let data = ApprovalPromptData {
            id,
            title: prompt.title,
            details: prompt.details,
            queued_label: prompt.queued_label,
            queued_count: 0,
            selected: 0,
            feedback_input: String::new(),
            resolved: None,
            session_option_label,
            prefix_option_label,
        };
        if self.active_approval_mut().is_some() {
            self.queued_approvals.push_back(data);
            self.update_active_approval_queue_count();
            return;
        }

        self.finish_active_text_blocks();
        self.transcript.insert_approval_after_tool_or_push(data);
    }

    fn active_approval_mut(&mut self) -> Option<&mut ApprovalPromptData> {
        self.transcript
            .entries_mut()
            .iter_mut()
            .rev()
            .find_map(|entry| {
                if let TranscriptEntry::ApprovalPrompt(data) = entry
                    && data.resolved.is_none()
                {
                    return Some(data);
                }
                None
            })
    }

    fn update_active_approval_queue_count(&mut self) {
        let queued_count = self.queued_approvals.len();
        if let Some(approval) = self.active_approval_mut() {
            approval.queued_count = queued_count;
            self.mark_dirty();
        }
    }

    fn advance_queued_approval(&mut self) {
        let Some(mut next) = self.queued_approvals.pop_front() else {
            return;
        };
        next.queued_count = self.queued_approvals.len();
        self.transcript.insert_approval_after_tool_or_push(next);
    }

    fn upsert_compaction(
        &mut self,
        phase: Option<neo_agent_core::CompactionPhase>,
        percent: u8,
        compacted_message_count: usize,
        tokens_before: usize,
        tokens_after: usize,
    ) {
        if let Some(TranscriptEntry::Compaction {
            phase: existing_phase,
            percent: existing_percent,
            compacted_message_count: existing_count,
            tokens_before: existing_tokens,
            tokens_after: existing_tokens_after,
        }) = self
            .transcript
            .entries_mut()
            .iter_mut()
            .rev()
            .find(|entry| matches!(entry, TranscriptEntry::Compaction { .. }))
        {
            *existing_phase = phase;
            *existing_percent = percent;
            *existing_count = compacted_message_count;
            *existing_tokens = tokens_before;
            *existing_tokens_after = tokens_after;
        } else {
            self.transcript.push(TranscriptEntry::Compaction {
                phase,
                percent,
                compacted_message_count,
                tokens_before,
                tokens_after,
            });
        }
        self.mark_dirty();
    }

    fn update_compaction_progress(&mut self, phase: neo_agent_core::CompactionPhase, percent: u8) {
        if let Some(TranscriptEntry::Compaction {
            phase: existing_phase,
            percent: existing_percent,
            ..
        }) = self
            .transcript
            .entries_mut()
            .iter_mut()
            .rev()
            .find(|entry| matches!(entry, TranscriptEntry::Compaction { .. }))
        {
            *existing_phase = Some(phase);
            *existing_percent = percent;
        } else {
            self.upsert_compaction(Some(phase), percent, 0, 0, 0);
            return;
        }
        self.mark_dirty();
    }

    fn render_transcript_rows(&mut self, width: usize) -> Vec<Line> {
        let mut rows = Vec::new();
        let mut tool_run = Vec::new();
        let entries = self.transcript.entries().to_owned();

        for entry in entries {
            match entry {
                TranscriptEntry::ToolRun { component } => tool_run.push(component),
                entry => {
                    append_transcript_block(&mut rows, self.flush_tool_run(&mut tool_run, width));
                    append_transcript_block(
                        &mut rows,
                        entry.render_with_activity_frame(width, &self.theme, self.activity_frame),
                    );
                }
            }
        }
        append_transcript_block(&mut rows, self.flush_tool_run(&mut tool_run, width));
        rows
    }

    fn has_streaming_thinking(&self) -> bool {
        self.transcript.entries().iter().any(|entry| {
            matches!(
                entry,
                TranscriptEntry::ThinkingBlock {
                    phase: crate::transcript::ThinkingPhase::Streaming,
                    ..
                }
            )
        })
    }

    fn flush_tool_run(&mut self, tool_run: &mut Vec<ToolCallComponent>, width: usize) -> Vec<Line> {
        if tool_run.is_empty() {
            return Vec::new();
        }
        let mut ordered = std::mem::take(tool_run);
        render_ordered_tools(&mut ordered, width, &self.theme)
    }
}

struct ApprovalPromptSummary {
    title: String,
    details: Vec<String>,
    queued_label: String,
}

fn approval_prompt(
    operation: PermissionOperation,
    subject: &str,
    arguments: &serde_json::Value,
) -> ApprovalPromptSummary {
    let is_task_stop =
        operation == PermissionOperation::Shell && arguments.get("task_id").is_some();
    let is_terminal = operation == PermissionOperation::Shell && arguments.get("mode").is_some();
    let is_edit = operation == PermissionOperation::FileWrite
        && (arguments.get("old").is_some()
            || arguments.get("new").is_some()
            || arguments.get("replace_all").is_some());

    if is_task_stop {
        ApprovalPromptSummary {
            title: "Stop background task?".to_owned(),
            details: compact_details([
                labeled_argument(arguments, "task_id"),
                labeled_argument(arguments, "reason"),
            ]),
            queued_label: String::new(),
        }
    } else if is_terminal {
        ApprovalPromptSummary {
            title: terminal_approval_title(arguments),
            details: terminal_approval_details(arguments, subject),
            queued_label: String::new(),
        }
    } else if is_edit {
        ApprovalPromptSummary {
            title: "Edit file?".to_owned(),
            details: compact_details([
                labeled_argument(arguments, "path"),
                labeled_argument(arguments, "replace_all"),
            ]),
            queued_label: String::new(),
        }
    } else {
        match operation {
            PermissionOperation::Shell => ApprovalPromptSummary {
                title: "Run this command?".to_owned(),
                details: shell_approval_details(arguments, subject),
                queued_label: String::new(),
            },
            PermissionOperation::FileWrite => ApprovalPromptSummary {
                title: "Write file?".to_owned(),
                details: compact_details([labeled_argument(arguments, "path")]),
                queued_label: String::new(),
            },
            PermissionOperation::FileRead => ApprovalPromptSummary {
                title: "Read workspace data?".to_owned(),
                details: non_empty_details(
                    compact_details([
                        labeled_argument(arguments, "path"),
                        labeled_argument(arguments, "pattern"),
                    ]),
                    || vec![format!("target: {subject}")],
                ),
                queued_label: String::new(),
            },
            PermissionOperation::Tool => ApprovalPromptSummary {
                title: "Run tool?".to_owned(),
                details: compact_details([Some(format!("tool: {subject}"))]),
                queued_label: String::new(),
            },
            PermissionOperation::UserQuestion => ApprovalPromptSummary {
                title: "User question".to_owned(),
                details: compact_details([Some(subject.to_owned())]),
                queued_label: String::new(),
            },
            PermissionOperation::PlanTransition => ApprovalPromptSummary {
                title: "Plan mode transition".to_owned(),
                details: compact_details([Some(subject.to_owned())]),
                queued_label: String::new(),
            },
            PermissionOperation::GoalTransition => ApprovalPromptSummary {
                title: "Goal mode transition".to_owned(),
                details: compact_details([Some(subject.to_owned())]),
                queued_label: String::new(),
            },
        }
    }
}

fn shell_approval_details(arguments: &serde_json::Value, subject: &str) -> Vec<String> {
    let mut details = Vec::new();
    if let Some(cwd) = arguments
        .get("cwd")
        .or_else(|| arguments.get("workdir"))
        .and_then(serde_json::Value::as_str)
    {
        details.push(format!("cwd: {cwd}"));
    }
    let command = arguments
        .get("command")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(subject);
    details.push(format!("$ {command}"));
    details
}

fn terminal_approval_title(arguments: &serde_json::Value) -> String {
    match arguments
        .get("mode")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
    {
        "start" => "Start terminal?".to_owned(),
        "write" => "Write to terminal?".to_owned(),
        "resize" => "Resize terminal?".to_owned(),
        "stop" => "Stop terminal?".to_owned(),
        _ => "Use terminal?".to_owned(),
    }
}

fn terminal_approval_details(arguments: &serde_json::Value, subject: &str) -> Vec<String> {
    let mut details = compact_details([
        labeled_argument(arguments, "mode"),
        labeled_argument(arguments, "handle"),
    ]);
    if let Some(command) = arguments.get("command").and_then(serde_json::Value::as_str) {
        details.push(format!("$ {command}"));
    } else if !subject.is_empty() && details.is_empty() {
        details.push(format!("target: {subject}"));
    }
    details.extend(compact_details([
        labeled_argument(arguments, "input"),
        labeled_argument(arguments, "cols"),
        labeled_argument(arguments, "rows"),
    ]));
    details
}

fn labeled_argument(arguments: &serde_json::Value, key: &str) -> Option<String> {
    let value = arguments.get(key)?;
    match value {
        serde_json::Value::String(value) if !value.is_empty() => Some(format!("{key}: {value}")),
        serde_json::Value::Bool(value) => Some(format!("{key}: {value}")),
        serde_json::Value::Number(value) => Some(format!("{key}: {value}")),
        _ => None,
    }
}

fn compact_details(lines: impl IntoIterator<Item = Option<String>>) -> Vec<String> {
    lines.into_iter().flatten().collect()
}

fn non_empty_details(details: Vec<String>, fallback: impl FnOnce() -> Vec<String>) -> Vec<String> {
    if details.is_empty() {
        fallback()
    } else {
        details
    }
}

fn append_transcript_block(rows: &mut Vec<Line>, block: Vec<Line>) {
    let first = block.iter().position(|line| !line.text().trim().is_empty());
    let last = block
        .iter()
        .rposition(|line| !line.text().trim().is_empty());
    let (Some(first), Some(last)) = (first, last) else {
        return;
    };
    if rows
        .last()
        .is_some_and(|line| !line.text().trim().is_empty())
    {
        rows.push(Line::raw(""));
    }
    rows.extend(block.into_iter().skip(first).take(last - first + 1));
}

/// Render an ordered slice of tool components, collapsing consecutive runs of
/// the same groupable tool (read/grep/glob/find) into a single tree card.
///
/// A run of length 1 still renders as a normal solo card. Any non-groupable
/// tool (bash/edit/write/...) breaks an in-progress run. Live output buffers
/// are preserved because we render from the components directly (not cloned
/// states).
fn render_ordered_tools(
    ordered: &mut [ToolCallComponent],
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    let mut rows = Vec::new();
    let mut i = 0;
    while i < ordered.len() {
        if !rows.is_empty() {
            rows.push(Line::raw(""));
        }
        let current_name = ordered[i].name().to_owned();
        let groupable = is_groupable(&current_name);
        if !groupable {
            rows.extend(ordered[i].render_with_theme(width, theme));
            i += 1;
            continue;
        }
        // Greedy run of consecutive same-name groupable tools.
        let mut j = i + 1;
        while j < ordered.len()
            && ordered[j].name() == current_name
            && is_groupable(ordered[j].name())
        {
            j += 1;
        }
        if j - i >= 2 {
            // Group of >= 2: render as a tree card. Only group tools that are
            // NOT still streaming live output (a running read shows solo).
            let any_live_output = ordered[i..j].iter().any(|t| !t.progress().is_empty());
            if any_live_output {
                for tool in &mut ordered[i..j] {
                    rows.extend(tool.render_with_theme(width, theme));
                }
            } else {
                let states: Vec<&ToolCallState> =
                    ordered[i..j].iter().map(ToolCallComponent::state).collect();
                let expanded = ordered[i..j].iter().all(ToolCallComponent::is_expanded);
                let group = ToolGroup {
                    tool: current_name.clone(),
                    states,
                };
                rows.extend(render_tool_group(&group, width, theme, expanded));
            }
        } else {
            rows.extend(ordered[i].render_with_theme(width, theme));
        }
        i = j;
    }
    rows
}

/// Whether a tool name is eligible for consecutive-call grouping.
fn is_groupable(name: &str) -> bool {
    matches!(name, "Read" | "Grep" | "Glob" | "Find" | "List")
}

fn content_display_text(content: &[Content]) -> String {
    content.iter().filter_map(content_visible_text).collect()
}

fn content_visible_text(content: &Content) -> Option<String> {
    match content {
        Content::Text { text } => Some(text.clone()),
        Content::Thinking { .. } => None,
        Content::Image { mime_type, data } => Some(image_summary(mime_type, data)),
    }
}

fn image_summary(mime_type: &str, data: &ImageRef) -> String {
    match data {
        ImageRef::Url(url) => format!("[image: {mime_type} url={}]", sanitized_image_url(url)),
        ImageRef::Base64(data) => format!("[image: {mime_type} data={} bytes]", data.len()),
        ImageRef::Blob(sha256) => format!("[image: {mime_type} blob={sha256}]"),
    }
}

fn sanitized_image_url(url: &str) -> String {
    let end = url.find(['?', '#']).unwrap_or(url.len());
    url[..end].to_owned()
}

fn decode_base64(encoded: &str) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(encoded.len() / 4 * 3);
    let mut buffer = 0_u32;
    let mut bits = 0_u8;

    for byte in encoded.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        if byte == b'=' {
            break;
        }
        let value = base64_value(byte)?;
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        while bits >= 8 {
            bits -= 8;
            output.push(((buffer >> bits) & 0xff) as u8);
        }
    }

    Some(output)
}

const fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn shell_finished_detail(
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
    truncated: bool,
) -> String {
    let mut detail = format!("{stdout}{stderr}");
    if exit_code != Some(0) {
        let exit_label = exit_code.map_or_else(|| "signal".to_owned(), |code| code.to_string());
        if !detail.ends_with('\n') && !detail.is_empty() {
            detail.push('\n');
        }
        let _ = write!(detail, "Command failed with exit code: {exit_label}.");
    }
    if truncated {
        if !detail.ends_with('\n') && !detail.is_empty() {
            detail.push('\n');
        }
        detail.push_str("[output truncated]");
    }
    detail
}

fn run_finished_notice(turn: u32, stop_reason: neo_agent_core::StopReason) -> Option<String> {
    match stop_reason {
        neo_agent_core::StopReason::MaxTokens => Some(format!(
            "Run stopped after turn {turn}: response hit the output length cap (max_tokens). \
             Raise [models.<alias>].max_output_tokens or [runtime].max_tokens to continue."
        )),
        neo_agent_core::StopReason::Error => {
            Some(format!("Run stopped after turn {turn}: runtime error."))
        }
        neo_agent_core::StopReason::Cancelled => {
            Some(format!("Run stopped after turn {turn}: cancelled."))
        }
        neo_agent_core::StopReason::EndTurn | neo_agent_core::StopReason::ToolUse => None,
    }
}

fn take_completed_tool_result(completed_tool_result_ids: &mut Vec<String>, id: &str) -> bool {
    if let Some(index) = completed_tool_result_ids
        .iter()
        .position(|completed_id| completed_id == id)
    {
        completed_tool_result_ids.remove(index);
        true
    } else {
        false
    }
}

/// Chrome lines, optional cursor position, and the row where the prompt box
/// starts within those lines.
pub struct ChromeRender {
    pub lines: Vec<String>,
    pub cursor: Option<CursorPos>,
    pub prompt_start_row: usize,
}

#[must_use]
pub fn render_chrome_lines(app: &NeoChromeState, width: usize, height: usize) -> ChromeRender {
    let content_width = frame_content_width(width);
    let mut lines = Vec::new();
    if app.has_todos() {
        lines.extend(
            TodoPanel::new(app.todo_items())
                .with_theme(app.theme())
                .render(content_width),
        );
    }
    if let Some(question) = app.question_dialog_state() {
        lines.extend(question.render_lines(content_width));
    }
    if let Some(btw_state) = app.btw_panel_state() {
        let terminal_rows = u16::try_from(height).unwrap_or(u16::MAX);
        let mut btw_state = btw_state.clone();
        lines.extend(
            crate::widgets::BtwPanel::new(&mut btw_state)
                .with_theme(app.theme())
                .render(content_width, terminal_rows),
        );
    }
    let pending_input = PendingInputPreview::new(
        app.pending_input().pending_steers(),
        app.pending_input().queued_follow_ups(),
    )
    .with_theme(app.theme())
    .render(content_width);
    if !pending_input.is_empty() {
        lines.extend(pending_input);
        lines.push(String::new());
    }
    let prompt_start_row = lines.len();
    let (prompt_lines, prompt_cursor) = if app.focused_overlay_blocks_prompt() {
        (Vec::new(), None)
    } else {
        render_prompt_lines(app, content_width)
    };
    lines.extend(prompt_lines);
    if !app.focused_overlay_blocks_prompt()
        && let Some(dropdown) = render_prompt_completion_dropdown(app, content_width)
    {
        lines.extend(dropdown);
    }
    lines.extend(render_footer_lines(app, content_width));
    ChromeRender {
        lines,
        cursor: prompt_cursor,
        prompt_start_row,
    }
}

/// Mutable variant of [`render_chrome_lines`] that updates the `/btw` panel's
/// internal scroll and height state instead of discarding those updates.
#[must_use]
pub fn render_chrome_lines_mut(
    app: &mut NeoChromeState,
    width: usize,
    height: usize,
) -> ChromeRender {
    let content_width = frame_content_width(width);
    let mut lines = Vec::new();
    if app.has_todos() {
        lines.extend(
            TodoPanel::new(app.todo_items())
                .with_theme(app.theme())
                .render(content_width),
        );
    }
    if let Some(question) = app.question_dialog_state() {
        lines.extend(question.render_lines(content_width));
    }
    let terminal_rows = u16::try_from(height).unwrap_or(u16::MAX);
    let theme = app.theme();
    if let Some(btw_state) = app.btw_panel_state_mut() {
        lines.extend(
            crate::widgets::BtwPanel::new(btw_state)
                .with_theme(theme)
                .render(content_width, terminal_rows),
        );
    }
    let pending_input = PendingInputPreview::new(
        app.pending_input().pending_steers(),
        app.pending_input().queued_follow_ups(),
    )
    .with_theme(app.theme())
    .render(content_width);
    if !pending_input.is_empty() {
        lines.extend(pending_input);
        lines.push(String::new());
    }
    let prompt_start_row = lines.len();
    let (prompt_lines, prompt_cursor) = if app.focused_overlay_blocks_prompt() {
        (Vec::new(), None)
    } else {
        render_prompt_lines(app, content_width)
    };
    lines.extend(prompt_lines);
    if !app.focused_overlay_blocks_prompt()
        && let Some(dropdown) = render_prompt_completion_dropdown(app, content_width)
    {
        lines.extend(dropdown);
    }
    lines.extend(render_footer_lines(app, content_width));
    ChromeRender {
        lines,
        cursor: prompt_cursor,
        prompt_start_row,
    }
}

/// Render only the footer status line, without the prompt box. Used when a
/// session picker overlay replaces the prompt/editor area.
#[must_use]
pub fn render_footer_only_lines(app: &NeoChromeState, width: usize) -> Vec<String> {
    let content_width = frame_content_width(width);
    render_footer_lines(app, content_width)
}

#[must_use]
pub fn frame_content_width(width: usize) -> usize {
    width.saturating_sub(CHROME_GUTTER + 1).max(1)
}

/// Render the `/` command dropdown below the prompt box, if active.
fn render_prompt_completion_dropdown(app: &NeoChromeState, width: usize) -> Option<Vec<String>> {
    let overlay = app.focused_overlay()?;
    let crate::chrome::OverlayKind::PromptCompletion(state) = &overlay.kind else {
        return None;
    };
    let inner_width = width.saturating_sub(2).max(1);
    let raw_lines = state.render_lines(inner_width);
    if raw_lines.is_empty() {
        return None;
    }
    let theme = app.theme();
    let border_style = Style::default().fg(theme.brand);
    let mut lines = Vec::with_capacity(raw_lines.len() + 1);
    for raw in raw_lines {
        lines.push(box_draw::side_bordered_line(&raw, width, border_style));
    }
    lines.push(box_draw::bottom_border(width, border_style));
    Some(lines)
}

/// Render the rounded prompt input box. The first content line carries the
/// `> ` prompt symbol; continuation lines use a 4-space hanging indent so
/// wrapped/explicit-newline text aligns under the body (matching Neo's
/// `paddingX: 4` editor). Border color is weak by default and switches to
/// the brand color when text is present or plan mode is active.
fn render_prompt_lines(app: &NeoChromeState, width: usize) -> (Vec<String>, Option<CursorPos>) {
    let theme = app.theme();
    let prompt = app.prompt();
    let highlighted = app.is_plan_mode() || !prompt.text.is_empty();
    let border_color = if highlighted {
        theme.brand
    } else {
        theme.text_muted
    };
    let border_style = Style::default().fg(border_color);
    let text_style = Style::default().fg(theme.text_primary);

    let inner_width = width.saturating_sub(2).max(1);
    let body_width = inner_width.saturating_sub(4).max(1);

    let logical_lines = build_prompt_logical_lines(prompt, body_width);

    // Total wrapped lines, counting empty logical lines as one display row.
    // Tabs must be expanded first so the count matches what build_prompt_logical_lines renders.
    let total_lines: usize = prompt
        .text
        .split('\n')
        .map(|line| {
            wrap_width(&expand_prompt_tabs(line), body_width)
                .len()
                .max(1)
        })
        .sum();
    let scroll_offset = prompt.scroll_offset();
    let lines_below = total_lines.saturating_sub(scroll_offset + logical_lines.len());

    let mut lines = Vec::with_capacity(logical_lines.len() + 2);
    lines.push(if scroll_offset > 0 {
        scroll_indicator_top_border(width, scroll_offset, border_style)
    } else {
        box_draw::top_border(width, border_style)
    });
    for (idx, line) in logical_lines.iter().enumerate() {
        let prefix = if idx == 0 { "  > " } else { "    " };
        let content = paint(&format!("{prefix}{line}"), text_style);
        lines.push(box_draw::content_line(&content, width, border_style));
    }
    lines.push(if lines_below > 0 {
        scroll_indicator_bottom_border(width, lines_below, border_style)
    } else {
        box_draw::bottom_border(width, border_style)
    });

    let cursor = find_cursor(&lines);
    let lines = lines
        .into_iter()
        .map(|line| line.replace(CURSOR_MARKER, ""))
        .collect();
    (lines, cursor)
}

/// Build the per-line content (already wrapped) for the prompt, inserting the
/// cursor marker on the active line. Each returned string is the body text
/// (without the `  > `/`    ` prefix, which is added by the caller).
fn build_prompt_logical_lines(prompt: &PromptState, body_width: usize) -> Vec<String> {
    let text = &prompt.text;
    let cursor = prompt.cursor.min(prompt.char_len());

    // Highlight selected marker before inserting the cursor marker.
    let styled_text = if let Some((start_byte, end_byte)) = prompt.selected_marker() {
        let start_char = text[..start_byte].chars().count();
        let end_char = text[..end_byte].chars().count();
        let before = &text[..start_byte];
        let selected = &text[start_byte..end_byte];
        let after = &text[end_byte..];
        let highlighted = paint(selected, Style::default().bg(Color::Rgb(60, 60, 60)));
        let mut styled = String::with_capacity(text.len() + highlighted.len() - selected.len());
        styled.push_str(before);
        styled.push_str(&highlighted);
        styled.push_str(after);

        // Insert the cursor marker at the correct position in the styled text.
        let cursor_byte = if cursor <= start_char {
            prompt.byte_index(cursor)
        } else if cursor >= end_char {
            prompt.byte_index(cursor) + highlighted.len() - selected.len()
        } else {
            // Cursor inside the selected range: place it at the start of the
            // highlighted region.
            start_byte
        };
        let mut with_cursor = String::with_capacity(styled.len() + CURSOR_MARKER.len());
        with_cursor.push_str(&styled[..cursor_byte]);
        with_cursor.push_str(CURSOR_MARKER);
        with_cursor.push_str(&styled[cursor_byte..]);
        with_cursor
    } else {
        let chars: Vec<char> = text.chars().collect();
        let before: String = chars[..cursor].iter().collect();
        let after: String = chars[cursor..].iter().collect();
        format!("{before}{CURSOR_MARKER}{after}")
    };

    let marked = expand_prompt_tabs(&styled_text);
    let mut all_lines = Vec::new();
    for logical in marked.split('\n') {
        let wrapped = wrap_width(logical, body_width);
        if wrapped.is_empty() {
            all_lines.push(String::new());
        } else {
            all_lines.extend(wrapped);
        }
    }
    if all_lines.len() <= MAX_PROMPT_VISIBLE_LINES {
        return all_lines;
    }
    let max_offset = all_lines.len().saturating_sub(MAX_PROMPT_VISIBLE_LINES);
    let scroll_offset = prompt.scroll_offset().min(max_offset);
    all_lines
        .into_iter()
        .skip(scroll_offset)
        .take(MAX_PROMPT_VISIBLE_LINES)
        .collect()
}

fn scroll_indicator_top_border(width: usize, count: usize, style: Style) -> String {
    scroll_indicator_border(
        width,
        count,
        "↑",
        style,
        ROUNDED.top_left,
        ROUNDED.top_right,
    )
}

fn scroll_indicator_bottom_border(width: usize, count: usize, style: Style) -> String {
    scroll_indicator_border(
        width,
        count,
        "↓",
        style,
        ROUNDED.bottom_left,
        ROUNDED.bottom_right,
    )
}

fn scroll_indicator_border(
    width: usize,
    count: usize,
    arrow: &str,
    style: Style,
    left_corner: char,
    right_corner: char,
) -> String {
    if width < 4 {
        return format!(
            "{}{}{}",
            paint(&left_corner.to_string(), style),
            paint(
                &repeat_char(ROUNDED.horizontal, width.saturating_sub(2)),
                style
            ),
            paint(&right_corner.to_string(), style)
        );
    }
    let label = format!(" {arrow} {count} more ");
    let label_width = visible_width(&label);
    let inner = width.saturating_sub(2);
    if label_width >= inner {
        return format!(
            "{}{}{}",
            paint(&left_corner.to_string(), style),
            paint(&repeat_char(ROUNDED.horizontal, inner), style),
            paint(&right_corner.to_string(), style)
        );
    }
    let bars = inner.saturating_sub(label_width);
    let left_bars = bars / 2;
    let right_bars = bars - left_bars;
    format!(
        "{}{}{}{}{}",
        paint(&left_corner.to_string(), style),
        paint(&repeat_char(ROUNDED.horizontal, left_bars), style),
        paint(&label, style),
        paint(&repeat_char(ROUNDED.horizontal, right_bars), style),
        paint(&right_corner.to_string(), style)
    )
}

fn expand_prompt_tabs(text: &str) -> String {
    if !text.contains('\t') {
        return text.to_owned();
    }
    text.replace('\t', "    ")
}

fn find_cursor(lines: &[String]) -> Option<CursorPos> {
    for (row, line) in lines.iter().enumerate() {
        if let Some(byte_pos) = line.find(CURSOR_MARKER) {
            let col = visible_width(&line[..byte_pos]);
            return Some(CursorPos { row, col });
        }
    }
    None
}

fn render_footer_lines(app: &NeoChromeState, width: usize) -> Vec<String> {
    let theme = app.theme();
    let (perm_label, perm_color) = app.permission_badge();
    let mut left_parts = vec![paint(
        &format!("[{perm_label}]"),
        Style::default().fg(perm_color),
    )];
    if let Some(label) = development_mode_badge(app.development_mode()) {
        left_parts.push(paint(label, Style::default().fg(theme.status_warn).bold()));
    }
    if !app.model_label().is_empty() {
        left_parts.push(paint(
            app.model_label(),
            Style::default().fg(theme.text_muted),
        ));
    }
    if app.thinking_enabled() {
        left_parts.push(paint(
            "thinking",
            Style::default().fg(theme.footer_working).italic(),
        ));
    }
    if let Some(exit) = app.exit_confirmation_label() {
        left_parts.push(paint(exit, Style::default().fg(theme.status_warn).bold()));
    }
    if let Some(working) = app.working_label() {
        const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let spinner = SPINNER[app.activity_frame() % SPINNER.len()];
        left_parts.push(paint(
            &format!("{spinner} {working}"),
            Style::default().fg(theme.footer_working),
        ));
    }
    left_parts.push(paint(
        &app.cwd_label(),
        Style::default().fg(theme.text_muted),
    ));
    if let Some(git_status) = app.git_status_label() {
        left_parts.push(render_git_status_label(git_status, theme));
    }

    let left_text = left_parts.join(" ");
    let row = if let Some(context) = app.context_window_label() {
        let context = paint(&context, Style::default().fg(app.context_color()));
        let total = visible_width(&left_text) + visible_width(&context);
        if total < width {
            format!("{left_text}{}{context}", " ".repeat(width - total))
        } else {
            let room = width
                .saturating_sub(visible_width(&context))
                .saturating_sub(1);
            format!("{} {context}", truncate_to_width(&left_text, room))
        }
    } else {
        truncate_to_width(&left_text, width)
    };

    vec![row]
}

fn development_mode_badge(mode: DevelopmentMode) -> Option<&'static str> {
    match mode {
        DevelopmentMode::Normal => None,
        DevelopmentMode::Plan => Some("[plan]"),
        DevelopmentMode::Goal(GoalModeStatus::Pending) => Some("[goal]"),
        DevelopmentMode::Goal(GoalModeStatus::Active) => Some("[goal•]"),
        DevelopmentMode::Goal(GoalModeStatus::Paused) => Some("[goal◌]"),
        DevelopmentMode::Goal(GoalModeStatus::Blocked) => Some("[goal✗]"),
    }
}

fn render_git_status_label(label: &str, theme: TuiTheme) -> String {
    let Some((branch, rest)) = label.rsplit_once(" [") else {
        return paint(label, Style::default().fg(GITHUB_YELLOW));
    };
    let status = rest.strip_suffix(']').unwrap_or(rest);
    let mut rendered = paint(branch, Style::default().fg(GITHUB_YELLOW));
    rendered.push_str(&paint(" [", Style::default().fg(theme.text_muted)));
    let mut first = true;
    for part in status.split(' ').filter(|part| !part.is_empty()) {
        if first {
            first = false;
        } else {
            rendered.push_str(&paint(" ", Style::default().fg(theme.text_muted)));
        }
        rendered.push_str(&render_git_status_part(part, theme));
    }
    rendered.push_str(&paint("]", Style::default().fg(theme.text_muted)));
    rendered
}

fn render_git_status_part(part: &str, theme: TuiTheme) -> String {
    let color = if part.starts_with('+') {
        GITHUB_GREEN
    } else if part.starts_with('-') {
        GITHUB_RED
    } else if part.starts_with('↑') || part.starts_with('↓') {
        GITHUB_BLUE
    } else {
        theme.text_muted
    };
    paint(part, Style::default().fg(color))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::{NeoChromeState, PickerItem, PromptCompletionPrefix, PromptEdit, TuiTheme};

    #[test]
    fn prompt_box_lines_are_exact_width() {
        let mut app = NeoChromeState::new("neo", "s", "m", "/tmp");
        app.set_theme(TuiTheme::default());
        app.prompt_mut()
            .apply_edit(PromptEdit::Insert("hello world"));
        let render = render_chrome_lines(&app, 40, 24);
        // Lines render below terminal width so the caller can apply
        // CHROME_GUTTER without triggering terminal autowrap.
        let expected_width = frame_content_width(40);
        for line in &render.lines {
            assert!(
                crate::ansi::visible_width(line) <= expected_width,
                "line: {line:?}"
            );
        }
        // The prompt box borders and content rows must be exactly content_width.
        let prompt_box_lines: Vec<&String> = render
            .lines
            .iter()
            .filter(|l| {
                let s = crate::ansi::strip_ansi(l);
                s.starts_with('│') || s.starts_with('╭') || s.starts_with('╰')
            })
            .collect();
        assert!(!prompt_box_lines.is_empty(), "prompt box lines missing");
        for line in prompt_box_lines {
            assert_eq!(
                crate::ansi::visible_width(line),
                expected_width,
                "line: {line:?}"
            );
        }
    }

    #[test]
    fn completion_dropdown_is_below_prompt() {
        let mut app = NeoChromeState::new("neo", "s", "m", "/tmp");
        app.prompt_mut().apply_edit(PromptEdit::Insert("/"));
        app.open_prompt_completion_picker(
            PromptCompletionPrefix {
                start: 0,
                end: 1,
                text: "/".to_owned(),
            },
            vec![
                PickerItem::new("/model", "model", Some("switch model")),
                PickerItem::new("/plan", "plan", Some("toggle plan")),
            ],
        );
        let render = render_chrome_lines(&app, 60, 24);
        // First line is the prompt top border.
        assert!(render.lines[0].contains('╭'));
        let dropdown_start = render
            .lines
            .iter()
            .position(|l| l.contains("model"))
            .expect("dropdown missing");
        assert!(dropdown_start > 1);
        // The line immediately before the dropdown must be the prompt bottom border.
        assert!(render.lines[dropdown_start - 1].contains('╰'));
        // Dropdown items are side-bordered.
        let stripped = crate::ansi::strip_ansi(&render.lines[dropdown_start]);
        assert!(stripped.starts_with('│'));
        assert!(stripped.ends_with('│'));
    }
}
