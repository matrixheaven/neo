use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use neo_agent_core::instructions::InstructionEpochData;
use neo_agent_core::{AgentEvent, AgentMessage, Content, MediaRef, skills::SkillStore};

use crate::dialogs::question_dialog::{QuestionDisplayData, QuestionStateMachine};
use crate::primitive::theme::TuiTheme;
use crate::primitive::{Finalization, next_sequence, strip_ansi};
use crate::shell::{StreamUpdate, ToolStatusKind};
use crate::terminal_image::{
    ImageRenderPolicy, ImageSource, InlineImage, TerminalImageCapabilities,
};
use crate::transcript::store::EntryRenderParams;
use crate::transcript::{
    DocumentLayout, McpStartupStatusData, QuestionPromptData, QuestionPromptState,
    ShellRunComponent, ToolCallComponent, ToolCallState, TranscriptEntry, TranscriptEntryId,
    TranscriptStore,
};

use super::entry::RetryStatusData;
use super::selection::{
    AutoScroll, DocumentPoint, DocumentSelection, MouseEvent, MouseKind, cell_to_grapheme_index,
    grapheme_index_to_cell, paint_selection_range, slice_text_by_cells, word_span_in_text,
};

/// Clamp a body coordinate into the u16 mouse coordinate space. Body rows and
/// columns come from terminal geometry and are already u16-bounded in
/// practice; the clamp keeps the threshold arithmetic total.
fn clamp_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

fn compaction_is_complete(phase: Option<neo_agent_core::CompactionPhase>, percent: u8) -> bool {
    phase == Some(neo_agent_core::CompactionPhase::Applying) && percent >= 100
}

const COMPACTION_PROGRESS_TICK_MS: u64 = 250;
const COMPACTION_MAX_STEP_PER_TICK: u8 = 1;
const COMPACTION_TAU_ESTIMATING_MS: u64 = 1_000;
const COMPACTION_TAU_SELECTING_MS: u64 = 1_500;
const COMPACTION_TAU_SUMMARIZING_MS: u64 = 30_000;
const COMPACTION_TAU_APPLYING_MS: u64 = 1_000;

#[derive(Debug, Clone, Copy)]
struct CompactionDisplayState {
    phase: neo_agent_core::CompactionPhase,
    confirmed_percent: u8,
    display_percent: u8,
    phase_started_at_ms: u64,
    last_update_at_ms: u64,
}

const fn compaction_phase_rank(phase: neo_agent_core::CompactionPhase) -> u8 {
    match phase {
        neo_agent_core::CompactionPhase::Estimating => 0,
        neo_agent_core::CompactionPhase::SelectingBoundary => 1,
        neo_agent_core::CompactionPhase::Summarizing => 2,
        neo_agent_core::CompactionPhase::Applying => 3,
    }
}

const fn compaction_phase_bounds(
    phase: neo_agent_core::CompactionPhase,
    confirmed_percent: u8,
) -> (u8, u8, u64) {
    match phase {
        neo_agent_core::CompactionPhase::Estimating => (0, 10, COMPACTION_TAU_ESTIMATING_MS),
        neo_agent_core::CompactionPhase::SelectingBoundary => (10, 20, COMPACTION_TAU_SELECTING_MS),
        neo_agent_core::CompactionPhase::Summarizing => (
            20,
            if confirmed_percent >= 85 { 85 } else { 82 },
            COMPACTION_TAU_SUMMARIZING_MS,
        ),
        neo_agent_core::CompactionPhase::Applying => (85, 99, COMPACTION_TAU_APPLYING_MS),
    }
}

fn compaction_time_target(
    phase: neo_agent_core::CompactionPhase,
    confirmed_percent: u8,
    elapsed_ms: u64,
) -> u8 {
    let (phase_start, phase_cap, tau_ms) = compaction_phase_bounds(phase, confirmed_percent);
    let elapsed_fraction = 1.0 - (-(elapsed_ms as f64) / tau_ms as f64).exp();
    let timed_target = f64::from(phase_start)
        + f64::from(phase_cap.saturating_sub(phase_start)) * elapsed_fraction;
    let timed_target = timed_target.clamp(f64::from(phase_start), f64::from(phase_cap)) as u8;
    confirmed_percent.max(timed_target).min(phase_cap)
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

/// The earliest unresolved blocking transcript entry: the interactive focus
/// that owns input until it resolves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockingEntryKind {
    Approval(String),
    Question(String),
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
pub struct TranscriptPane {
    width: usize,
    height: usize,
    pub(super) transcript: TranscriptStore,
    /// The incremental document: per-entry layout, logical anchor, and view
    /// state. The physical terminal only receives a bounded visible slice
    /// resolved against this document.
    document: DocumentLayout,
    /// Rendered blocks for entries re-rendered during the current layout
    /// refresh, so composition reuses them instead of rendering twice.
    frame_blocks: BTreeMap<usize, Vec<String>>,
    dirty: bool,
    tool_output_expanded: bool,
    pub(super) streaming_tool_args: BTreeMap<String, String>,
    tool_call_metadata: BTreeMap<String, (u32, String)>,
    delegate_absorption_targets: BTreeMap<(u32, AbsorbedToolKind), BTreeSet<String>>,
    /// Tool call ids absorbed by instruction epoch cards. Their
    /// provider-valid deferred results replay through the normal finish
    /// path but must never un-suppress the placeholders.
    instruction_deferred_tool_ids: BTreeSet<String>,
    pub(super) completed_tool_result_ids: Vec<String>,
    next_image_id: u64,
    activity_frame: usize,
    compaction_display: Option<CompactionDisplayState>,
    workspace_root: Option<PathBuf>,
    neo_home: Option<PathBuf>,
    /// Cache of the last composed body frame (ANSI strings, no chrome), so
    /// tests can inspect rendered output via [`frame_ansi_lines`] without
    /// recomposing unchanged rows.
    last_frame: Vec<String>,
    #[cfg(test)]
    last_reused_prefix_rows: usize,
    /// Theme used to color the live transcript body. Mirrors [`NeoChromeState`]'s
    /// theme; kept here (rather than borrowed) so the runtime can render
    /// without holding a reference to the app. The interactive mode keeps it
    /// in sync via [`Self::set_theme`].
    theme: TuiTheme,
    image_render_policy: ImageRenderPolicy,
    image_capabilities: TerminalImageCapabilities,
    pub(super) skill_store: Option<SkillStore>,
    /// Document-coordinate text selection: endpoints, drag lifecycle, word
    /// selection, auto-scroll intent, and materialized plain text.
    selection: DocumentSelection,
    /// Body height (rows) of the last rendered visible slice, used to map
    /// mouse rows into document rows and to detect viewport-edge drags.
    body_height: usize,
    /// Pointer column of the most recent drag event, carried into frame-
    /// driven auto-scroll so the active endpoint keeps the pointer's column.
    last_drag_col: usize,
    /// Entry index range visible in the last resolved document layout.
    /// `None` until the first layout resolution: animation scheduling then
    /// falls back to scanning every entry, so frames are never scheduled
    /// before the viewport exists.
    visible_entries: Option<std::ops::Range<usize>>,
}

impl TranscriptPane {
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            transcript: TranscriptStore::new(),
            dirty: false,
            tool_output_expanded: false,
            streaming_tool_args: BTreeMap::new(),
            tool_call_metadata: BTreeMap::new(),
            delegate_absorption_targets: BTreeMap::new(),
            instruction_deferred_tool_ids: BTreeSet::new(),
            completed_tool_result_ids: Vec::new(),
            next_image_id: 0,
            activity_frame: 0,
            compaction_display: None,
            workspace_root: None,
            neo_home: None,
            last_frame: Vec::new(),
            document: DocumentLayout::new(),
            frame_blocks: BTreeMap::new(),
            #[cfg(test)]
            last_reused_prefix_rows: 0,
            theme: TuiTheme::default(),
            image_render_policy: ImageRenderPolicy::default(),
            image_capabilities: TerminalImageCapabilities::default(),
            skill_store: None,
            selection: DocumentSelection::new(),
            body_height: 0,
            last_drag_col: 0,
            visible_entries: None,
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
        self.document.rebuild();
        self.transcript.invalidate_render_cache();
        self.mark_dirty();
    }

    pub fn set_image_render_policy(&mut self, policy: ImageRenderPolicy) {
        if self.image_render_policy == policy {
            return;
        }
        self.image_render_policy = policy;
        self.document.rebuild();
        self.transcript.invalidate_render_cache();
        self.mark_dirty();
    }

    pub fn set_image_capabilities(&mut self, capabilities: TerminalImageCapabilities) {
        if self.image_capabilities == capabilities {
            return;
        }
        self.image_capabilities = capabilities;
        self.document.rebuild();
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

    pub fn set_neo_home(&mut self, neo_home: Option<PathBuf>) {
        self.neo_home = neo_home;
    }

    /// Point the pane at the active session directory so expanded Workflow
    /// direct tools can read their complete output artifacts. Session-local
    /// wiring only; `None` keeps expansion honest ("not captured").
    pub fn set_session_directory(&mut self, session_dir: Option<PathBuf>) {
        self.transcript.set_session_directory(session_dir);
        self.mark_dirty();
    }

    /// Toggle inline expansion for one Workflow direct tool, keyed by its
    /// typed tool ID. Expanding a tool collapses any other; toggling the same
    /// ID again restores the one-line row. Entry-local view state, never
    /// persisted.
    pub fn toggle_workflow_direct_tool_expansion(&mut self, tool_id: &str) -> bool {
        let Some(index) = self.transcript.entries().iter().position(|entry| {
            matches!(entry, TranscriptEntry::Workflow { component }
                if component.direct_tools().iter().any(|tool| tool.id() == tool_id))
        }) else {
            return false;
        };
        let changed = self.transcript.mutate_entry(index, |entry| {
            let TranscriptEntry::Workflow { component } = entry else {
                return false;
            };
            component.toggle_direct_tool_expansion(tool_id)
        });
        if changed {
            self.mark_dirty();
        }
        changed
    }

    #[must_use]
    pub fn neo_home(&self) -> Option<&Path> {
        self.neo_home.as_deref()
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
            AgentMessage::User {
                content,
                display_text,
                ..
            } => {
                let (content_text, images) = user_content_display(content);
                let text = display_text.as_deref().map_or(content_text, str::to_owned);
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
                    self.apply_agent_event(&AgentEvent::ToolExecutionStarted {
                        turn: 0,
                        id: tool_call.id.to_string(),
                        name: tool_call.name.to_string(),
                        arguments: serde_json::from_str(&tool_call.raw_arguments)
                            .unwrap_or_default(),
                        workflow_origin: None,
                        output_ref: None,
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
                // Plan (and other) result details are only available from
                // persisted `ToolExecutionFinished` events. Aggregate
                // `ToolResult` messages do not carry details, and must not
                // fabricate them from Write/Edit args or the live workspace.
                self.apply_agent_event(&AgentEvent::ToolExecutionFinished {
                    turn: 0,
                    id: tool_call_id.to_string(),
                    name: tool_name.to_string(),
                    result: neo_agent_core::ToolResult {
                        content: text,
                        media: Vec::new(),
                        is_error: *is_error,
                        details: None,
                        terminate: false,
                    },
                    workflow_origin: None,
                    output_ref: None,
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
                let mut shell_run = ShellRunComponent::running(id, command.to_string());
                shell_run.finish(
                    stdout.to_string(),
                    stderr.to_string(),
                    *exit_code,
                    None,
                    outcome.clone(),
                    *truncated,
                );
                self.push_transcript(TranscriptEntry::shell_run(shell_run));
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
                Content::Video { mime_type, data } => {
                    // Videos are not rendered as inline images; show a stable
                    // text summary instead.
                    text.push_str(&media_summary("video", mime_type, data));
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
            kind,
            id,
        } = part
        else {
            return;
        };
        self.flush_replayed_assistant_text(text);
        if thinking_text.is_empty() && !*redacted && id.is_none() {
            return;
        }
        self.transcript.start_thinking_with_kind_and_id(
            *kind,
            id.as_ref().map(std::string::ToString::to_string),
        );
        self.transcript.append_thinking_delta(thinking_text);
        self.transcript.finish_thinking(*redacted);
        self.apply_expand_state_to_active_thinking();
        self.mark_dirty();
    }

    fn flush_replayed_assistant_text(&mut self, text: &mut String) {
        if !text.is_empty() {
            self.replay_assistant_message(std::mem::take(text));
        }
    }

    pub fn push_image(&mut self, mime_type: &str, data: &MediaRef) {
        self.next_image_id = self.next_image_id.saturating_add(1);
        let id = format!("image-{}", self.next_image_id);
        let entry = match data {
            MediaRef::Base64(encoded) => {
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
            MediaRef::Url(url) => {
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
            MediaRef::Blob(sha256) => {
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

    pub fn scroll_transcript_up(&mut self, rows: usize) {
        self.document.scroll_up(rows);
        self.mark_dirty();
    }

    pub fn scroll_transcript_down(&mut self, rows: usize) {
        self.document.scroll_down(rows);
        self.mark_dirty();
    }

    pub fn select_visible_transcript_entry(&mut self) {
        self.ensure_layout_current();
        let height = self.body_height.max(self.height);
        let range = self.document.visible_row_range(height);
        let Some(index) = self
            .document
            .layouts()
            .iter()
            .enumerate()
            .rev()
            .find(|(index, layout)| {
                self.document
                    .block_height(*index)
                    .is_some_and(|rows| rows > 0)
                    && layout.start_row < range.end
                    && layout.start_row + layout.height > range.start
            })
            .map(|(index, _)| index)
        else {
            return;
        };
        self.set_keyboard_entry_selection(index, index);
    }

    pub fn clear_transcript_selection(&mut self) {
        self.selection.clear();
    }

    pub fn extend_transcript_selection_up(&mut self, rows: usize) {
        self.ensure_layout_current();
        if self.selection.keyboard_entries().is_none() {
            self.select_visible_transcript_entry();
        }
        let Some((start_id, end_id)) = self.selection.keyboard_entries() else {
            return;
        };
        let Some((start, end)) = self.keyboard_entry_indices(start_id, end_id) else {
            return;
        };
        self.set_keyboard_entry_selection(start.saturating_sub(rows), end);
    }

    pub fn extend_transcript_selection_down(&mut self, rows: usize) {
        self.ensure_layout_current();
        if self.selection.keyboard_entries().is_none() {
            self.select_visible_transcript_entry();
        }
        let Some((start_id, end_id)) = self.selection.keyboard_entries() else {
            return;
        };
        let Some((start, end)) = self.keyboard_entry_indices(start_id, end_id) else {
            return;
        };
        let last = self.document.layouts().len().saturating_sub(1);
        self.set_keyboard_entry_selection(start, end.saturating_add(rows).min(last));
    }

    fn keyboard_entry_indices(
        &self,
        start_id: TranscriptEntryId,
        end_id: TranscriptEntryId,
    ) -> Option<(usize, usize)> {
        let layouts = self.document.layouts();
        let start = layouts
            .iter()
            .position(|layout| layout.entry_id == start_id)?;
        let end = layouts
            .iter()
            .position(|layout| layout.entry_id == end_id)?;
        Some((start.min(end), start.max(end)))
    }

    fn set_keyboard_entry_selection(&mut self, start: usize, end: usize) {
        let layouts = self.document.layouts();
        let start = start.min(layouts.len().saturating_sub(1));
        let end = end.min(layouts.len().saturating_sub(1));
        if !self
            .document
            .block_height(start)
            .is_some_and(|height| height > 0)
        {
            return;
        }
        let Some(end_height) = self.document.block_height(end).filter(|height| *height > 0) else {
            return;
        };
        self.selection.set_keyboard_entry_selection(
            DocumentPoint {
                entry_id: layouts[start].entry_id,
                row_in_entry: 0,
                display_cell: 0,
            },
            DocumentPoint {
                entry_id: layouts[end].entry_id,
                row_in_entry: end_height - 1,
                display_cell: usize::MAX,
            },
        );
    }

    #[must_use]
    pub fn has_transcript_selection(&self) -> bool {
        self.selection.is_active()
    }

    /// The materialized plain text of the current selection, frozen at the
    /// document revision where it was captured. Materializes on demand when
    /// the selection came from the keyboard instead of a mouse release.
    pub fn copy_selected_transcript_text(&mut self) -> Option<String> {
        if self.selection.materialized().is_none()
            && let Some(text) = self.materialize_selection()
        {
            self.selection.set_materialized(text);
        }
        self.selection.materialized().map(str::to_owned)
    }

    // ======================================================================
    // Mouse-driven document selection
    // ======================================================================

    /// Feed one typed mouse event with transcript-body coordinates. Wheel
    /// events are handled by the runtime (transcript-wide navigation) and are
    /// ignored here. Shift-modified drags are never consumed so terminal
    /// emulators keep native selection.
    pub fn handle_mouse_event(&mut self, event: MouseEvent, body_row: usize, body_col: usize) {
        if event.is_shift_modified() || event.is_wheel() {
            return;
        }
        match event.kind {
            MouseKind::Press if event.button == crossterm::event::MouseButton::Left => {
                self.mouse_press(body_row, body_col);
            }
            MouseKind::Drag if event.button == crossterm::event::MouseButton::Left => {
                self.mouse_drag(body_row, body_col);
            }
            MouseKind::Release => self.mouse_release(),
            _ => {}
        }
    }

    /// Whether the frame loop must keep rendering while a drag auto-scrolls.
    #[must_use]
    pub(crate) fn selection_requests_animation(&self) -> bool {
        self.selection.requests_animation()
    }

    fn mouse_press(&mut self, body_row: usize, body_col: usize) {
        self.ensure_layout_current();
        let Some(point) = self.point_at_body(body_row, body_col) else {
            return;
        };
        let double_click = self.selection.press(
            point,
            clamp_u16(body_row),
            clamp_u16(body_col),
            Instant::now(),
        );
        if double_click && let Some((start, end)) = self.word_span(point) {
            self.selection.set_word_selection(start, end);
            self.selection.invalidate_materialized();
        }
    }

    fn mouse_drag(&mut self, body_row: usize, body_col: usize) {
        if !self.selection.is_gesture_open() {
            return;
        }
        self.ensure_layout_current();
        self.last_drag_col = body_col;
        let row = clamp_u16(body_row);
        let col = clamp_u16(body_col);
        let point = self.point_at_body(body_row, body_col);
        let update = self.selection.drag(point, row, col);
        if update.started {
            // Lock the view so tail-following cannot shift the pointer-to-
            // document mapping mid-drag.
            let range = self.document.visible_row_range(self.body_height);
            if self.document.is_following_tail() {
                self.document.lock_at_row(range.start);
                self.mark_dirty();
            }
        }
        let auto_scroll = if body_row >= self.body_height {
            Some(AutoScroll::Down)
        } else if body_row == 0 {
            Some(AutoScroll::Up)
        } else {
            None
        };
        self.selection.set_auto_scroll(auto_scroll);
        if auto_scroll.is_some() {
            self.mark_dirty();
        }
    }

    fn mouse_release(&mut self) {
        if self.selection.release()
            && let Some(text) = self.materialize_selection()
        {
            self.selection.set_materialized(text);
        }
    }

    /// Advance the document one row per frame while a drag crosses the
    /// viewport edge, extending the active endpoint into the revealed row.
    /// Called from [`Self::render_visible_slice`], so auto-scroll rides the
    /// existing frame cadence and stops on mouse release (which clears the
    /// auto-scroll intent and the animation deadline request).
    fn apply_drag_autoscroll(&mut self, body_height: usize) {
        let Some(direction) = self.selection.auto_scroll() else {
            return;
        };
        if !self.selection.is_dragging() {
            return;
        }
        match direction {
            AutoScroll::Up => self.document.scroll_up(1),
            AutoScroll::Down => self.document.scroll_down(1),
        }
        let range = self.document.visible_row_range(body_height);
        let edge_row = match direction {
            AutoScroll::Up => range.start,
            AutoScroll::Down => range.end.saturating_sub(1),
        };
        if let Some(point) = self.document.point_at(edge_row, self.last_drag_col) {
            self.selection.extend_to(point);
        }
        self.mark_dirty();
    }

    /// Resolve a body position to a document point through the current
    /// visible slice. `None` outside the body or over a non-text region.
    fn point_at_body(&mut self, body_row: usize, body_col: usize) -> Option<DocumentPoint> {
        if body_row >= self.body_height {
            return None;
        }
        let range = self.document.visible_row_range(self.body_height);
        self.document
            .point_at(range.start.saturating_add(body_row), body_col)
    }

    /// The word under `point`, as a document endpoint span. Uses the entry's
    /// rendered row text so the word matches what is on screen. Image rows
    /// have no selectable word.
    fn word_span(&mut self, point: DocumentPoint) -> Option<(DocumentPoint, DocumentPoint)> {
        let content_width = super::chrome_render::frame_content_width(self.width);
        let index = self
            .transcript
            .entry_ids()
            .iter()
            .position(|id| *id == point.entry_id)?;
        let block = self.entry_block_lines(index, content_width);
        let raw = block.get(point.row_in_entry)?;
        if ansi_line_is_image(raw) {
            return None;
        }
        let text = strip_ansi(raw);
        let grapheme_index = cell_to_grapheme_index(&text, point.display_cell);
        let (start, end) = word_span_in_text(&text, grapheme_index);
        let start_cell = grapheme_index_to_cell(&text, start);
        let end_cell = grapheme_index_to_cell(&text, end);
        let mut start_point = point;
        let mut end_point = point;
        start_point.display_cell = start_cell;
        end_point.display_cell = end_cell.saturating_sub(1);
        Some((start_point, end_point))
    }

    /// Materialize the plain text between the selection endpoints against
    /// the current document revision, clamping vanished endpoints.
    fn materialize_selection(&mut self) -> Option<String> {
        self.ensure_layout_current();
        if let Some((start_id, end_id)) = self.selection.keyboard_entries() {
            let (start, end) = self.keyboard_entry_indices(start_id, end_id)?;
            let mut text = String::new();
            for (offset, entry) in self.transcript.entries()[start..=end].iter().enumerate() {
                if offset > 0 {
                    text.push_str("\n\n");
                }
                let (label, content) = entry.copy_parts();
                text.push_str(label);
                text.push('\n');
                text.push_str(&content);
            }
            return Some(text);
        }
        let (anchor, active) = (self.selection.anchor()?, self.selection.active()?);
        let anchor = self.clamp_point(anchor)?;
        let active = self.clamp_point(active)?;
        let start_row = self.document.row_of(anchor)?;
        let end_row = self.document.row_of(active)?;
        let (min_row, max_row) = (start_row.min(end_row), start_row.max(end_row));
        // Endpoint cells are symmetric: the min-end row is cut on the right
        // at the active cell when the drag ends there (upward drag), and the
        // max-end row is cut on the left at the anchor cell when the drag
        // leaves it there (upward drag). Downward drags cut the anchor row
        // on the left and the active row on the right. A single-row
        // selection spans the cells between both endpoints.
        let (min_start_cell, min_end_cell, max_start_cell, max_end_cell) = if start_row == end_row {
            let start = anchor.display_cell.min(active.display_cell);
            let end = anchor
                .display_cell
                .max(active.display_cell)
                .saturating_add(1);
            (start, end, start, end)
        } else if start_row < end_row {
            (
                anchor.display_cell,
                usize::MAX,
                0,
                active.display_cell.saturating_add(1),
            )
        } else {
            (
                0,
                active.display_cell.saturating_add(1),
                anchor.display_cell,
                usize::MAX,
            )
        };
        let content_width = super::chrome_render::frame_content_width(self.width);
        let mut text = String::new();
        let mut index = self.document.entry_at_row(min_row)?;
        let mut row = min_row;
        let mut first_line = true;
        while row <= max_row {
            let Some(layout) = self.document.entry_layout(index).copied() else {
                break;
            };
            let block_len = self.document.block_height(index).unwrap_or(0);
            let block_start = layout.start_row + layout.height.saturating_sub(block_len);
            let span_end = (layout.start_row + layout.height).min(max_row + 1);
            if span_end <= row {
                index += 1;
                continue;
            }
            let block = if block_len > 0 {
                self.entry_block_lines(index, content_width)
            } else {
                Vec::new()
            };
            let mut virtual_row = row;
            while virtual_row < span_end {
                if !first_line {
                    text.push('\n');
                }
                first_line = false;
                // Separator rows (between cards) copy as blank lines; only
                // rows inside the rendered block carry text.
                if virtual_row >= block_start {
                    let row_in_block = virtual_row - block_start;
                    if row_in_block < block_len
                        && let Some(line) = block.get(row_in_block)
                    {
                        let plain = strip_ansi(line);
                        let start_cell = if virtual_row == min_row {
                            min_start_cell
                        } else if virtual_row == max_row {
                            max_start_cell
                        } else {
                            0
                        };
                        let end_cell = if virtual_row == max_row {
                            max_end_cell
                        } else if virtual_row == min_row {
                            min_end_cell
                        } else {
                            usize::MAX
                        };
                        text.push_str(&slice_text_by_cells(&plain, start_cell, end_cell));
                    }
                }
                virtual_row += 1;
            }
            row = span_end;
            index += 1;
            if index >= self.document.layouts().len() {
                break;
            }
        }
        Some(text)
    }

    /// Clamp an endpoint against the current document: a vanished row clamps
    /// to the last surviving row of the same entry, then to the nearest
    /// preceding surviving entry, then to the first surviving entry.
    fn clamp_point(&self, point: DocumentPoint) -> Option<DocumentPoint> {
        let layouts = self.document.layouts();
        let surviving = |offset: usize| {
            self.document
                .block_height(offset)
                .filter(|height| *height > 0)
                .map(|height| DocumentPoint {
                    entry_id: layouts[offset].entry_id,
                    row_in_entry: height - 1,
                    display_cell: point.display_cell,
                })
        };
        if let Some(position) = layouts.iter().position(|l| l.entry_id == point.entry_id) {
            if let Some(height) = self.document.block_height(position).filter(|h| *h > 0) {
                return Some(DocumentPoint {
                    entry_id: point.entry_id,
                    row_in_entry: point.row_in_entry.min(height - 1),
                    display_cell: point.display_cell,
                });
            }
            for offset in (0..position).rev() {
                if let Some(point) = surviving(offset) {
                    return Some(point);
                }
            }
        } else {
            for offset in (0..layouts.len()).rev() {
                if let Some(point) = surviving(offset) {
                    return Some(point);
                }
            }
        }
        for offset in 0..layouts.len() {
            if let Some(point) = surviving(offset) {
                return Some(point);
            }
        }
        None
    }

    /// Reconcile the document layout with the store and feed fresh heights,
    /// so row resolution reflects the current document at event time.
    /// `pub(crate)` so the outer frame composition can read a current
    /// document view before rendering the body slice.
    pub(crate) fn ensure_layout_current(&mut self) {
        let content_width = super::chrome_render::frame_content_width(self.width);
        self.refresh_layout(content_width);
    }

    pub fn start_assistant_message(&mut self) {
        self.transcript.start_assistant();
        self.mark_dirty();
    }

    pub fn start_assistant_message_with_phase(&mut self, phase: neo_ai::MessagePhase) {
        self.transcript.start_assistant_with_phase(phase);
        self.mark_dirty();
    }

    pub fn append_assistant_delta(&mut self, text: &str) {
        self.transcript.finish_thinking(false);
        self.transcript.append_assistant_delta(text);
        self.mark_dirty();
    }

    pub fn finish_assistant_message(&mut self) {
        self.transcript.finish_assistant();
        self.mark_dirty();
    }

    pub fn upsert_retry_status(&mut self, data: RetryStatusData) -> bool {
        let changed = self.transcript.upsert_retry_status(data);
        if changed {
            self.mark_dirty();
        }
        changed
    }

    pub fn clear_retry_status(&mut self, turn: u32) -> bool {
        let changed = self.transcript.clear_retry_status(turn);
        if changed {
            self.mark_dirty();
        }
        changed
    }

    pub fn reset_live_model_attempt(&mut self, turn: u32) -> bool {
        let changed = self.transcript.reset_live_model_attempt(turn);
        if changed {
            self.mark_dirty();
        }
        changed
    }

    pub(super) fn interrupt_retry_status(&mut self, turn: u32) -> bool {
        let changed = self.transcript.interrupt_retry_status(turn);
        if changed {
            self.mark_dirty();
        }
        changed
    }

    pub fn set_tool_output_expanded(&mut self, expanded: bool) {
        self.tool_output_expanded = expanded;
        self.document.rebuild();
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
            self.document.set_width(width);
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
        let visible_entries = self.visible_entry_slice();
        let has_frame_animation = visible_entries.iter().any(|entry| {
            !matches!(entry, TranscriptEntry::Compaction { .. }) && entry.has_visible_animation()
        });
        self.activity_frame = self.activity_frame.wrapping_add(1);
        let compaction_changed = self.advance_compaction_display(now_ms);
        let live_entry_changed = self
            .transcript
            .tick_live_entries_in(now_ms, self.visible_entries.clone());
        if compaction_changed || live_entry_changed || has_frame_animation {
            self.mark_dirty();
        }
    }

    #[must_use]
    pub(crate) fn has_live_entries(&self) -> bool {
        match &self.visible_entries {
            Some(range) => self.transcript.has_live_entries_in(range.clone()),
            None => self.transcript.has_live_entries(),
        }
    }

    #[must_use]
    pub(crate) fn has_visible_animation(&self) -> bool {
        self.visible_entry_slice()
            .iter()
            .any(TranscriptEntry::has_visible_animation)
    }

    /// The entries currently on screen, or every entry when the document has
    /// not been laid out yet. Animation scheduling is driven exclusively by
    /// this slice: off-screen entries neither request deadlines nor tick.
    fn visible_entry_slice(&self) -> &[TranscriptEntry] {
        let entries = self.transcript.entries();
        match &self.visible_entries {
            Some(range) => entries.get(range.clone()).unwrap_or(entries),
            None => entries,
        }
    }

    /// The entry indices whose laid-out rows intersect `[start_row, end_row)`.
    fn visible_entry_indices(
        &self,
        start_row: usize,
        end_row: usize,
    ) -> Option<std::ops::Range<usize>> {
        let layouts = self.document.layouts();
        if start_row >= end_row || layouts.is_empty() {
            return None;
        }
        let start = layouts
            .iter()
            .position(|layout| layout.start_row + layout.height > start_row)?;
        let mut end = start;
        for layout in &layouts[start..] {
            if layout.start_row >= end_row {
                break;
            }
            end += 1;
        }
        Some(start..end)
    }

    /// Render a single flat frame of all non-chrome content lines as ANSI
    /// strings.
    ///
    /// The chrome (prompt box + footer) depends on [`NeoChromeState`] state and is
    /// appended by the caller via [`render_chrome_lines`].
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
        self.body_height = height;

        let lines = self.render_body_lines(width);
        self.last_frame.clone_from(&lines);
        Some(lines)
    }

    /// The transcript index of the earliest unresolved blocking entry
    /// (pending approval or question), in store order. Feeds the document's
    /// visible-window constraint; kept in lockstep with
    /// [`Self::earliest_blocking_entry`] so input focus and visible focus
    /// can never disagree.
    fn blocking_entry_index(&self) -> Option<usize> {
        self.transcript
            .entries()
            .iter()
            .position(|entry| match entry {
                TranscriptEntry::ApprovalPrompt(data) => data.is_pending(),
                TranscriptEntry::QuestionPrompt(data) => data.is_pending(),
                _ => false,
            })
    }

    /// The earliest unresolved blocking entry (approval or question) in
    /// transcript order, if any. That entry owns interactive input until it
    /// resolves; later entries can never displace it.
    #[must_use]
    pub fn earliest_blocking_entry(&self) -> Option<BlockingEntryKind> {
        let index = self.blocking_entry_index()?;
        match &self.transcript.entries()[index] {
            TranscriptEntry::ApprovalPrompt(data) => {
                Some(BlockingEntryKind::Approval(data.id().to_owned()))
            }
            TranscriptEntry::QuestionPrompt(data) => {
                Some(BlockingEntryKind::Question(data.id.clone()))
            }
            _ => None,
        }
    }

    /// Upsert one question prompt entry on arrival. The runtime chrome
    /// [`QuestionStateMachine`] stays the input/selection owner; this entry
    /// is its single visible display, synced after every input.
    pub fn upsert_question_prompt(
        &mut self,
        id: &str,
        questions: Vec<QuestionDisplayData>,
    ) -> bool {
        self.upsert_question_prompt_with_origin(id, questions, None)
    }

    pub fn apply_question_stream_update(&mut self, update: StreamUpdate) -> bool {
        let StreamUpdate::QuestionRequested {
            id,
            questions,
            workflow_origin,
        } = update
        else {
            return false;
        };
        self.upsert_question_prompt_with_origin(&id, questions, workflow_origin)
    }

    fn upsert_question_prompt_with_origin(
        &mut self,
        id: &str,
        questions: Vec<QuestionDisplayData>,
        workflow_origin: Option<neo_agent_core::workflow::WorkflowExecutionOrigin>,
    ) -> bool {
        let display = questions
            .first()
            .cloned()
            .unwrap_or_else(|| QuestionDisplayData {
                question: String::new(),
                header: None,
                body: None,
                options: Vec::new(),
                multi_select: false,
            });
        let existing_index = self.transcript.entries().iter().position(
            |entry| matches!(entry, TranscriptEntry::QuestionPrompt(data) if data.id == id),
        );
        if let Some(index) = existing_index {
            let changed = self.transcript.mutate_entry(index, |entry| {
                let TranscriptEntry::QuestionPrompt(data) = entry else {
                    return false;
                };
                let machine = QuestionStateMachine::new(id, questions);
                if !data.is_pending()
                    || (data.machine == machine && data.workflow_origin == workflow_origin)
                {
                    return false;
                }
                data.machine = machine;
                data.display = display;
                data.workflow_origin = workflow_origin;
                true
            });
            if changed {
                self.mark_dirty();
            }
            return changed;
        }
        self.push_transcript(TranscriptEntry::QuestionPrompt(QuestionPromptData {
            id: id.to_owned(),
            state: QuestionPromptState::Pending,
            workflow_origin,
            display,
            machine: QuestionStateMachine::new(id, questions),
        }));
        true
    }

    /// Refresh the pending question entry's display clone from the runtime
    /// state machine so the live card shows the current selection.
    pub fn sync_question_prompt(&mut self, machine: &QuestionStateMachine) -> bool {
        let Some(index) = self.transcript.entries().iter().position(|entry| {
            matches!(entry, TranscriptEntry::QuestionPrompt(data)
                if data.id == machine.id && data.is_pending())
        }) else {
            return false;
        };
        let changed = self.transcript.mutate_entry(index, |entry| {
            let TranscriptEntry::QuestionPrompt(data) = entry else {
                return false;
            };
            if data.machine == *machine {
                return false;
            }
            data.machine = machine.clone();
            true
        });
        if changed {
            self.mark_dirty();
        }
        changed
    }

    /// Mark one question prompt answered in place; it commits as one terminal
    /// transcript fact.
    pub fn resolve_question_prompt(&mut self, id: &str, answers: Vec<String>) -> bool {
        let Some(index) = self.transcript.entries().iter().position(|entry| {
            matches!(entry, TranscriptEntry::QuestionPrompt(data)
                if data.id == id && data.is_pending())
        }) else {
            return false;
        };
        let changed = self.transcript.mutate_entry(index, |entry| {
            let TranscriptEntry::QuestionPrompt(data) = entry else {
                return false;
            };
            data.state = QuestionPromptState::Answered { answers };
            true
        });
        if changed {
            self.mark_dirty();
        }
        changed
    }

    /// Mark one question prompt cancelled in place.
    pub fn cancel_question_prompt(&mut self, id: &str) -> bool {
        let Some(index) = self.transcript.entries().iter().position(|entry| {
            matches!(entry, TranscriptEntry::QuestionPrompt(data)
                if data.id == id && data.is_pending())
        }) else {
            return false;
        };
        let changed = self.transcript.mutate_entry(index, |entry| {
            let TranscriptEntry::QuestionPrompt(data) = entry else {
                return false;
            };
            data.state = QuestionPromptState::Cancelled;
            true
        });
        if changed {
            self.mark_dirty();
        }
        changed
    }

    pub fn finalize_cancelled_live_model_attempt(&mut self) {
        let Some(turn) = self.transcript.live_model_attempt_turn() else {
            return;
        };
        self.apply_agent_event(neo_agent_core::AgentEvent::TurnFinished {
            turn,
            stop_reason: neo_agent_core::StopReason::Cancelled,
        });
    }

    pub fn finalize_interrupted_live_entries(&mut self) -> bool {
        let changed = self.transcript.finalize_interrupted_live_entries();
        if changed {
            self.mark_dirty();
        }
        changed
    }

    /// Build the non-chrome body lines without consuming the dirty flag.
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

    /// Project one instruction epoch into the transcript: a finalized
    /// metadata-only card at the earliest deferred placeholder's canonical
    /// position, with the unexecuted placeholders absorbed behind it.
    /// Identical epochs (live event plus JSONL replay) dedup to one card.
    pub fn insert_instruction_epoch(&mut self, epoch: &InstructionEpochData) -> TranscriptEntryId {
        self.finish_active_text_blocks();
        self.instruction_deferred_tool_ids
            .extend(epoch.deferred_tool_ids.iter().cloned());
        let id = self.transcript.insert_instruction_epoch(
            epoch,
            self.workspace_root.clone().unwrap_or_default(),
            self.neo_home.clone(),
            self.tool_output_expanded,
        );
        self.mark_dirty();
        id
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
                && matches!(
                    status,
                    ToolStatusKind::Pending | ToolStatusKind::Queued | ToolStatusKind::Running
                )
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
        if let Some(index) = self.transcript.take_empty_live_attempt_anchor() {
            self.transcript.mutate_entry(index, |current| {
                *current = entry;
                true
            });
        } else {
            self.transcript.push(entry);
        }
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
                    ToolStatusKind::Pending | ToolStatusKind::Queued | ToolStatusKind::Running
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
        if self.instruction_deferred_tool_ids.contains(id) {
            // Instruction-deferred placeholders stay absorbed: their
            // provider-valid deferred results pass through this finish path
            // without executing and must not re-expose the placeholder.
            return;
        }
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
            ToolStatusKind::Pending | ToolStatusKind::Queued | ToolStatusKind::Running => {
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
        self.transcript.finish_thinking(false);
    }

    fn latest_compaction_is_complete(&self) -> bool {
        self.transcript
            .entries()
            .iter()
            .rev()
            .find_map(|entry| match entry {
                TranscriptEntry::Compaction { phase, percent, .. } => {
                    Some(compaction_is_complete(*phase, *percent))
                }
                _ => None,
            })
            .unwrap_or(false)
    }

    fn new_compaction_display_state(
        phase: neo_agent_core::CompactionPhase,
        confirmed_percent: u8,
        now_ms: u64,
    ) -> CompactionDisplayState {
        CompactionDisplayState {
            phase,
            confirmed_percent: confirmed_percent.min(99),
            display_percent: 0,
            phase_started_at_ms: now_ms,
            last_update_at_ms: now_ms,
        }
    }

    pub(super) fn upsert_compaction(
        &mut self,
        phase: Option<neo_agent_core::CompactionPhase>,
        percent: u8,
        compacted_message_count: usize,
        tokens_before: usize,
        tokens_after: usize,
    ) {
        let now_ms = super::entry::monotonic_time_ms();
        let is_complete = compaction_is_complete(phase, percent);
        let (display_phase, display_percent) = if is_complete {
            self.compaction_display = None;
            (Some(neo_agent_core::CompactionPhase::Applying), 100)
        } else {
            if self.compaction_display.is_none() {
                let phase = phase.unwrap_or(neo_agent_core::CompactionPhase::Estimating);
                self.compaction_display =
                    Some(Self::new_compaction_display_state(phase, percent, now_ms));
            }
            let state = self
                .compaction_display
                .expect("active compaction display state initialized above");
            (Some(state.phase), state.display_percent)
        };

        let changed = if let Some(index) = self
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
                if *existing_phase == display_phase
                    && *existing_percent == display_percent
                    && *existing_count == compacted_message_count
                    && *existing_tokens == tokens_before
                    && *existing_tokens_after == tokens_after
                {
                    return false;
                }
                *existing_phase = display_phase;
                *existing_percent = display_percent;
                *existing_count = compacted_message_count;
                *existing_tokens = tokens_before;
                *existing_tokens_after = tokens_after;
                true
            })
        } else {
            self.transcript.push(TranscriptEntry::Compaction {
                phase: display_phase,
                percent: display_percent,
                compacted_message_count,
                tokens_before,
                tokens_after,
            });
            true
        };
        if changed {
            self.mark_dirty();
        }
    }

    fn update_compaction_progress_at_ms(
        &mut self,
        phase: neo_agent_core::CompactionPhase,
        percent: u8,
        now_ms: u64,
    ) -> bool {
        let percent = percent.min(99);
        let Some(index) = self
            .transcript
            .entries()
            .iter()
            .rposition(is_live_compaction_entry)
        else {
            if self.latest_compaction_is_complete() {
                return false;
            }
            let state = Self::new_compaction_display_state(phase, percent, now_ms);
            self.compaction_display = Some(state);
            self.transcript.push(TranscriptEntry::Compaction {
                phase: Some(phase),
                percent: state.display_percent,
                compacted_message_count: 0,
                tokens_before: 0,
                tokens_after: 0,
            });
            self.mark_dirty();
            return true;
        };

        let mut state = self.compaction_display.unwrap_or_else(|| {
            let display_percent = match self.transcript.entries().get(index) {
                Some(TranscriptEntry::Compaction { percent, .. }) => *percent,
                _ => 0,
            };
            CompactionDisplayState {
                phase,
                confirmed_percent: display_percent,
                display_percent,
                phase_started_at_ms: now_ms,
                last_update_at_ms: now_ms,
            }
        });
        let current_phase = state.phase;
        let current_rank = compaction_phase_rank(current_phase);
        let incoming_rank = compaction_phase_rank(phase);
        if incoming_rank < current_rank {
            return false;
        }
        if incoming_rank > current_rank {
            state.phase = phase;
            state.confirmed_percent = percent;
            state.phase_started_at_ms = now_ms;
            state.last_update_at_ms = now_ms;
        } else {
            state.confirmed_percent = state.confirmed_percent.max(percent);
        }

        let elapsed_ms = now_ms.saturating_sub(state.phase_started_at_ms);
        let target = compaction_time_target(state.phase, state.confirmed_percent, elapsed_ms);
        let next_percent = target.min(
            state
                .display_percent
                .saturating_add(COMPACTION_MAX_STEP_PER_TICK),
        );
        let phase_changed = state.phase != current_phase;
        let display_changed = next_percent != state.display_percent;
        state.display_percent = next_percent;
        state.last_update_at_ms = now_ms;
        self.compaction_display = Some(state);

        let changed = self.transcript.mutate_entry(index, |entry| {
            let TranscriptEntry::Compaction {
                phase: existing_phase,
                percent: existing_percent,
                ..
            } = entry
            else {
                return false;
            };
            if !phase_changed && !display_changed {
                return false;
            }
            *existing_phase = Some(state.phase);
            *existing_percent = state.display_percent;
            true
        });
        if changed {
            self.mark_dirty();
        }
        changed
    }

    pub(super) fn update_compaction_progress(
        &mut self,
        phase: neo_agent_core::CompactionPhase,
        percent: u8,
    ) {
        let _ = self.update_compaction_progress_at_ms(
            phase,
            percent,
            super::entry::monotonic_time_ms(),
        );
    }

    fn advance_compaction_display(&mut self, now_ms: u64) -> bool {
        let Some(mut state) = self.compaction_display else {
            return false;
        };
        if now_ms.saturating_sub(state.last_update_at_ms) < COMPACTION_PROGRESS_TICK_MS {
            return false;
        }
        let elapsed_ms = now_ms.saturating_sub(state.phase_started_at_ms);
        let target = compaction_time_target(state.phase, state.confirmed_percent, elapsed_ms);
        let next_percent = target.min(
            state
                .display_percent
                .saturating_add(COMPACTION_MAX_STEP_PER_TICK),
        );
        state.last_update_at_ms = now_ms;
        if next_percent == state.display_percent {
            self.compaction_display = Some(state);
            return false;
        }
        state.display_percent = next_percent;
        self.compaction_display = Some(state);

        let Some(index) = self
            .transcript
            .entries()
            .iter()
            .rposition(is_live_compaction_entry)
        else {
            return false;
        };
        let changed = self.transcript.mutate_entry(index, |entry| {
            let TranscriptEntry::Compaction { percent, .. } = entry else {
                return false;
            };
            if *percent == state.display_percent {
                return false;
            }
            *percent = state.display_percent;
            true
        });
        if changed {
            self.mark_dirty();
        }
        changed
    }

    fn render_transcript_ansi_rows(&mut self, width: usize) -> Vec<String> {
        self.refresh_layout(width);
        let total = self.document.total_rows();
        self.visible_entries = self.visible_entry_indices(0, total);
        self.compose_rows(0, total, width)
    }

    /// Compose the bounded physical slice for a body viewport of `height`
    /// rows, resolving the document's anchor/follow state against the new
    /// document bottom. The frame composition itself is owned by the
    /// fullscreen lifecycle; this is the document-powered resolution.
    ///
    /// While a drag crosses the viewport edge, each frame-driven call also
    /// advances the document one row (the existing frame cadence) and extends
    /// the active endpoint into the revealed row. A pending press is checked
    /// against the long-press delay on the same cadence.
    ///
    /// Every frame re-derives the earliest unresolved blocking entry from
    /// the canonical entries and feeds it to the document, which confines
    /// the visible window to that entry until it resolves.
    #[must_use]
    pub fn render_visible_slice(&mut self, width: usize, height: usize) -> Vec<String> {
        self.body_height = height;
        let content_width = super::chrome_render::frame_content_width(width);
        self.refresh_layout(content_width);
        self.selection.tick(Instant::now());
        self.apply_drag_autoscroll(height);
        self.document
            .set_blocking_focus(self.blocking_entry_index());
        let range = self.document.visible_row_range(height);
        self.visible_entries = self.visible_entry_indices(range.start, range.end);
        self.compose_rows(range.start, range.end, content_width)
    }

    /// Compose the bounded physical slice for one terminal frame, consuming
    /// the dirty flag so steady-state frames do not keep the runtime's frame
    /// scheduler busy.
    #[must_use]
    pub fn render_terminal_slice(&mut self, width: usize, height: usize) -> Vec<String> {
        self.dirty = false;
        self.render_visible_slice(width, height)
    }

    /// Read access to the incremental document layout and view state.
    #[must_use]
    pub const fn document(&self) -> &DocumentLayout {
        &self.document
    }

    /// Reconcile the document with the store and feed fresh block heights for
    /// exactly the entries the document invalidated: revision changes
    /// (mutations, ticks, streaming deltas), explicit suppression transitions
    /// (which touch the affected `ToolRun` span), and rebuilds (width/theme/
    /// expansion). Unchanged cacheable entries keep their per-entry render
    /// cache.
    ///
    /// Non-cacheable entries are NOT re-rendered unconditionally. An entry's
    /// rendered height changes only when its revision changes, when
    /// suppression toggles, or when a rebuild invalidates everything: every
    /// entry kind in this codebase renders row-count-invariant animation
    /// (spinners, progress bars, elapsed headers are in-place), time-based
    /// live entries (Delegate family, Workflow, MCP/Retry status) tick and
    /// bump their revision, and streaming content (assistant/thinking/tool
    /// output) arrives through revision-bumping mutations. Visible
    /// non-cacheable entries are re-rendered during composition for output;
    /// off-screen ones keep their exact last measured height.
    fn refresh_layout(&mut self, width: usize) {
        self.document.set_width(width);
        self.transcript.ensure_cache_width(width);
        self.document.sync_entries(
            self.transcript.entry_ids(),
            self.transcript.entry_revisions(),
        );
        let re_render: BTreeSet<usize> = self.document.invalid_entries().into_iter().collect();
        #[cfg(test)]
        {
            self.last_reused_prefix_rows = 0;
        }
        self.frame_blocks.clear();
        for index in re_render {
            let block = self.entry_block_lines(index, width);
            if !block.is_empty() {
                self.frame_blocks.insert(index, block.clone());
            }
            self.document.set_entry_height(index, block.len());
        }
    }

    /// Compose the virtual rows `[start_row, end_row)` from per-entry render
    /// caches. Every entry contributes exactly its laid-out height (block
    /// plus one separator row when a preceding non-empty block exists), so
    /// the composed slice is a byte-exact window of the full document.
    fn compose_rows(&mut self, start_row: usize, end_row: usize, width: usize) -> Vec<String> {
        if start_row >= end_row || start_row >= self.document.total_rows() {
            return Vec::new();
        }
        let Some(start_entry) = self.document.entry_at_row(start_row) else {
            return Vec::new();
        };
        let mut rows: Vec<String> = Vec::new();
        let mut tool_run: Vec<ToolCallComponent> = Vec::new();
        let mut group_start: Option<usize> = None;
        let entry_count = self.transcript.entries().len();
        for index in start_entry..entry_count {
            let (entry_start, entry_height) = match self.document.entry_layout(index) {
                Some(layout) => (layout.start_row, layout.height),
                None => break,
            };
            if entry_start >= end_row {
                self.flush_group_block(&mut rows, &mut tool_run, group_start, width);
                break;
            }
            // Extract whether this is a ToolRun (and its id) in a short-lived
            // borrow scope so we can freely call &mut self methods afterward.
            let tool_run_id: Option<String> = match self.transcript.entries().get(index) {
                Some(TranscriptEntry::ToolRun { component }) => Some(component.id().to_owned()),
                _ => None,
            };
            if let Some(id) = tool_run_id {
                if self.transcript.is_tool_run_suppressed(&id) {
                    self.flush_group_block(&mut rows, &mut tool_run, group_start, width);
                    group_start = None;
                } else if let Some(TranscriptEntry::ToolRun { component }) =
                    self.transcript.entries().get(index)
                {
                    if tool_run.is_empty() {
                        group_start = Some(index);
                    }
                    tool_run.push(component.clone());
                }
            } else {
                self.flush_group_block(&mut rows, &mut tool_run, group_start, width);
                group_start = None;
                let block = self.rendered_block(index, width);
                if !block.is_empty() {
                    if entry_height > block.len() {
                        rows.push(String::new());
                    }
                    rows.extend(block);
                }
            }
        }
        self.flush_group_block(&mut rows, &mut tool_run, group_start, width);

        // Slice the composed window down to the requested virtual range.
        let base = self
            .document
            .entry_layout(start_entry)
            .map_or(start_row, |layout| layout.start_row);
        let offset = start_row.saturating_sub(base);
        if offset > 0 {
            rows.drain(..offset.min(rows.len()));
        }
        rows.truncate(end_row.saturating_sub(start_row));
        // Rows now map one-to-one onto the virtual range `[start_row, end_row)`.
        self.apply_selection_highlight(&mut rows, start_row);
        rows
    }

    /// Paint the active document selection onto composed visible rows, by
    /// document coordinates. Row `start_row + i` is the selection intersection
    /// of row `i`; endpoint cells mirror [`Self::materialize_selection`] —
    /// endpoints are inclusive, the min row cuts on the right (or left for an
    /// upward drag), and blank separator rows stay unpainted. The keyboard
    /// entry selection is already encoded as full-entry endpoints, so one
    /// code path covers mouse and keyboard selections.
    fn apply_selection_highlight(&self, rows: &mut [String], start_row: usize) {
        let (Some(anchor), Some(active)) = (self.selection.anchor(), self.selection.active())
        else {
            return;
        };
        let (Some(anchor), Some(active)) = (self.clamp_point(anchor), self.clamp_point(active))
        else {
            return;
        };
        let (Some(anchor_row), Some(active_row)) =
            (self.document.row_of(anchor), self.document.row_of(active))
        else {
            return;
        };
        let (min_row, max_row) = (anchor_row.min(active_row), anchor_row.max(active_row));
        let (min_start_cell, min_end_cell, max_start_cell, max_end_cell) =
            match anchor_row.cmp(&active_row) {
                std::cmp::Ordering::Equal => {
                    let start = anchor.display_cell.min(active.display_cell);
                    let end = anchor
                        .display_cell
                        .max(active.display_cell)
                        .saturating_add(1);
                    (start, end, start, end)
                }
                std::cmp::Ordering::Less => (
                    anchor.display_cell,
                    usize::MAX,
                    0,
                    active.display_cell.saturating_add(1),
                ),
                std::cmp::Ordering::Greater => (
                    0,
                    active.display_cell.saturating_add(1),
                    anchor.display_cell,
                    usize::MAX,
                ),
            };
        let mut entry = self.document.entry_at_row(min_row);
        for (index, line) in rows.iter_mut().enumerate() {
            let row = start_row.saturating_add(index);
            if row > max_row {
                break;
            }
            if row < min_row {
                continue;
            }
            // Advance the entry cursor monotonically over the selection rows.
            loop {
                let Some(entry_index) = entry else {
                    return;
                };
                let Some(layout) = self.document.entry_layout(entry_index) else {
                    return;
                };
                if row < layout.start_row + layout.height {
                    break;
                }
                entry =
                    (entry_index + 1 < self.document.layouts().len()).then_some(entry_index + 1);
            }
            let Some(entry_index) = entry else {
                return;
            };
            let Some(layout) = self.document.entry_layout(entry_index) else {
                return;
            };
            let block = self.document.block_height(entry_index).unwrap_or(0);
            let block_start = layout.start_row + layout.height.saturating_sub(block);
            if row < block_start {
                // Blank separator row: part of the selection as a newline but
                // never painted as a full row background.
                continue;
            }
            let start_cell = if row == min_row {
                min_start_cell
            } else if row == max_row {
                max_start_cell
            } else {
                0
            };
            let end_cell = if row == max_row {
                max_end_cell
            } else if row == min_row {
                min_end_cell
            } else {
                usize::MAX
            };
            *line = paint_selection_range(line, start_cell, end_cell, self.theme.selection_bg);
        }
    }

    /// The rendered block for one entry: the per-entry render cache for
    /// ordinary entries, the grouped tool-card block for the first member of
    /// an unsuppressed tool-run group. Blocks re-rendered during this frame's
    /// layout refresh are reused instead of rendered twice.
    fn rendered_block(&mut self, index: usize, width: usize) -> Vec<String> {
        if let Some(block) = self.frame_blocks.remove(&index) {
            return block;
        }
        let block = self.entry_block_lines(index, width);
        #[cfg(test)]
        {
            // The block came from a cached (unchanged) entry: count it as
            // reused render work.
            self.last_reused_prefix_rows = self.last_reused_prefix_rows.saturating_add(block.len());
        }
        block
    }

    /// Rendered block contribution of one entry: trimmed ANSI rows. Tool-run
    /// group blocks are attributed to the group's first member; other members
    /// and suppressed runs contribute nothing.
    fn entry_block_lines(&mut self, index: usize, width: usize) -> Vec<String> {
        let Some(entry) = self.transcript.entries().get(index) else {
            return Vec::new();
        };
        if let TranscriptEntry::ToolRun { component } = entry {
            if self.transcript.is_tool_run_suppressed(component.id()) {
                return Vec::new();
            }
            // Only the first member of an unsuppressed group carries the
            // grouped tool-card block.
            let preceded_by_group = index > 0
                && matches!(
                    self.transcript.entries().get(index - 1),
                    Some(TranscriptEntry::ToolRun { component })
                        if !self.transcript.is_tool_run_suppressed(component.id())
                );
            if preceded_by_group {
                return Vec::new();
            }
            let mut group = Vec::new();
            for entry in self.transcript.entries().iter().skip(index) {
                match entry {
                    TranscriptEntry::ToolRun { component }
                        if !self.transcript.is_tool_run_suppressed(component.id()) =>
                    {
                        group.push(component.clone());
                    }
                    _ => break,
                }
            }
            let mut ordered = group;
            let lines =
                super::chrome_render::render_ordered_tools(&mut ordered, width, &self.theme);
            let first = lines.iter().position(|line| !line.is_blank());
            let last = lines.iter().rposition(|line| !line.is_blank());
            let (Some(first), Some(last)) = (first, last) else {
                return Vec::new();
            };
            lines
                .into_iter()
                .skip(first)
                .take(last - first + 1)
                .map(|line| line.to_ansi())
                .collect()
        } else {
            let mut block = self.transcript.render_entry_ansi_cached(
                index,
                EntryRenderParams {
                    width,
                    theme: &self.theme,
                    activity_frame: self.activity_frame,
                    image_render_policy: self.image_render_policy,
                    image_capabilities: self.image_capabilities,
                    viewport_rows: self.body_height.max(1),
                },
            );
            trim_ansi_transcript_block(&mut block);
            block
        }
    }

    /// Append the grouped tool-card block for an accumulated tool run,
    /// inserting the document-driven separator row.
    fn flush_group_block(
        &mut self,
        rows: &mut Vec<String>,
        tool_run: &mut Vec<ToolCallComponent>,
        group_start: Option<usize>,
        width: usize,
    ) {
        if tool_run.is_empty() {
            return;
        }
        std::mem::take(tool_run);
        let Some(start) = group_start else {
            return;
        };
        let block = self.rendered_block(start, width);
        if block.is_empty() {
            return;
        }
        let separator = self
            .document
            .entry_layout(start)
            .is_some_and(|layout| layout.height > block.len());
        if separator {
            rows.push(String::new());
        }
        rows.extend(block);
    }

    #[cfg(test)]
    fn cached_prefix_rows_reused_for_test(&self) -> usize {
        self.last_reused_prefix_rows
    }
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
            Content::Video { mime_type, data } => {
                text.push_str(&media_summary("video", mime_type, data));
            }
        }
    }
    (text, images)
}

fn transcript_attachment_from_content_image(
    image_index: usize,
    mime_type: &str,
    data: &MediaRef,
) -> Option<crate::transcript::TranscriptImageAttachment> {
    let MediaRef::Base64(encoded) = data else {
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
        Content::Video { mime_type, data } => Some(media_summary("video", mime_type, data)),
    }
}

fn image_summary(mime_type: &str, data: &MediaRef) -> String {
    media_summary("image", mime_type, data)
}

fn media_summary(kind: &str, mime_type: &str, data: &MediaRef) -> String {
    match data {
        MediaRef::Url(url) => {
            format!("[{kind}: {mime_type} url={}]", sanitized_image_url(url))
        }
        MediaRef::Base64(data) => format!("[{kind}: {mime_type} data={} bytes]", data.len()),
        MediaRef::Blob(sha256) => format!("[{kind}: {mime_type} blob={sha256}]"),
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
                    percent: 1,
                    compacted_message_count: 23,
                    tokens_before: 51_000,
                    tokens_after: 0,
                }
            ),
            "latest card should carry the new compaction progress"
        );
    }

    fn latest_compaction_phase_percent(
        pane: &TranscriptPane,
    ) -> (Option<neo_agent_core::CompactionPhase>, u8) {
        pane.transcript()
            .entries()
            .iter()
            .rev()
            .find_map(|entry| match entry {
                TranscriptEntry::Compaction { phase, percent, .. } => Some((*phase, *percent)),
                _ => None,
            })
            .expect("compaction entry")
    }

    #[test]
    fn compaction_progress_is_rate_limited_and_monotonic() {
        let mut pane = TranscriptPane::new(80, 20);
        pane.upsert_compaction(
            Some(neo_agent_core::CompactionPhase::Estimating),
            0,
            0,
            0,
            0,
        );
        pane.dirty = false;

        pane.update_compaction_progress_at_ms(
            neo_agent_core::CompactionPhase::Summarizing,
            15,
            1_000,
        );
        let (phase, first) = latest_compaction_phase_percent(&pane);
        assert_eq!(phase, Some(neo_agent_core::CompactionPhase::Summarizing));
        assert_eq!(first, 1);

        pane.dirty = false;
        pane.update_compaction_progress_at_ms(
            neo_agent_core::CompactionPhase::Summarizing,
            85,
            1_000,
        );
        let (_, after_event) = latest_compaction_phase_percent(&pane);
        assert_eq!(after_event, first + 1);
        assert!(pane.dirty, "a one-point visible step should dirty the pane");

        pane.dirty = false;
        pane.advance_animation_at_ms(1_100);
        assert!(!pane.dirty, "sub-cadence ticks should not redraw compact");
        assert_eq!(latest_compaction_phase_percent(&pane).1, after_event);

        pane.advance_animation_at_ms(1_250);
        let (_, after_tick) = latest_compaction_phase_percent(&pane);
        assert_eq!(after_tick, after_event + 1);
        assert!(after_tick >= after_event);
    }

    #[test]
    fn compaction_summary_estimate_stays_below_completion() {
        let mut pane = TranscriptPane::new(80, 20);
        pane.upsert_compaction(
            Some(neo_agent_core::CompactionPhase::Estimating),
            0,
            0,
            0,
            0,
        );
        pane.update_compaction_progress_at_ms(neo_agent_core::CompactionPhase::Summarizing, 15, 0);

        for tick in 1..=2_000 {
            pane.advance_animation_at_ms(tick * COMPACTION_PROGRESS_TICK_MS);
        }
        let (_, before_confirmation) = latest_compaction_phase_percent(&pane);
        assert!(
            before_confirmation <= 82,
            "summary estimate: {before_confirmation}"
        );

        pane.update_compaction_progress_at_ms(
            neo_agent_core::CompactionPhase::Summarizing,
            85,
            500_001,
        );
        let (_, after_confirmation) = latest_compaction_phase_percent(&pane);
        assert!(
            after_confirmation <= before_confirmation + 1,
            "confirmed anchor must remain rate-limited: {before_confirmation} -> {after_confirmation}"
        );
        for tick in 2_001..=2_020 {
            pane.advance_animation_at_ms(tick * COMPACTION_PROGRESS_TICK_MS);
        }
        let (_, after_ticks) = latest_compaction_phase_percent(&pane);
        assert!(after_ticks <= 85, "summary estimate: {after_ticks}");
        assert!(after_ticks < 100, "only CompactionApplied may display 100%");
    }

    #[test]
    fn stale_compaction_progress_does_not_regress_or_reopen_completed_card() {
        let mut pane = TranscriptPane::new(80, 20);
        pane.upsert_compaction(
            Some(neo_agent_core::CompactionPhase::Estimating),
            0,
            0,
            0,
            0,
        );
        pane.update_compaction_progress_at_ms(
            neo_agent_core::CompactionPhase::Summarizing,
            15,
            1_000,
        );
        let (_, before_stale) = latest_compaction_phase_percent(&pane);

        pane.dirty = false;
        pane.update_compaction_progress_at_ms(
            neo_agent_core::CompactionPhase::SelectingBoundary,
            15,
            2_000,
        );
        let (phase, after_stale) = latest_compaction_phase_percent(&pane);
        assert_eq!(phase, Some(neo_agent_core::CompactionPhase::Summarizing));
        assert_eq!(after_stale, before_stale);
        assert!(!pane.dirty, "stale phase must be a no-op");

        pane.upsert_compaction(
            Some(neo_agent_core::CompactionPhase::Applying),
            100,
            4,
            100,
            40,
        );
        pane.dirty = false;
        pane.update_compaction_progress_at_ms(
            neo_agent_core::CompactionPhase::Summarizing,
            85,
            3_000,
        );
        assert_eq!(pane.transcript().entries().len(), 1);
        assert!(!pane.dirty, "delayed progress must not reopen completion");
    }

    #[test]
    fn duplicate_compaction_progress_is_a_noop() {
        let mut pane = TranscriptPane::new(80, 20);
        pane.upsert_compaction(
            Some(neo_agent_core::CompactionPhase::Estimating),
            0,
            0,
            0,
            0,
        );
        let now_ms = pane
            .compaction_display
            .expect("compaction display state")
            .last_update_at_ms;
        pane.dirty = false;

        pane.update_compaction_progress_at_ms(
            neo_agent_core::CompactionPhase::Estimating,
            0,
            now_ms,
        );
        assert!(!pane.dirty);
        pane.update_compaction_progress_at_ms(
            neo_agent_core::CompactionPhase::Estimating,
            0,
            now_ms,
        );
        assert!(!pane.dirty);
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
