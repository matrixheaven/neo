use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;

use neo_agent_core::{AgentEvent, AgentMessage, Content, ImageRef, skills::SkillStore};

use crate::primitive::theme::TuiTheme;
use crate::primitive::{Expandable, Line};
use crate::shell::ToolStatusKind;
use crate::terminal_image::{
    ImageRenderPolicy, ImageSource, InlineImage, TerminalImageCapabilities,
};
use crate::transcript::{
    ApprovalPromptData, InlineImageRender, ShellRunComponent, ToolCallComponent, ToolCallState,
    TranscriptEntry, TranscriptStore,
};

const DEFAULT_LIVE_CHROME_HEIGHT: usize = 4;

#[derive(Debug, Clone)]
pub struct TranscriptPane {
    width: usize,
    height: usize,
    live_chrome_height: usize,
    pub(super) transcript: TranscriptStore,
    dirty: bool,
    tool_output_expanded: bool,
    pub(super) streaming_tool_args: BTreeMap<String, String>,
    pub(super) queued_approvals: VecDeque<ApprovalPromptData>,
    pub(super) completed_tool_result_ids: Vec<String>,
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
    pub(super) skill_store: Option<SkillStore>,
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

    /// Push a status entry with explicit severity.
    pub fn push_status_with_severity(
        &mut self,
        content: impl Into<String>,
        severity: crate::transcript::entry::StatusSeverity,
    ) {
        self.push_transcript(TranscriptEntry::Status {
            text: content.into(),
            severity: Some(severity),
        });
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
            AgentMessage::ShellCommand {
                command,
                stdout,
                stderr,
                exit_code,
                outcome,
                truncated,
            } => {
                let id = format!("replay-shell-{}", self.transcript.entries().len());
                self.push_transcript(TranscriptEntry::shell_run(ShellRunComponent::finished(
                    id,
                    command.clone(),
                    stdout.clone(),
                    stderr.clone(),
                    *exit_code,
                    outcome.clone(),
                    *truncated,
                )));
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
        self.mark_dirty();
    }

    pub fn scroll_transcript_down(&mut self, rows: usize) {
        self.transcript.viewport_mut().scroll_down(rows);
        self.mark_dirty();
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
                TranscriptEntry::Delegate { component } => component.set_expanded(expanded),
                TranscriptEntry::DelegateSwarm { component } => component.set_expanded(expanded),
                TranscriptEntry::Workflow { component } => component.set_expanded(expanded),
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
        let now_ms = current_time_ms();
        if self.transcript.tick_live_entries(now_ms) || self.has_streaming_thinking() {
            self.mark_dirty();
        }
        let _ = self.render_frame(self.width, self.height);
    }

    #[must_use]
    pub const fn is_dirty_for_test(&self) -> bool {
        self.dirty
    }

    pub fn render_tick_at_ms_for_test(&mut self, now_ms: u64) {
        self.activity_frame = self.activity_frame.wrapping_add(1);
        if self.transcript.tick_live_entries(now_ms) || self.has_streaming_thinking() {
            self.mark_dirty();
        }
    }

    /// Render a single flat frame of all non-chrome content lines as ANSI
    /// strings.
    ///
    /// The chrome (prompt box + footer) depends on [`NeoChromeState`] state and is
    /// appended by the caller via [`render_chrome_lines`] before the
    /// whole frame is handed to [`crate::screen_output::TuiRenderer::render`].
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
        let content_width = super::chrome_render::frame_content_width(width);
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

    #[must_use]
    pub const fn dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    pub(super) fn upsert_tool(
        &mut self,
        id: &str,
        name: String,
        arguments: Option<String>,
        status: ToolStatusKind,
    ) {
        use crate::primitive::Expandable as _;

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

    pub(super) fn apply_expand_state_to_active_thinking(&mut self) {
        for entry in self.transcript.entries_mut().iter_mut().rev() {
            if let TranscriptEntry::ThinkingBlock { expanded, .. } = entry {
                *expanded = self.tool_output_expanded;
                break;
            }
        }
    }

    pub(super) fn finish_active_text_blocks(&mut self) {
        self.finish_assistant_message();
        self.transcript.finish_thinking();
    }

    pub(super) fn upsert_compaction(
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

    pub(super) fn update_compaction_progress(
        &mut self,
        phase: neo_agent_core::CompactionPhase,
        percent: u8,
    ) {
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

        let viewport_rows = self.height.saturating_sub(self.live_chrome_height).max(1);
        self.transcript
            .viewport_mut()
            .sync(rows.len(), viewport_rows);
        let range = self
            .transcript
            .viewport()
            .visible_row_range(rows.len(), viewport_rows);
        rows.into_iter()
            .skip(range.start)
            .take(range.len())
            .collect()
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
        super::chrome_render::render_ordered_tools(&mut ordered, width, &self.theme)
    }
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
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
