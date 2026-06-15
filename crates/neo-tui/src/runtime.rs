use std::collections::BTreeMap;

use neo_agent_core::AgentEvent;

use crate::ansi::{Style, paint, truncate_to_width, visible_width};
use crate::core::{Expandable, Finalization, Line, RenderKind, RenderScheduler, TerminalRenderer};
use crate::renderer::CURSOR_MARKER;
use crate::transcript::{ToolCallComponent, ToolCallState, TranscriptController, TranscriptEntry};
use crate::{NeoTuiApp, PromptState, ToolStatusKind, wrap_width};

const DEFAULT_LIVE_CHROME_HEIGHT: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderOutput {
    pub committed: Vec<Line>,
    pub live: Vec<Line>,
}

#[derive(Debug)]
pub struct NeoTuiRuntime {
    width: usize,
    height: usize,
    live_chrome_height: usize,
    transcript: TranscriptController,
    scheduler: RenderScheduler,
    terminal: TerminalRenderer,
    tool_output_expanded: bool,
    tools: BTreeMap<String, ToolCallComponent>,
    tool_order: Vec<String>,
    streaming_tool_args: BTreeMap<String, String>,
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
            terminal: TerminalRenderer::new(width, height),
            tool_output_expanded: false,
            tools: BTreeMap::new(),
            tool_order: Vec::new(),
            streaming_tool_args: BTreeMap::new(),
        }
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
        self.terminal.resize(width, height);
        self.scheduler.request(RenderKind::ForceFull);
    }

    pub fn render_tick(&mut self) {
        let _ = self.render_output();
    }

    pub fn render_output(&mut self) -> Option<RenderOutput> {
        self.scheduler.take_next()?;

        let mut committed = self.transcript.drain_finalized_rows(self.width);
        let mut live = self.transcript.render_live_rows(self.width);
        if live.is_empty() {
            committed.extend(self.drain_finalized_tool_rows());
        } else {
            live.extend(self.render_tool_rows());
        }
        self.terminal.commit_rows(&committed);

        if live.is_empty() {
            live.extend(self.render_tool_rows());
        }
        live = clamp_tail(live, self.available_live_rows());
        self.terminal.render_live_region(&live, None);

        Some(RenderOutput { committed, live })
    }

    #[must_use]
    pub const fn terminal(&self) -> &TerminalRenderer {
        &self.terminal
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

    #[must_use]
    pub fn live_ansi_lines(&self) -> Vec<String> {
        self.terminal
            .live_rows()
            .iter()
            .map(Line::to_ansi)
            .collect()
    }

    #[must_use]
    pub fn committed_ansi_lines(&self) -> Vec<String> {
        self.terminal
            .committed_rows()
            .iter()
            .map(Line::to_ansi)
            .collect()
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
        use crate::core::Component as _;

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

        let mut rows = Vec::new();
        for id in finalized_ids {
            self.streaming_tool_args.remove(&id);
            if let Some(mut tool) = self.tools.remove(&id) {
                rows.extend(tool.render(self.width));
            }
        }
        rows
    }

    fn render_tool_rows(&mut self) -> Vec<Line> {
        use crate::core::Component as _;

        let mut rows = Vec::new();
        for id in &self.tool_order {
            if let Some(tool) = self.tools.get_mut(id) {
                rows.extend(tool.render(self.width));
            }
        }
        rows
    }

    fn available_live_rows(&self) -> usize {
        if self.height > self.live_chrome_height {
            self.height - self.live_chrome_height
        } else {
            self.height
        }
    }
}

fn clamp_tail(mut rows: Vec<Line>, max_rows: usize) -> Vec<Line> {
    if rows.len() > max_rows {
        rows.drain(..rows.len() - max_rows);
    }
    rows
}

#[must_use]
pub fn runtime_chrome_ansi_lines(
    app: &NeoTuiApp,
    width: usize,
) -> (Vec<String>, Option<crate::CursorPos>) {
    let mut lines = Vec::new();
    let (prompt_lines, prompt_cursor) =
        render_prompt_lines(app.prompt(), width, app.theme().prompt);
    lines.extend(prompt_lines);
    lines.extend(render_footer_lines(app, width));
    (lines, prompt_cursor)
}

fn render_prompt_lines(
    prompt: &PromptState,
    width: usize,
    color: crate::ansi::Color,
) -> (Vec<String>, Option<crate::CursorPos>) {
    let inner_width = width.saturating_sub(2).max(1);
    let display = prompt_display(prompt);
    let content_lines: Vec<String> = wrap_width(&display, inner_width)
        .into_iter()
        .take(6)
        .collect();
    let border_style = Style::default().fg(color);
    let text_style = Style::default().fg(color);

    let mut lines = Vec::with_capacity(content_lines.len() + 2);
    lines.push(paint(
        &format!("┌{}┐", "─".repeat(inner_width)),
        border_style,
    ));
    for line in content_lines {
        let pad = inner_width.saturating_sub(visible_width(&line));
        lines.push(format!(
            "{}{}{}",
            paint("│", border_style),
            paint(&format!("{line}{}", " ".repeat(pad)), text_style),
            paint("│", border_style)
        ));
    }
    lines.push(paint(
        &format!("└{}┘", "─".repeat(inner_width)),
        border_style,
    ));
    let cursor = find_cursor(&lines);
    let lines = lines
        .into_iter()
        .map(|line| line.replace(CURSOR_MARKER, ""))
        .collect();
    (lines, cursor)
}

fn prompt_display(prompt: &PromptState) -> String {
    let chars: Vec<char> = prompt.text.chars().collect();
    let cursor = prompt.cursor.min(chars.len());
    let before: String = chars[..cursor].iter().collect();
    let after: String = chars[cursor..].iter().collect();
    format!("> {before}{CURSOR_MARKER}{after}")
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
