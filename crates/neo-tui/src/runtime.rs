use std::collections::BTreeMap;

use neo_agent_core::AgentEvent;

use crate::ansi::{Style, paint, truncate_to_width, visible_width};
use crate::app::TuiTheme;
use crate::core::{Expandable, Finalization, Line, RenderKind, RenderScheduler};
use crate::renderer::CURSOR_MARKER;
use crate::transcript::{
    ToolCallComponent, ToolCallState, ToolGroup, TranscriptController, TranscriptEntry,
    render_tool_group,
};
use crate::widgets::box_draw;
use crate::{NeoTuiApp, PromptState, ToolStatusKind, wrap_width};

const DEFAULT_LIVE_CHROME_HEIGHT: usize = 5;

/// Uniform 1-column left/right gutter applied to ALL chrome (body, banner,
/// prompt box, footer). Matches kimi-code's `CHROME_GUTTER = 1`. Applied once
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

#[derive(Debug)]
pub struct NeoTuiRuntime {
    width: usize,
    height: usize,
    live_chrome_height: usize,
    transcript: TranscriptController,
    scheduler: RenderScheduler,
    tool_output_expanded: bool,
    tools: BTreeMap<String, ToolCallComponent>,
    tool_order: Vec<String>,
    streaming_tool_args: BTreeMap<String, String>,
    /// Finalized content that has already been pushed into scrollback by the
    /// renderer. Kept here so every frame re-exports the full history: in the
    /// pi-tui single-buffer model the renderer diffs the *whole* frame against
    /// the previous one and scrolls overflow into the terminal scrollback via
    /// `\r\n`. Without this buffer, drained finalized rows would vanish on the
    /// next frame and the renderer would blank them.
    history: Vec<String>,
    /// Cache of the last composed body frame (ANSI strings, no chrome), so
    /// tests can inspect rendered output via [`frame_ansi_lines`] without
    /// re-running the scheduler.
    last_frame: Vec<String>,
    /// Theme used to color the live transcript body. Mirrors [`NeoTuiApp`]'s
    /// theme; kept here (rather than borrowed) so the runtime can render
    /// without holding a reference to the app. The interactive mode keeps it
    /// in sync via [`Self::set_theme`].
    theme: TuiTheme,
}

impl NeoTuiRuntime {
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            live_chrome_height: DEFAULT_LIVE_CHROME_HEIGHT,
            transcript: TranscriptController::new(),
            scheduler: RenderScheduler::new(),
            tool_output_expanded: false,
            tools: BTreeMap::new(),
            tool_order: Vec::new(),
            streaming_tool_args: BTreeMap::new(),
            history: Vec::new(),
            last_frame: Vec::new(),
            theme: TuiTheme::default(),
        }
    }

    /// Update the theme used to color the live transcript body. Called by the
    /// interactive mode whenever the app's theme changes (e.g. from a
    /// `~/.neo/themes/*.json` file).
    pub fn set_theme(&mut self, theme: TuiTheme) {
        self.theme = theme;
        self.request_render(RenderKind::Incremental);
    }

    #[must_use]
    pub const fn theme(&self) -> TuiTheme {
        self.theme
    }

    pub fn push_transcript(&mut self, entry: TranscriptEntry) {
        self.transcript.push(entry);
        self.request_render(RenderKind::Incremental);
    }

    pub fn push_user_message(&mut self, content: impl Into<String>) {
        self.push_transcript(TranscriptEntry::user(content));
    }

    pub fn push_assistant_final(&mut self, content: impl Into<String>) {
        self.push_transcript(TranscriptEntry::assistant_final(content));
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
        use crate::transcript::messages::BannerData;
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
        self.push_assistant_final(content);
    }

    pub fn append_notice(&mut self, content: impl Into<String>) {
        self.push_transcript(TranscriptEntry::notice(content));
    }

    pub fn start_assistant_message(&mut self) {
        if !self.transcript.tail_is_live_assistant() {
            self.transcript.push(TranscriptEntry::assistant_live(""));
        }
        self.request_render(RenderKind::Incremental);
    }

    pub fn append_assistant_delta(&mut self, text: &str) {
        self.transcript.append_assistant_delta(text);
        self.request_render(RenderKind::Incremental);
    }

    pub fn finish_assistant_message(&mut self) {
        self.transcript.finalize_active_assistant();
        self.request_render(RenderKind::Incremental);
    }

    pub fn set_tool_output_expanded(&mut self, expanded: bool) {
        self.tool_output_expanded = expanded;
        for tool in self.tools.values_mut() {
            tool.set_expanded(expanded);
        }
        self.request_render(RenderKind::Incremental);
    }

    pub fn toggle_tool_output_expanded(&mut self) -> bool {
        if self.tools.is_empty() {
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
        self.request_render(RenderKind::Incremental);
    }

    #[must_use]
    pub const fn live_chrome_height(&self) -> usize {
        self.live_chrome_height
    }

    pub fn apply_agent_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::MessageStarted { .. } => self.start_assistant_message(),
            AgentEvent::TextDelta { text, .. } => self.append_assistant_delta(&text),
            AgentEvent::MessageFinished { .. } | AgentEvent::TurnFinished { .. } => {
                self.finish_assistant_message();
            }
            AgentEvent::ToolCallStarted { id, name, .. } => {
                self.upsert_tool(id, name, None, ToolStatusKind::Running);
                self.request_render(RenderKind::Incremental);
            }
            AgentEvent::ToolCallArgumentsDelta {
                id, json_fragment, ..
            } => {
                let arguments = self.streaming_tool_args.entry(id.clone()).or_default();
                arguments.push_str(&json_fragment);
                if let Some(tool) = self.tools.get_mut(&id) {
                    tool.update_call(Some(arguments.clone()));
                    self.request_render(RenderKind::Incremental);
                }
            }
            AgentEvent::ToolCallFinished { tool_call, .. } => {
                let arguments = tool_call.arguments.to_string();
                self.streaming_tool_args
                    .insert(tool_call.id.clone(), arguments.clone());
                self.upsert_tool(
                    tool_call.id,
                    tool_call.name,
                    Some(arguments),
                    ToolStatusKind::Running,
                );
                self.request_render(RenderKind::Incremental);
            }
            AgentEvent::ToolExecutionStarted {
                id,
                name,
                arguments,
                ..
            } => {
                let arguments = self
                    .streaming_tool_args
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| arguments.to_string());
                self.upsert_tool(id, name, Some(arguments), ToolStatusKind::Running);
                self.request_render(RenderKind::Incremental);
            }
            AgentEvent::ToolExecutionUpdate {
                id,
                name,
                partial_result,
                ..
            } => {
                self.upsert_tool(id.clone(), name, None, ToolStatusKind::Running);
                if let Some(tool) = self.tools.get_mut(&id) {
                    tool.append_live_output(partial_result.content);
                }
                self.request_render(RenderKind::Incremental);
            }
            AgentEvent::ToolExecutionFinished {
                id, name, result, ..
            } => {
                self.upsert_tool(id.clone(), name, None, ToolStatusKind::Running);
                self.streaming_tool_args.remove(&id);
                if let Some(tool) = self.tools.get_mut(&id) {
                    let details = result.details;
                    let exit_code = details
                        .as_ref()
                        .and_then(|details| details.get("exit_code"))
                        .and_then(serde_json::Value::as_i64)
                        .and_then(|code| i32::try_from(code).ok());
                    tool.set_result(Some(result.content), details, result.is_error, exit_code);
                }
                self.request_render(RenderKind::Incremental);
            }
            _ => {}
        }
    }

    pub fn request_render(&mut self, kind: RenderKind) {
        self.scheduler.request(kind);
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.scheduler.request(RenderKind::ForceFull);
    }

    pub fn render_tick(&mut self) {
        let _ = self.render_frame(self.width, self.height);
    }

    /// Render a single flat frame of all non-chrome content lines as ANSI
    /// strings: finalized transcript rows, finalized tool cards, live
    /// transcript rows, then live tool cards.
    ///
    /// The chrome (prompt box + footer) depends on [`NeoTuiApp`] state and is
    /// appended by the caller via [`runtime_chrome_ansi_lines`] before the
    /// whole frame is handed to [`crate::renderer::InlineRenderer::render`].
    /// This mirrors pi-tui's single-buffer model: every screen line lives in
    /// one `Vec<String>`, so the renderer can diff the whole frame and rewrite
    /// only what changed.
    ///
    /// Returns `None` when the scheduler has no pending render.
    #[must_use]
    pub fn render_frame(&mut self, width: usize, height: usize) -> Option<Vec<String>> {
        self.scheduler.take_next()?;
        self.width = width;
        self.height = height;

        let lines = self.compose_body_lines(width);
        self.last_frame.clone_from(&lines);
        Some(lines)
    }

    /// Build the non-chrome body lines without consuming a scheduler slot.
    /// Shared between [`render_frame`] (live path) and [`frame_ansi_lines`]
    /// (read-only snapshot for tests).
    ///
    /// In the pi-tui single-buffer model the renderer must see the *full*
    /// content on every frame so it can diff against the previous frame and
    /// scroll overflow into the terminal scrollback via `\r\n`. So we keep a
    /// `history` buffer of finalized rows and re-export it every frame,
    /// followed by the current live region (streaming assistant text + live
    /// tool cards + finalized tool cards). The chrome (prompt box + footer) is
    /// appended by the caller.
    fn compose_body_lines(&mut self, width: usize) -> Vec<String> {
        // The body renders at `content_width = width - CHROME_GUTTER` so that
        // when the caller prepends the gutter it never overflows. The gutter
        // itself is applied uniformly to body + chrome in
        // [`apply_gutter`], NOT here.
        let content_width = width.saturating_sub(CHROME_GUTTER).max(1);
        // 1. Drain newly finalized transcript rows into history (they will not
        //    change again). They stay in history so future frames still show
        //    them until the renderer scrolls them off-screen.
        for line in self
            .transcript
            .drain_finalized_rows(content_width, &self.theme)
        {
            self.history.push(line.to_ansi());
        }

        // 2. Drain newly finalized tool cards into history, interleaved after
        //    any finalized transcript rows above. This only drains when there
        //    is no live transcript tail, matching the previous semantics.
        let live_rows = self.transcript.render_live_rows(content_width, &self.theme);
        if live_rows.is_empty() {
            for line in self.drain_finalized_tool_rows() {
                self.history.push(line.to_ansi());
            }
        }

        // 3. Compose the frame: history + live region. The live region is the
        //    still-streaming assistant text plus live tool cards. Finalized
        //    tool cards that became live-only (when there IS a live transcript
        //    tail) are rendered inline here.
        let mut frame: Vec<String> = self.history.clone();
        let mut live = live_rows;
        if !live.is_empty() {
            live.extend(self.render_tool_rows());
        }
        if live.is_empty() {
            live.extend(self.render_tool_rows());
        }
        // Insert a blank separator between the finalized history and the live
        // region so a tool card block never touches the live assistant text
        // that follows it (and vice versa). Avoid a double blank when the
        // history already ends with one (e.g. a trailing tool-card gap).
        if !frame.is_empty()
            && !live.is_empty()
            && frame.last().is_some_and(|line| !line.is_empty())
        {
            frame.push(String::new());
        }
        for line in live {
            frame.push(line.to_ansi());
        }

        frame
    }

    /// Read-only snapshot of the most recently rendered body frame (ANSI
    /// strings, no chrome). Returns an empty vec before the first render.
    /// Used by tests that need to inspect what the runtime would draw.
    #[must_use]
    pub fn frame_ansi_lines(&self) -> Vec<String> {
        self.last_frame.clone()
    }

    #[must_use]
    pub const fn transcript(&self) -> &TranscriptController {
        &self.transcript
    }

    pub fn transcript_mut(&mut self) -> &mut TranscriptController {
        self.request_render(RenderKind::Incremental);
        &mut self.transcript
    }

    #[must_use]
    pub const fn dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    fn upsert_tool(
        &mut self,
        id: String,
        name: String,
        arguments: Option<String>,
        status: ToolStatusKind,
    ) {
        use crate::core::Expandable as _;

        if let Some(tool) = self.tools.get_mut(&id) {
            if arguments.is_some() {
                tool.update_call(arguments);
            }
            return;
        }

        let mut tool = ToolCallComponent::new(ToolCallState {
            id: id.clone(),
            name,
            arguments,
            result: None,
            details: None,
            status,
            exit_code: None,
        });
        tool.set_expanded(self.tool_output_expanded);
        self.tools.insert(id.clone(), tool);
        self.tool_order.push(id);
    }

    fn drain_finalized_tool_rows(&mut self) -> Vec<Line> {
        let finalized_ids: Vec<String> = self
            .tool_order
            .iter()
            .filter(|id| {
                self.tools
                    .get(*id)
                    .is_some_and(|tool| tool.finalization() == Finalization::Finalized)
            })
            .cloned()
            .collect();
        if finalized_ids.is_empty() {
            return Vec::new();
        }

        self.tool_order
            .retain(|id| !finalized_ids.iter().any(|finalized| finalized == id));

        // Collect the finalized tool components in order so we can group
        // consecutive reads (and other groupable tools) into tree cards.
        let mut ordered: Vec<ToolCallComponent> = Vec::new();
        for id in &finalized_ids {
            self.streaming_tool_args.remove(id);
            if let Some(tool) = self.tools.remove(id) {
                ordered.push(tool);
            }
        }
        render_ordered_tools(&mut ordered, self.width, &self.theme)
    }

    fn render_tool_rows(&mut self) -> Vec<Line> {
        // Render the live tool cards in order. We borrow mutably in order so
        // the theme-aware render can flush any cached state; grouping scans
        // consecutive same-name groupable tools and renders tree cards.
        let ordered_ids: Vec<String> = self.tool_order.clone();
        let mut ordered: Vec<ToolCallComponent> = Vec::new();
        for id in &ordered_ids {
            if let Some(tool) = self.tools.remove(id) {
                ordered.push(tool);
            }
        }
        // Put them back so future frames still see them as live.
        let rows = render_ordered_tools(&mut ordered, self.width, &self.theme);
        for tool in ordered {
            self.tools.insert(tool.id().to_owned(), tool);
        }
        rows
    }
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
    use crate::core::Expandable as _;

    let mut rows = Vec::new();
    let mut i = 0;
    while i < ordered.len() {
        // Each card (solo or group) is preceded by a blank line — separating
        // it both from any transcript text above the tool block and from the
        // previous card — so adjacent cards never touch (kimi-code
        // MESSAGE_INDENT gap between every visual block).
        rows.push(Line::raw(""));
        let current_name = ordered[i].name().to_owned();
        let groupable = is_groupable(&current_name);
        if !groupable {
            ordered[i].set_expanded(false);
            rows.extend(ordered[i].render_with_theme(width, theme));
            i += 1;
            continue;
        }
        // Greedy run of consecutive same-name groupable tools.
        let mut j = i + 1;
        while j < ordered.len()
            && ordered[j].name().eq_ignore_ascii_case(&current_name)
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
                    tool.set_expanded(false);
                    rows.extend(tool.render_with_theme(width, theme));
                }
            } else {
                let states: Vec<&ToolCallState> =
                    ordered[i..j].iter().map(ToolCallComponent::state).collect();
                let group = ToolGroup {
                    tool: current_name.clone(),
                    states,
                };
                rows.extend(render_tool_group(&group, theme));
            }
        } else {
            ordered[i].set_expanded(false);
            rows.extend(ordered[i].render_with_theme(width, theme));
        }
        i = j;
    }
    // Trail the tool block with a blank line so the next block (e.g. live
    // assistant text) is also separated from the last card.
    if !rows.is_empty() {
        rows.push(Line::raw(""));
    }
    rows
}

/// Whether a tool name is eligible for consecutive-call grouping.
fn is_groupable(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        "read" | "grep" | "glob" | "find" | "list"
    )
}

/// Chrome lines, optional cursor position, and the row where the prompt box
/// starts within those lines.
pub struct ChromeRender {
    pub lines: Vec<String>,
    pub cursor: Option<crate::CursorPos>,
    pub prompt_start_row: usize,
}

#[must_use]
pub fn runtime_chrome_ansi_lines(app: &NeoTuiApp, width: usize) -> ChromeRender {
    let content_width = width.saturating_sub(CHROME_GUTTER).max(1);
    let mut lines = Vec::new();
    let prompt_start_row = lines.len();
    let (prompt_lines, prompt_cursor) = render_prompt_lines(app, content_width);
    lines.extend(prompt_lines);
    if let Some(dropdown) = render_prompt_completion_dropdown(app, content_width) {
        lines.extend(dropdown);
    }
    lines.extend(render_footer_lines(app, content_width));
    ChromeRender {
        lines,
        cursor: prompt_cursor,
        prompt_start_row,
    }
}

/// Render only the footer lines (status bar + hint line), without the prompt
/// box. Used when a session picker overlay replaces the prompt/editor area.
#[must_use]
pub fn footer_only_ansi_lines(app: &NeoTuiApp, width: usize) -> Vec<String> {
    let content_width = width.saturating_sub(CHROME_GUTTER).max(1);
    render_footer_lines(app, content_width)
}

/// Render the `/` command dropdown below the prompt box, if active.
fn render_prompt_completion_dropdown(app: &NeoTuiApp, width: usize) -> Option<Vec<String>> {
    let overlay = app.focused_overlay()?;
    let crate::app::OverlayKind::PromptCompletion(state) = &overlay.kind else {
        return None;
    };
    let inner_width = width.saturating_sub(2).max(1);
    let raw_lines = state.render_lines(inner_width);
    if raw_lines.is_empty() {
        return None;
    }
    let theme = app.theme();
    let border_style = Style::default().fg(theme.accent);
    let mut lines = Vec::with_capacity(raw_lines.len() + 1);
    for raw in raw_lines {
        lines.push(box_draw::side_bordered_line(&raw, width, border_style));
    }
    lines.push(box_draw::bottom_border(width, border_style));
    Some(lines)
}

/// Render the rounded prompt input box. The first content line carries the
/// `> ` prompt symbol; continuation lines use a 4-space hanging indent so
/// wrapped/explicit-newline text aligns under the body (matching kimi-code's
/// `paddingX: 4` editor). Border color is muted by default and switches to
/// the accent color when the input starts with `/` or plan mode is active.
fn render_prompt_lines(app: &NeoTuiApp, width: usize) -> (Vec<String>, Option<crate::CursorPos>) {
    let theme = app.theme();
    let prompt = app.prompt();
    let highlighted = app.is_plan_mode() || prompt.text.trim_start().starts_with('/');
    let border_color = if highlighted {
        theme.accent
    } else {
        theme.muted
    };
    let border_style = Style::default().fg(border_color);
    let text_style = Style::default().fg(theme.header);

    let inner_width = width.saturating_sub(2).max(1);
    let body_width = inner_width.saturating_sub(4).max(1);

    let logical_lines = build_prompt_logical_lines(prompt, body_width);

    let mut lines = Vec::with_capacity(logical_lines.len() + 2);
    lines.push(box_draw::top_border(width, border_style));
    for (idx, line) in logical_lines.iter().enumerate() {
        let prefix = if idx == 0 { "  > " } else { "    " };
        let content = paint(&format!("{prefix}{line}"), text_style);
        lines.push(box_draw::content_line(&content, width, border_style));
    }
    lines.push(box_draw::bottom_border(width, border_style));

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
    let chars: Vec<char> = prompt.text.chars().collect();
    let cursor = prompt.cursor.min(chars.len());
    let before: String = chars[..cursor].iter().collect();
    let after: String = chars[cursor..].iter().collect();
    let marked = format!("{before}{CURSOR_MARKER}{after}");
    let mut out = Vec::new();
    for logical in marked.split('\n') {
        let wrapped = wrap_width(logical, body_width);
        if wrapped.is_empty() {
            out.push(String::new());
        } else {
            out.extend(wrapped);
        }
    }
    if out.len() > 6 {
        out.truncate(6);
    }
    out
}

fn find_cursor(lines: &[String]) -> Option<crate::CursorPos> {
    for (row, line) in lines.iter().enumerate() {
        if let Some(byte_pos) = line.find(CURSOR_MARKER) {
            let col = visible_width(&line[..byte_pos]);
            return Some(crate::CursorPos { row, col });
        }
    }
    None
}

fn render_footer_lines(app: &NeoTuiApp, width: usize) -> Vec<String> {
    let theme = app.theme();
    let (perm_label, perm_color) = app.permission_badge();
    let mut left_parts = vec![paint(
        &format!("[{perm_label}]"),
        Style::default().fg(perm_color),
    )];
    if !app.model_label().is_empty() {
        left_parts.push(paint(app.model_label(), Style::default().fg(theme.muted)));
    }
    if app.is_plan_mode() {
        left_parts.push(paint(
            "[PLAN MODE]",
            Style::default().fg(theme.warning).bold(),
        ));
    }
    if let Some(working) = app.working_label() {
        left_parts.push(paint(
            &format!("● {working}"),
            Style::default().fg(theme.footer_working),
        ));
    }
    left_parts.push(paint(&app.cwd_label(), Style::default().fg(theme.muted)));

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

    let hints = if width < 50 {
        "enter send · esc interrupt"
    } else {
        "enter send · shift+enter/ctrl+j newline · / commands"
    };
    vec![row, paint(hints, Style::default().fg(theme.footer_hint))]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{NeoTuiApp, PickerItem, PromptCompletionPrefix, TuiTheme};

    #[test]
    fn prompt_box_lines_are_exact_width() {
        let mut app = NeoTuiApp::new("neo", "s", "m", "/tmp");
        app.set_theme(TuiTheme::default());
        app.prompt_mut()
            .apply_edit(crate::PromptEdit::Insert("hello world"));
        let render = runtime_chrome_ansi_lines(&app, 40);
        // Lines render at content_width so the caller can apply CHROME_GUTTER.
        let expected_width = 40_usize.saturating_sub(CHROME_GUTTER).max(1);
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
        let mut app = NeoTuiApp::new("neo", "s", "m", "/tmp");
        app.prompt_mut().apply_edit(crate::PromptEdit::Insert("/"));
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
        let render = runtime_chrome_ansi_lines(&app, 60);
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
