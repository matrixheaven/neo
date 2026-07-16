use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use neo_agent_core::{AgentEvent, AgentMessage, Content, ImageRef, skills::SkillStore};

use crate::primitive::theme::TuiTheme;
use crate::primitive::{Finalization, Line, next_sequence};
use crate::shell::ToolStatusKind;
use crate::terminal_image::{
    ImageRenderPolicy, ImageSource, InlineImage, TerminalImageCapabilities,
};
use crate::transcript::{
    ApprovalPromptData, InlineImageRender, McpStartupStatusData, ShellRunComponent,
    ToolCallComponent, ToolCallState, TranscriptBrowserState, TranscriptEntry, TranscriptStore,
};

use super::presentation::{FinalizedBlock, TranscriptPresentation, TranscriptTerminalUpdate};

const DEFAULT_LIVE_CHROME_HEIGHT: usize = 4;

fn compaction_is_complete(phase: Option<neo_agent_core::CompactionPhase>, percent: u8) -> bool {
    phase == Some(neo_agent_core::CompactionPhase::Applying) && percent >= 100
}

fn is_live_compaction_entry(entry: &TranscriptEntry) -> bool {
    matches!(
        entry,
        TranscriptEntry::Compaction { phase, percent, .. }
            if !compaction_is_complete(*phase, *percent)
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum AbsorbedToolKind {
    Delegate,
    DelegateSwarm,
}

impl AbsorbedToolKind {
    const fn tool_name(self) -> &'static str {
        match self {
            Self::Delegate => "Delegate",
            Self::DelegateSwarm => "DelegateSwarm",
        }
    }

    fn from_tool_name(name: &str) -> Option<Self> {
        match name {
            "Delegate" => Some(Self::Delegate),
            "DelegateSwarm" => Some(Self::DelegateSwarm),
            _ => None,
        }
    }

    fn details_match_target(self, details: &serde_json::Value, targets: &BTreeSet<String>) -> bool {
        match self {
            Self::Delegate => {
                details.get("kind").and_then(serde_json::Value::as_str) == Some("delegate")
                    && ["agent_id", "id"]
                        .iter()
                        .filter_map(|key| details.get(*key).and_then(serde_json::Value::as_str))
                        .any(|id| targets.contains(id))
            }
            Self::DelegateSwarm => {
                details.get("kind").and_then(serde_json::Value::as_str) == Some("delegate_swarm")
                    && [
                        details.get("swarm_id").and_then(serde_json::Value::as_str),
                        details.get("id").and_then(serde_json::Value::as_str),
                        details
                            .get("swarm")
                            .and_then(|swarm| swarm.get("swarm_id"))
                            .and_then(serde_json::Value::as_str),
                    ]
                    .into_iter()
                    .flatten()
                    .any(|id| targets.contains(id))
            }
        }
    }
}

#[derive(Debug, Clone)]
struct TranscriptBodyCache {
    width: usize,
    entry_count: usize,
    rows: Vec<String>,
    entry_row_starts: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct TranscriptPane {
    width: usize,
    height: usize,
    live_chrome_height: usize,
    pub(super) transcript: TranscriptStore,
    dirty: bool,
    tool_output_expanded: bool,
    pub(super) streaming_tool_args: BTreeMap<String, String>,
    tool_call_metadata: BTreeMap<String, (u32, String)>,
    delegate_absorption_targets: BTreeMap<(u32, AbsorbedToolKind), BTreeSet<String>>,
    pub(super) queued_approvals: VecDeque<ApprovalPromptData>,
    pub(super) completed_tool_result_ids: Vec<String>,
    replay_plan_snapshot: Option<ReplayPlanSnapshot>,
    next_image_id: u64,
    activity_frame: usize,
    workspace_root: Option<PathBuf>,
    /// Cache of the last composed body frame (ANSI strings, no chrome), so
    /// tests can inspect rendered output via [`frame_ansi_lines`] without
    /// recomposing unchanged rows.
    last_frame: Vec<String>,
    body_cache: Option<TranscriptBodyCache>,
    #[cfg(test)]
    last_reused_prefix_rows: usize,
    /// Theme used to color the live transcript body. Mirrors [`NeoChromeState`]'s
    /// theme; kept here (rather than borrowed) so the runtime can render
    /// without holding a reference to the app. The interactive mode keeps it
    /// in sync via [`Self::set_theme`].
    theme: TuiTheme,
    image_render_policy: ImageRenderPolicy,
    image_capabilities: TerminalImageCapabilities,
    presentation: TranscriptPresentation,
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
            tool_call_metadata: BTreeMap::new(),
            delegate_absorption_targets: BTreeMap::new(),
            queued_approvals: VecDeque::new(),
            completed_tool_result_ids: Vec::new(),
            replay_plan_snapshot: None,
            next_image_id: 0,
            activity_frame: 0,
            workspace_root: None,
            last_frame: Vec::new(),
            body_cache: None,
            #[cfg(test)]
            last_reused_prefix_rows: 0,
            theme: TuiTheme::default(),
            image_render_policy: ImageRenderPolicy::default(),
            image_capabilities: TerminalImageCapabilities::default(),
            presentation: TranscriptPresentation::default(),
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
        self.body_cache = None;
        self.transcript.invalidate_render_cache();
        self.mark_dirty();
    }

    pub fn set_image_render_policy(&mut self, policy: ImageRenderPolicy) {
        if self.image_render_policy == policy {
            return;
        }
        self.image_render_policy = policy;
        self.body_cache = None;
        self.transcript.invalidate_render_cache();
        self.mark_dirty();
    }

    pub fn set_image_capabilities(&mut self, capabilities: TerminalImageCapabilities) {
        if self.image_capabilities == capabilities {
            return;
        }
        self.image_capabilities = capabilities;
        self.body_cache = None;
        self.transcript.invalidate_render_cache();
        self.mark_dirty();
    }

    pub fn set_workspace_root(&mut self, workspace_root: impl Into<PathBuf>) {
        let path = workspace_root.into();
        if self.workspace_root.as_deref() == Some(&path) {
            return;
        }
        self.workspace_root = Some(path);
        for index in 0..self.transcript.entries().len() {
            if !matches!(
                self.transcript.entries()[index],
                TranscriptEntry::ToolRun { .. }
            ) {
                continue;
            }
            self.transcript.mutate_entry(index, |entry| match entry {
                TranscriptEntry::ToolRun { component } => {
                    component.set_workspace_dir(self.workspace_root.clone().unwrap_or_default())
                }
                _ => false,
            });
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

    pub fn push_user_message_with_images(
        &mut self,
        content: impl Into<String>,
        images: Vec<crate::transcript::TranscriptImageAttachment>,
    ) {
        self.push_transcript(TranscriptEntry::user_message_with_images(content, images));
    }

    /// Push a queued (Enter while busy) or steered (Ctrl+S) message preview
    /// into the transcript. Rendered with a distinct prefix so the user sees
    /// visual feedback that their input was captured mid-turn.
    pub fn push_queued_message(&mut self, content: impl Into<String>, is_steer: bool) {
        self.push_transcript(TranscriptEntry::queued_message(content, is_steer));
    }

    /// Pop the oldest queued follow-up entry from the transcript. Used
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

    pub fn upsert_mcp_startup_status(&mut self, data: McpStartupStatusData) -> bool {
        let existing_index = self
            .transcript
            .entries()
            .iter()
            .position(|entry| matches!(entry, TranscriptEntry::McpStartupStatus { data: existing } if existing.id == data.id));
        if let Some(index) = existing_index {
            if self.transcript.entry_finalization(index) == Some(Finalization::Finalized) {
                return false;
            }
            let changed = self.transcript.mutate_entry(index, |entry| {
                let next = TranscriptEntry::mcp_startup_status(data);
                if *entry == next {
                    return false;
                }
                *entry = next;
                true
            });
            if changed {
                self.mark_dirty();
            }
            changed
        } else {
            self.push_transcript(TranscriptEntry::mcp_startup_status(data));
            true
        }
    }

    pub fn replay_message(&mut self, message: &AgentMessage) {
        if message.is_injection() {
            return;
        }
        match message {
            AgentMessage::User { content, .. } => {
                let (text, images) = user_content_display(content);
                if !text.is_empty() {
                    if images.is_empty() {
                        self.replay_user_message(text);
                    } else {
                        self.push_user_message_with_images(text, images);
                    }
                }
            }
            AgentMessage::Assistant {
                content,
                tool_calls,
                ..
            } => {
                self.replay_assistant_content(content);
                for tool_call in tool_calls {
                    self.remember_replay_plan_snapshot(tool_call);
                    self.apply_agent_event(&AgentEvent::ToolExecutionStarted {
                        turn: 0,
                        id: tool_call.id.to_string(),
                        name: tool_call.name.to_string(),
                        arguments: serde_json::from_str(&tool_call.raw_arguments)
                            .unwrap_or_default(),
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
                let details = self.replay_tool_result_details(tool_name.as_ref());
                self.apply_agent_event(&AgentEvent::ToolExecutionFinished {
                    turn: 0,
                    id: tool_call_id.to_string(),
                    name: tool_name.to_string(),
                    result: neo_agent_core::ToolResult {
                        content: text,
                        is_error: *is_error,
                        details,
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
                    command.to_string(),
                    stdout.to_string(),
                    stderr.to_string(),
                    *exit_code,
                    None,
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
            self.push_transcript(TranscriptEntry::thinking_complete(
                thinking_text.to_string(),
            ));
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
                    Some(format!("[image blob {sha256}]")),
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
        for index in 0..self.transcript.entries().len() {
            self.transcript
                .mutate_entry(index, |entry| entry.set_expanded(expanded));
        }
        self.mark_dirty();
    }

    pub fn toggle_tool_output_expanded(&mut self) -> bool {
        if !self
            .transcript
            .entries()
            .iter()
            .any(TranscriptEntry::is_expandable)
        {
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

    /// Whether the transcript has pending changes requiring a re-render.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        if self.width == width && self.height == height {
            return;
        }
        if self.width != width {
            self.body_cache = None;
        }
        self.width = width;
        self.height = height;
        self.dirty = true;
    }

    #[must_use]
    pub const fn is_dirty_for_test(&self) -> bool {
        self.dirty
    }

    pub fn advance_animation_at_ms(&mut self, now_ms: u64) {
        let has_visible_animation = self.has_visible_animation();
        if !has_visible_animation {
            return;
        }
        self.activity_frame = self.activity_frame.wrapping_add(1);
        if self.transcript.tick_live_entries(now_ms) || has_visible_animation {
            self.mark_dirty();
        }
    }

    #[must_use]
    pub(crate) fn has_live_entries(&self) -> bool {
        self.transcript.has_live_entries()
    }

    #[must_use]
    pub(crate) fn has_visible_animation(&self) -> bool {
        self.transcript
            .entries()
            .iter()
            .any(TranscriptEntry::has_visible_animation)
    }

    /// Render a single flat frame of all non-chrome content lines as ANSI
    /// strings.
    ///
    /// The chrome (prompt box + footer) depends on [`NeoChromeState`] state and is
    /// appended by the caller via [`render_chrome_lines`]. The terminal path
    /// partitions this canonical snapshot into immutable history and a bounded
    /// mutable live surface.
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

    #[must_use]
    pub fn render_terminal_update(
        &mut self,
        width: usize,
        height: usize,
    ) -> TranscriptTerminalUpdate {
        self.width = width;
        self.height = height;
        let lines = self.render_body_lines(width);
        self.last_frame.clone_from(&lines);
        self.dirty = false;

        let content_width = super::chrome_render::frame_content_width(width);
        self.presentation.render(
            &mut self.transcript,
            content_width,
            &self.theme,
            self.activity_frame,
            self.image_render_policy,
            self.image_capabilities,
            height.saturating_sub(self.live_chrome_height),
        )
    }

    #[must_use]
    pub fn has_committed_expandable_entries(&self) -> bool {
        self.transcript
            .entries()
            .iter()
            .enumerate()
            .any(|(index, entry)| {
                entry.is_expandable()
                    && self
                        .transcript
                        .entry_ids()
                        .get(index)
                        .is_some_and(|id| self.presentation.is_committed(*id))
            })
    }

    #[must_use]
    pub fn render_browser_rows(
        &mut self,
        state: &mut TranscriptBrowserState,
        width: usize,
        height: usize,
    ) -> Vec<String> {
        let mut snapshot = self.clone();
        snapshot.width = width;
        snapshot.height = height;
        snapshot.set_tool_output_expanded(state.expanded());
        let rows = snapshot.render_body_lines(width);
        state.viewport.sync(rows.len(), height);
        let range = state.viewport.visible_row_range(rows.len(), height);
        self.dirty = false;
        rows[range].to_vec()
    }

    pub fn acknowledge_history(&mut self, blocks: &[FinalizedBlock]) {
        self.presentation.acknowledge(blocks);
    }

    pub fn finalize_interrupted_live_entries(&mut self) -> bool {
        let mut changed = false;
        if !self.queued_approvals.is_empty() {
            self.queued_approvals.clear();
            changed = true;
        }
        for index in 0..self.transcript.entries().len() {
            changed |= self.transcript.mutate_entry(index, |entry| {
                let TranscriptEntry::ApprovalPrompt(data) = entry else {
                    return false;
                };
                if data.queued_count == 0 {
                    return false;
                }
                data.queued_count = 0;
                true
            });
        }
        changed |= self.transcript.finalize_interrupted_live_entries();
        if changed {
            self.mark_dirty();
        }
        changed
    }

    /// Build the non-chrome body lines without consuming the dirty flag.
    /// Shared between [`render_frame`] (live path) and [`frame_ansi_lines`]
    /// (read-only snapshot for tests).
    ///
    fn render_body_lines(&mut self, width: usize) -> Vec<String> {
        let content_width = super::chrome_render::frame_content_width(width);
        self.render_transcript_ansi_rows(content_width)
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
        if self.transcript.tool(id).is_some_and(|tool| {
            tool.finalization() == Finalization::Finalized
                && matches!(status, ToolStatusKind::Pending | ToolStatusKind::Running)
        }) {
            return;
        }
        if self.transcript.has_tool(id) {
            self.transcript.mutate_tool(id, |tool| {
                tool.update_call_state(name.clone(), arguments.clone(), status)
            });
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
        if let Some(workspace_root) = &self.workspace_root {
            component.set_workspace_dir(workspace_root.clone());
        }
        let entry = self.apply_expand_state_to_entry(TranscriptEntry::tool_run(component));
        self.transcript.push(entry);
    }

    pub(super) fn remember_tool_call(&mut self, turn: u32, id: &str, name: &str) {
        self.tool_call_metadata
            .insert(id.to_owned(), (turn, name.to_owned()));
        if let Some(kind) = AbsorbedToolKind::from_tool_name(name)
            && self.should_suppress_delegate_tool_run(turn, kind, id)
        {
            self.transcript.suppress_tool_run(id);
        }
    }

    pub(super) fn suppress_delegate_tool_runs_for_turn(
        &mut self,
        turn: u32,
        kind: AbsorbedToolKind,
    ) {
        let ids = self
            .tool_call_metadata
            .iter()
            .filter(|&(_id, (tool_turn, tool_name))| {
                *tool_turn == turn && tool_name == kind.tool_name()
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in ids {
            if self.should_suppress_delegate_tool_run(turn, kind, &id) {
                self.transcript.suppress_tool_run(&id);
            }
        }
    }

    pub(super) fn mark_unfinished_tools_for_turn(
        &mut self,
        turn: u32,
        status: ToolStatusKind,
        result: &str,
    ) {
        let ids = self
            .tool_call_metadata
            .iter()
            .filter(|&(_id, (tool_turn, _tool_name))| *tool_turn == turn)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        let mut changed = false;
        for id in ids {
            let should_finish = self.transcript.tool(&id).is_some_and(|tool| {
                matches!(
                    tool.status(),
                    ToolStatusKind::Pending | ToolStatusKind::Running
                )
            });
            if !should_finish {
                continue;
            }
            changed |= self.transcript.mutate_tool(&id, |tool| {
                tool.set_terminal_status(status, Some(result.to_owned()))
            });
        }
        if changed {
            self.mark_dirty();
        }
    }

    pub(super) fn record_delegate_absorption_target(
        &mut self,
        turn: u32,
        kind: AbsorbedToolKind,
        target_id: &str,
    ) {
        self.delegate_absorption_targets
            .entry((turn, kind))
            .or_default()
            .insert(target_id.to_owned());
        self.suppress_delegate_tool_runs_for_turn(turn, kind);
    }

    pub(super) fn reconcile_delegate_tool_result(
        &mut self,
        turn: u32,
        id: &str,
        name: &str,
        is_error: bool,
        details: Option<&serde_json::Value>,
    ) {
        if is_error {
            self.transcript.unsuppress_tool_run(id);
            return;
        }
        let Some(kind) = AbsorbedToolKind::from_tool_name(name) else {
            return;
        };
        let Some(targets) = self.delegate_absorption_targets.get(&(turn, kind)) else {
            self.transcript.unsuppress_tool_run(id);
            return;
        };
        let Some(details) = details else {
            self.transcript.unsuppress_tool_run(id);
            return;
        };
        if kind.details_match_target(details, targets) {
            self.transcript.suppress_tool_run(id);
        } else {
            self.transcript.unsuppress_tool_run(id);
        }
    }

    fn should_suppress_delegate_tool_run(
        &self,
        turn: u32,
        kind: AbsorbedToolKind,
        id: &str,
    ) -> bool {
        let Some(targets) = self.delegate_absorption_targets.get(&(turn, kind)) else {
            return false;
        };
        let Some(tool) = self
            .transcript
            .entries()
            .iter()
            .find_map(|entry| match entry {
                TranscriptEntry::ToolRun { component } if component.id() == id => Some(component),
                _ => None,
            })
        else {
            return self.has_absorption_target_for_each_tool_call(turn, kind, targets);
        };
        match tool.status() {
            ToolStatusKind::Pending | ToolStatusKind::Running => {
                self.has_absorption_target_for_each_tool_call(turn, kind, targets)
            }
            ToolStatusKind::Succeeded => tool
                .state()
                .details
                .as_ref()
                .is_some_and(|details| kind.details_match_target(details, targets)),
            ToolStatusKind::Failed | ToolStatusKind::Cancelled => false,
        }
    }

    fn has_absorption_target_for_each_tool_call(
        &self,
        turn: u32,
        kind: AbsorbedToolKind,
        targets: &BTreeSet<String>,
    ) -> bool {
        let tool_call_count = self
            .tool_call_metadata
            .values()
            .filter(|(tool_turn, tool_name)| *tool_turn == turn && tool_name == kind.tool_name())
            .count();
        tool_call_count > 0 && targets.len() >= tool_call_count
    }

    pub(super) fn apply_expand_state_to_entry(
        &self,
        mut entry: TranscriptEntry,
    ) -> TranscriptEntry {
        entry.set_expanded(self.tool_output_expanded);
        entry
    }

    pub(super) fn apply_expand_state_to_active_thinking(&mut self) {
        let Some(index) = self
            .transcript
            .entries()
            .iter()
            .rposition(|entry| matches!(entry, TranscriptEntry::ThinkingBlock { .. }))
        else {
            return;
        };
        self.transcript
            .mutate_entry(index, |entry| entry.set_expanded(self.tool_output_expanded));
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
        if let Some(index) = self
            .transcript
            .entries()
            .iter()
            .rposition(is_live_compaction_entry)
        {
            self.transcript.mutate_entry(index, |entry| {
                let TranscriptEntry::Compaction {
                    phase: existing_phase,
                    percent: existing_percent,
                    compacted_message_count: existing_count,
                    tokens_before: existing_tokens,
                    tokens_after: existing_tokens_after,
                } = entry
                else {
                    return false;
                };
                if *existing_phase == phase
                    && *existing_percent == percent
                    && *existing_count == compacted_message_count
                    && *existing_tokens == tokens_before
                    && *existing_tokens_after == tokens_after
                {
                    return false;
                }
                *existing_phase = phase;
                *existing_percent = percent;
                *existing_count = compacted_message_count;
                *existing_tokens = tokens_before;
                *existing_tokens_after = tokens_after;
                true
            });
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
        if let Some(index) = self
            .transcript
            .entries()
            .iter()
            .rposition(is_live_compaction_entry)
        {
            self.transcript.mutate_entry(index, |entry| {
                let TranscriptEntry::Compaction {
                    phase: existing_phase,
                    percent: existing_percent,
                    ..
                } = entry
                else {
                    return false;
                };
                if *existing_phase == Some(phase) && *existing_percent == percent {
                    return false;
                }
                *existing_phase = Some(phase);
                *existing_percent = percent;
                true
            });
        } else {
            self.upsert_compaction(Some(phase), percent, 0, 0, 0);
            return;
        }
        self.mark_dirty();
    }

    fn render_transcript_ansi_rows(&mut self, width: usize) -> Vec<String> {
        self.transcript.ensure_cache_width(width);

        let entry_count = self.transcript.entries().len();
        let (mut rows, start_index, mut entry_row_starts, _reused_prefix_rows) =
            self.cached_render_prefix(width, entry_count);
        #[cfg(test)]
        #[allow(clippy::used_underscore_binding)]
        {
            self.last_reused_prefix_rows = _reused_prefix_rows;
        }
        let mut tool_run: Vec<ToolCallComponent> = Vec::new();

        for (index, row_start) in entry_row_starts
            .iter_mut()
            .enumerate()
            .take(entry_count)
            .skip(start_index)
        {
            *row_start = rows.len();
            // Extract whether this is a ToolRun (and its id) in a short-lived
            // borrow scope so we can freely call &mut self methods afterward.
            let tool_run_id: Option<String> = match self.transcript.entries().get(index) {
                Some(TranscriptEntry::ToolRun { component }) => Some(component.id().to_owned()),
                _ => None,
            };

            if let Some(id) = tool_run_id {
                if self.transcript.is_tool_run_suppressed(&id) {
                    append_line_transcript_block(
                        &mut rows,
                        self.flush_tool_run(&mut tool_run, width),
                    );
                } else if let Some(TranscriptEntry::ToolRun { component }) =
                    self.transcript.entries().get(index)
                {
                    tool_run.push(component.clone());
                }
            } else {
                append_line_transcript_block(&mut rows, self.flush_tool_run(&mut tool_run, width));
                let lines = self.transcript.render_entry_ansi_cached(
                    index,
                    width,
                    &self.theme,
                    self.activity_frame,
                    self.image_render_policy,
                    self.image_capabilities,
                );
                append_ansi_transcript_block(&mut rows, lines);
            }
        }
        append_line_transcript_block(&mut rows, self.flush_tool_run(&mut tool_run, width));
        entry_row_starts[entry_count] = rows.len();

        let viewport_rows = self.height.saturating_sub(self.live_chrome_height).max(1);
        self.transcript
            .viewport_mut()
            .sync(rows.len(), viewport_rows);
        self.transcript.clear_dirty_entries();
        self.body_cache = Some(TranscriptBodyCache {
            width,
            entry_count,
            rows: rows.clone(),
            entry_row_starts,
        });
        rows
    }

    fn cached_render_prefix(
        &self,
        width: usize,
        entry_count: usize,
    ) -> (Vec<String>, usize, Vec<usize>, usize) {
        let Some(cache) = &self.body_cache else {
            return (Vec::new(), 0, vec![0; entry_count + 1], 0);
        };
        if cache.width != width
            || cache.entry_count > entry_count
            || cache.entry_row_starts.len() != cache.entry_count + 1
        {
            return (Vec::new(), 0, vec![0; entry_count + 1], 0);
        }
        let dirty_start = self.transcript.first_dirty_entry().unwrap_or(0);
        let start_index = self.safe_render_start(dirty_start.min(entry_count));
        let Some(prefix_rows) = cache.entry_row_starts.get(start_index).copied() else {
            return (Vec::new(), 0, vec![0; entry_count + 1], 0);
        };
        let prefix_rows = prefix_rows.min(cache.rows.len());
        let mut entry_row_starts = vec![0; entry_count + 1];
        let copied_starts = (start_index + 1).min(cache.entry_row_starts.len());
        entry_row_starts[..copied_starts].copy_from_slice(&cache.entry_row_starts[..copied_starts]);
        (
            cache.rows[..prefix_rows].to_vec(),
            start_index,
            entry_row_starts,
            prefix_rows,
        )
    }

    fn safe_render_start(&self, dirty_start: usize) -> usize {
        let entries = self.transcript.entries();
        let mut start = dirty_start.min(entries.len());
        while start > 0 {
            match entries.get(start - 1) {
                Some(TranscriptEntry::ToolRun { component })
                    if !self.transcript.is_tool_run_suppressed(component.id()) =>
                {
                    start -= 1;
                }
                _ => break,
            }
        }
        start
    }

    fn flush_tool_run(&mut self, tool_run: &mut Vec<ToolCallComponent>, width: usize) -> Vec<Line> {
        if tool_run.is_empty() {
            return Vec::new();
        }
        let mut ordered = std::mem::take(tool_run);
        super::chrome_render::render_ordered_tools(&mut ordered, width, &self.theme).lines
    }

    #[cfg(test)]
    fn cached_prefix_rows_reused_for_test(&self) -> usize {
        self.last_reused_prefix_rows
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReplayPlanSnapshot {
    path: String,
    content: Option<String>,
}

impl TranscriptPane {
    fn remember_replay_plan_snapshot(&mut self, tool_call: &neo_agent_core::AgentToolCall) {
        if !matches!(tool_call.name.as_ref(), "Write" | "Edit") {
            return;
        }
        let Ok(arguments) = serde_json::from_str::<serde_json::Value>(&tool_call.raw_arguments)
        else {
            return;
        };
        let Some(path) = arguments.get("path").and_then(serde_json::Value::as_str) else {
            return;
        };
        if !looks_like_plan_file_path(path) {
            return;
        }
        let content = arguments
            .get("content")
            .or_else(|| arguments.get("new"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        self.replay_plan_snapshot = Some(ReplayPlanSnapshot {
            path: path.to_owned(),
            content,
        });
    }

    fn replay_tool_result_details(&self, tool_name: &str) -> Option<serde_json::Value> {
        if tool_name != "ExitPlanMode" {
            return None;
        }
        let snapshot = self.replay_plan_snapshot.as_ref()?;
        let content = std::fs::read_to_string(&snapshot.path)
            .ok()
            .or_else(|| snapshot.content.clone())?;
        Some(serde_json::json!({
            "plan_content": content,
            "plan_path": snapshot.path,
        }))
    }
}

fn looks_like_plan_file_path(path: &str) -> bool {
    let path = Path::new(path);
    if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
        return false;
    }
    let segments = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>();
    segments
        .windows(3)
        .any(|window| window == ["agents", "main", "plans"])
}

fn append_line_transcript_block(rows: &mut Vec<String>, block: Vec<Line>) {
    let first = block.iter().position(|line| !line.is_blank());
    let last = block.iter().rposition(|line| !line.is_blank());
    let (Some(first), Some(last)) = (first, last) else {
        return;
    };
    if rows.last().is_some_and(|line| !ansi_line_is_blank(line)) {
        rows.push(String::new());
    }
    rows.extend(
        block
            .into_iter()
            .skip(first)
            .take(last - first + 1)
            .map(|line| line.to_ansi()),
    );
}

fn append_ansi_transcript_block(rows: &mut Vec<String>, mut block: Vec<String>) {
    trim_ansi_transcript_block(&mut block);
    if block.is_empty() {
        return;
    }
    if rows.last().is_some_and(|line| !ansi_line_is_blank(line)) {
        rows.push(String::new());
    }
    rows.extend(block);
}

pub(super) fn trim_ansi_transcript_block(block: &mut Vec<String>) {
    let first = block.iter().position(|line| !ansi_line_is_blank(line));
    let last = if block.iter().any(|line| ansi_line_is_image(line)) {
        block.len().checked_sub(1)
    } else {
        block.iter().rposition(|line| !ansi_line_is_blank(line))
    };
    let (Some(first), Some(last)) = (first, last) else {
        block.clear();
        return;
    };
    block.truncate(last + 1);
    block.drain(..first);
}

fn ansi_line_is_blank(line: &str) -> bool {
    if ansi_line_is_image(line) {
        return false;
    }
    let mut index = 0;
    while index < line.len() {
        if let Some(sequence) = next_sequence(line, index) {
            index += sequence.len();
            continue;
        }
        let Some(character) = line[index..].chars().next() else {
            break;
        };
        if !character.is_whitespace() {
            return false;
        }
        index += character.len_utf8();
    }
    true
}

pub(super) fn ansi_line_is_image(line: &str) -> bool {
    line.contains("\x1b_G") || line.contains("\x1b]1337;File=")
}

fn content_display_text(content: &[Content]) -> String {
    content.iter().filter_map(content_visible_text).collect()
}

fn user_content_display(
    content: &[Content],
) -> (String, Vec<crate::transcript::TranscriptImageAttachment>) {
    let mut image_index = 0;
    let mut text = String::new();
    let mut images = Vec::new();
    for part in content {
        match part {
            Content::Text { text: part_text } => text.push_str(part_text),
            Content::Thinking { .. } => {}
            Content::Image { mime_type, data } => {
                image_index += 1;
                if let Some(image) =
                    transcript_attachment_from_content_image(image_index, mime_type, data)
                {
                    text.push_str(&image.placeholder);
                    images.push(image);
                } else {
                    text.push_str(&image_summary(mime_type, data));
                }
            }
        }
    }
    (text, images)
}

fn transcript_attachment_from_content_image(
    image_index: usize,
    mime_type: &str,
    data: &ImageRef,
) -> Option<crate::transcript::TranscriptImageAttachment> {
    let ImageRef::Base64(encoded) = data else {
        return None;
    };
    let bytes = decode_base64(encoded)?;
    let (width, height) = crate::terminal_image::detect_image_dimensions(&bytes, mime_type)?;
    let placeholder = format!("[image #{image_index} ({width}x{height})]");
    Some(crate::transcript::TranscriptImageAttachment::new(
        format!("image-{image_index}"),
        mime_type.to_owned(),
        width,
        height,
        placeholder,
        bytes,
    ))
}

fn content_visible_text(content: &Content) -> Option<String> {
    match content {
        Content::Text { text } => Some(text.to_string()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_compaction_after_completed_card_appends_entry() {
        let mut pane = TranscriptPane::new(80, 20);
        pane.upsert_compaction(
            Some(neo_agent_core::CompactionPhase::Applying),
            100,
            13,
            42_000,
            9_000,
        );
        pane.push_transcript(TranscriptEntry::assistant_message(
            "tool transcript after compact",
        ));

        pane.upsert_compaction(
            Some(neo_agent_core::CompactionPhase::Estimating),
            0,
            23,
            51_000,
            0,
        );
        pane.update_compaction_progress(neo_agent_core::CompactionPhase::Summarizing, 84);

        let entries = pane.transcript().entries();
        assert_eq!(
            entries.len(),
            3,
            "new compaction should not rewrite the prior completed card"
        );
        assert!(
            matches!(
                &entries[0],
                TranscriptEntry::Compaction {
                    phase: Some(neo_agent_core::CompactionPhase::Applying),
                    percent: 100,
                    compacted_message_count: 13,
                    tokens_before: 42_000,
                    tokens_after: 9_000,
                }
            ),
            "completed card should stay intact"
        );
        assert!(
            matches!(
                &entries[2],
                TranscriptEntry::Compaction {
                    phase: Some(neo_agent_core::CompactionPhase::Summarizing),
                    percent: 84,
                    compacted_message_count: 23,
                    tokens_before: 51_000,
                    tokens_after: 0,
                }
            ),
            "latest card should carry the new compaction progress"
        );
    }

    #[test]
    fn append_only_render_reuses_cached_body_prefix() {
        let mut pane = TranscriptPane::new(80, 20);
        pane.push_transcript(TranscriptEntry::assistant_message("first"));
        let first = pane.render_frame(80, 20).expect("first render");
        assert!(first.iter().any(|line| line.contains("first")));

        pane.push_transcript(TranscriptEntry::assistant_message("second"));
        let second = pane.render_frame(80, 20).expect("second render");

        assert!(second.iter().any(|line| line.contains("first")));
        assert!(second.iter().any(|line| line.contains("second")));
        assert!(
            pane.cached_prefix_rows_reused_for_test() > 0,
            "append-only render should reuse stable prefix rows"
        );
    }
}
