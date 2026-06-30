# Shell Mode (`!`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `!` shell mode to neo's TUI where user-entered shell commands run directly with live output, queueing, cancellation, Ctrl+B backgrounding, detach timeout reset, durable context injection, and resume replay.

**Architecture:** Shell mode is a composer input mode, not a development mode. It uses the existing Bash execution and shell-event path, extended into one shared runner for both model Bash tool calls and user shell-mode commands. Do not add a parallel `UserShell*` event family, duplicate shell process runner, or XML parser for replay.

**Tech Stack:** Rust, tokio, `neo-agent-core` runtime/events/session JSONL, `neo-tui` transcript rendering.

**Spec:** `docs/superpowers/specs/2026-06-27-shell-mode-design.md`

**Git rule:** Do not run `git add`, `git commit`, `git checkout`, `git restore`, `git reset`, `git stash`, or other git mutations unless the user explicitly authorizes that exact command. The checkpoints below are review checkpoints, not commit instructions.

**Required v1 work:** Task 10 (foreground background-task registration and detach timeout reset) and Task 15 (Ctrl+B detach) are mandatory. Do not defer them or ship shell mode without them.

---

## File Structure

### New files

| File | Responsibility |
| --- | --- |
| `crates/neo-tui/src/utils/shell_output.rs` | ANSI/control sanitization and shell output formatting |
| `crates/neo-tui/src/widgets/shell_run.rs` | User shell-mode transcript widget for running/finished/backgrounded states |

### Modified files

| File | Responsibility | Key changes |
| --- | --- | --- |
| `crates/neo-agent-core/src/messages.rs` | Conversation model | Add `AgentMessage::ShellCommand`; convert it to provider user text at request boundary |
| `crates/neo-agent-core/src/events.rs` | Runtime event model | Extend existing `ShellCommandStarted/Finished`; add shell origin/outcome types; do not add `UserShell*` |
| `crates/neo-agent-core/src/tools/bash.rs` | Shared process execution | Extract one reusable Bash runner used by `BashTool` and shell mode |
| `crates/neo-agent-core/src/tools/background_tasks.rs` | Background lifecycle | Add foreground registration/detach and detach timeout reset |
| `crates/neo-agent-core/src/runtime.rs` | Context replay/application | Apply persisted shell command messages into `AgentContext` |
| `crates/neo-agent/src/modes/interactive.rs` | TUI controller | Shell mode input, execution dispatch, queue drain, Ctrl+B detach, cancellation, persistence |
| `crates/neo-agent/src/themes.rs` | Theme loading | Map `shell_mode` JSON color |
| `crates/neo-tui/src/shell/theme.rs` | Theme | Add `shell_mode` color |
| `crates/neo-tui/src/shell/mod.rs` | Chrome state | Shell mode/running state and working label |
| `crates/neo-tui/src/shell/pending_input.rs` | Queue state | Shell command FIFO/LIFO queue APIs |
| `crates/neo-tui/src/transcript/entry.rs` | Transcript entries | Add a user shell-run entry if the component is not represented elsewhere |
| `crates/neo-tui/src/transcript/event_handler.rs` | Event routing | Route existing shell events by origin |
| `crates/neo-tui/src/transcript/pane.rs` | Chrome/transcript rendering | Shell prompt, footer badge, queue layout, shell replay |
| `crates/neo-tui/src/widgets/pending_input_preview.rs` | Queue rendering | Render queued shell commands distinctly |
| `crates/neo-tui/src/widgets/box_draw.rs` | Border drawing | Top border with label |
| `crates/neo-tui/src/widgets/mod.rs` / `crates/neo-tui/src/utils/mod.rs` | Exports | Export new modules |
| `crates/neo-tui/src/input/keybinding.rs` | Keybindings | Add context-sensitive background action only if controller cannot intercept Ctrl+B earlier |

---

## Task 1: Theme Color

**Files:**
- Modify: `crates/neo-tui/src/shell/theme.rs`
- Modify: `crates/neo-agent/src/themes.rs`
- Test: `crates/neo-tui/tests/shell_mode_theme.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/neo-tui/tests/shell_mode_theme.rs`:

```rust
use neo_tui::primitive::Color;
use neo_tui::shell::TuiTheme;

#[test]
fn shell_mode_color_defaults_to_cyan() {
    assert_eq!(TuiTheme::default().shell_mode, Color::Rgb(86, 182, 194));
}

#[test]
fn shell_mode_color_has_builder() {
    let theme = TuiTheme::default().with_shell_mode(Color::Rgb(1, 2, 3));
    assert_eq!(theme.shell_mode, Color::Rgb(1, 2, 3));
}
```

- [ ] **Step 2: Run the failing test**


Expected: compile failure because `shell_mode` does not exist.

- [ ] **Step 3: Add the theme field**

In `crates/neo-tui/src/shell/theme.rs`, add:

```rust
/// Shell mode (`!`): prompt symbol, editor border, label, and `$ command`
/// echo lines.
pub shell_mode: Color,
```

Set the default:

```rust
shell_mode: Color::Rgb(86, 182, 194),
```

Add the builder:

```rust
#[must_use]
pub const fn with_shell_mode(mut self, color: Color) -> Self {
    self.shell_mode = color;
    self
}
```

- [ ] **Step 4: Wire theme JSON**

In `crates/neo-agent/src/themes.rs`, add `shell_mode: Option<String>` to the theme color override struct and map the JSON key `shell_mode` to `theme.shell_mode`.

- [ ] **Step 5: Verify**


Expected: PASS.

- [ ] **Checkpoint**

Review only these files: `theme.rs`, `themes.rs`, `shell_mode_theme.rs`.

---

## Task 2: Shell Mode Chrome State

**Files:**
- Modify: `crates/neo-tui/src/shell/mod.rs`
- Test: `crates/neo-tui/tests/shell_mode_state.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/neo-tui/tests/shell_mode_state.rs`:

```rust
use neo_tui::shell::NeoChromeState;

#[test]
fn shell_mode_defaults_to_inactive() {
    let app = NeoChromeState::new("neo", "s1", "model", "/tmp");
    assert!(!app.shell_mode_active());
    assert!(!app.shell_running());
}

#[test]
fn enter_and_exit_shell_mode_toggle_state() {
    let mut app = NeoChromeState::new("neo", "s1", "model", "/tmp");
    app.enter_shell_mode();
    assert!(app.shell_mode_active());
    app.exit_shell_mode();
    assert!(!app.shell_mode_active());
}

#[test]
fn shell_running_toggle_controls_working_label() {
    let mut app = NeoChromeState::new("neo", "s1", "model", "/tmp");
    app.set_shell_running(true);
    assert!(app.shell_running());
    assert_eq!(app.working_label().as_deref(), Some("shell · esc to cancel"));
    app.set_shell_running(false);
    assert!(!app.shell_running());
}
```

- [ ] **Step 2: Run the failing test**


Expected: compile failure because methods do not exist.

- [ ] **Step 3: Add fields and methods**

Add to `NeoChromeState`:

```rust
shell_mode_active: bool,
shell_running: bool,
```

Initialize both to `false` in `new()`.

Add:

```rust
#[must_use]
pub const fn shell_mode_active(&self) -> bool {
    self.shell_mode_active
}

#[must_use]
pub const fn shell_running(&self) -> bool {
    self.shell_running
}

pub fn set_shell_running(&mut self, running: bool) {
    self.shell_running = running;
}

pub fn enter_shell_mode(&mut self) {
    self.shell_mode_active = true;
}

pub fn exit_shell_mode(&mut self) {
    self.shell_mode_active = false;
}
```

Update `working_label()` so shell running wins before generic streaming:

```rust
if self.shell_running {
    return Some("shell · esc to cancel".to_owned());
}
```

- [ ] **Step 4: Verify**


Expected: PASS.

- [ ] **Checkpoint**

Review only `crates/neo-tui/src/shell/mod.rs` and the new test.

---

## Task 3: Shell Output Sanitization

**Files:**
- Create: `crates/neo-tui/src/utils/shell_output.rs`
- Modify: `crates/neo-tui/src/utils/mod.rs`
- Test: `crates/neo-tui/tests/shell_output.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/neo-tui/tests/shell_output.rs`:

```rust
use neo_tui::utils::shell_output::sanitize_shell_output;

#[test]
fn strips_common_terminal_sequences() {
    assert_eq!(sanitize_shell_output("\x1b[31mred\x1b[0m"), "red");
    assert_eq!(sanitize_shell_output("\x1b]0;title\x07hello"), "hello");
    assert_eq!(sanitize_shell_output("\x1b[?1049hhello\x1b[?1049l"), "hello");
    assert_eq!(sanitize_shell_output("\x1bcreset"), "reset");
}

#[test]
fn preserves_newline_and_tab_but_strips_other_c0_controls() {
    assert_eq!(sanitize_shell_output("a\x00b\x07c\n\t"), "abc\n\t");
}
```

- [ ] **Step 2: Run the failing test**


Expected: compile failure because the module does not exist.

- [ ] **Step 3: Implement sanitizer**

Create `crates/neo-tui/src/utils/shell_output.rs` with a manual scanner or compiled regex helpers. Prefer a manual scanner so `neo-tui` does not need a new dependency solely for this. The function must:

- remove OSC: `ESC ] ... BEL` and `ESC ] ... ESC \`
- remove CSI: `ESC [ ... final`
- remove single-character ESC sequences such as `ESC c`
- remove C0 controls except `\n` and `\t`

Add:

```rust
#[must_use]
pub fn sanitize_shell_output(raw: &str) -> String {
    // Manual byte scanner implementation.
}
```

Also add a formatting helper used later:

```rust
#[must_use]
pub fn split_sanitized_shell_lines(stdout: &str, stderr: &str) -> Vec<String> {
    let combined = format!("{}{}", sanitize_shell_output(stdout), sanitize_shell_output(stderr));
    combined.lines().map(str::to_owned).collect()
}
```

- [ ] **Step 4: Export**

Add `pub mod shell_output;` to `crates/neo-tui/src/utils/mod.rs`.

- [ ] **Step 5: Verify**


Expected: PASS.

---

## Task 4: Pending Shell Queue

**Files:**
- Modify: `crates/neo-tui/src/shell/pending_input.rs`

- [ ] **Step 1: Write tests**

Add to the file's test module:

```rust
#[test]
fn shell_queue_drains_fifo_but_edits_lifo() {
    let mut state = PendingInputState::new();
    state.queue_shell_command("one");
    state.queue_shell_command("two");
    assert_eq!(state.drain_next_shell_command(), Some("one".to_owned()));
    assert_eq!(state.pop_most_recent_shell_command_for_edit(), Some("two".to_owned()));
    assert!(state.is_empty());
}

#[test]
fn shell_queue_counts_as_pending_input() {
    let mut state = PendingInputState::new();
    state.queue_shell_command("whoami");
    assert!(!state.is_empty());
    assert!(state.has_queued_shell_commands());
}
```

- [ ] **Step 2: Run failing tests**


Expected: compile failure because queue APIs do not exist.

- [ ] **Step 3: Implement queue APIs**

Add:

```rust
queued_shell_commands: VecDeque<String>,
```

Add methods:

```rust
pub fn queue_shell_command(&mut self, text: impl Into<String>) {
    self.queued_shell_commands.push_back(text.into());
}

pub fn drain_next_shell_command(&mut self) -> Option<String> {
    self.queued_shell_commands.pop_front()
}

pub fn pop_most_recent_shell_command_for_edit(&mut self) -> Option<String> {
    self.queued_shell_commands.pop_back()
}

#[must_use]
pub fn queued_shell_commands(&self) -> &VecDeque<String> {
    &self.queued_shell_commands
}

#[must_use]
pub fn has_queued_shell_commands(&self) -> bool {
    !self.queued_shell_commands.is_empty()
}
```

Update `is_empty()` to include shell commands.

- [ ] **Step 4: Verify**


Expected: PASS.

---

## Task 5: Prompt Box Border Label

**Files:**
- Modify: `crates/neo-tui/src/widgets/box_draw.rs`

- [ ] **Step 1: Add tests**

Add:

```rust
#[test]
fn top_border_with_label_preserves_corners_and_width() {
    let style = Style::default();
    let line = top_border_with_label(30, "! shell mode", style, style);
    let plain = strip_ansi(&line);
    assert!(plain.starts_with('╭'));
    assert!(plain.ends_with('╮'));
    assert_eq!(visible_width(&plain), 30);
    assert!(plain.contains("! shell mode"));
}

#[test]
fn top_border_with_too_long_label_falls_back_to_plain_border() {
    let style = Style::default();
    let line = top_border_with_label(8, "! shell mode", style, style);
    let plain = strip_ansi(&line);
    assert!(!plain.contains("shell mode"));
    assert_eq!(visible_width(&plain), 8);
}
```

- [ ] **Step 2: Run failing test**


Expected: compile failure.

- [ ] **Step 3: Implement helper**

Add:

```rust
#[must_use]
pub fn top_border_with_label(
    width: usize,
    label: &str,
    border_style: Style,
    label_style: Style,
) -> String {
    if width < 2 {
        return String::new();
    }
    let inner = width - 2;
    let label_width = visible_width(label);
    if label_width + 1 > inner {
        return top_border(width, border_style);
    }
    let remaining = inner - label_width;
    format!(
        "{}{}{}{}",
        paint("╭", border_style),
        paint(label, label_style),
        paint(&"─".repeat(remaining), border_style),
        paint("╮", border_style),
    )
}
```

Use existing local border constants/helpers if present instead of duplicating glyph constants.

- [ ] **Step 4: Verify**


Expected: PASS.

---

## Task 6: Shell Prompt, Footer, And Queue Preview Rendering

**Files:**
- Modify: `crates/neo-tui/src/transcript/pane.rs`
- Modify: `crates/neo-tui/src/widgets/pending_input_preview.rs`
- Test: `crates/neo-tui/tests/shell_mode_render.rs`

- [ ] **Step 1: Write rendering tests**

Create `crates/neo-tui/tests/shell_mode_render.rs`:

```rust
use neo_tui::primitive::strip_ansi;
use neo_tui::shell::{NeoChromeState, PromptEdit};
use neo_tui::transcript::pane::render_chrome_lines;

fn render(app: &NeoChromeState) -> Vec<String> {
    render_chrome_lines(app, 80, 30)
        .lines
        .into_iter()
        .map(|line| strip_ansi(&line))
        .collect()
}

#[test]
fn shell_mode_prompt_uses_exclamation_prefix_and_label() {
    let mut app = NeoChromeState::new("neo", "s1", "model", "/tmp");
    app.enter_shell_mode();
    app.prompt_mut().apply_edit(PromptEdit::Insert("whoami"));
    let lines = render(&app);
    assert!(lines.iter().any(|line| line.contains("! shell mode")));
    assert!(lines.iter().any(|line| line.contains("! whoami")));
    assert!(!lines.iter().any(|line| line.contains("> whoami")));
}

#[test]
fn footer_shows_shell_badge_only_in_shell_mode() {
    let mut app = NeoChromeState::new("neo", "s1", "model", "/tmp");
    assert!(!render(&app).join("\n").contains("[shell]"));
    app.enter_shell_mode();
    assert!(render(&app).join("\n").contains("[shell]"));
}

#[test]
fn queued_shell_command_preview_uses_dollar_prompt_and_non_steer_hint() {
    let mut app = NeoChromeState::new("neo", "s1", "model", "/tmp");
    app.pending_input_mut().queue_shell_command("echo hi");
    let rendered = render(&app).join("\n");
    assert!(rendered.contains("$ echo hi"));
    assert!(rendered.contains("will run after current task"));
    assert!(!rendered.contains("ctrl-s to steer"));
}
```

- [ ] **Step 2: Run failing tests**


Expected: failures for missing render behavior.

- [ ] **Step 3: Render shell prompt**

In `render_prompt_lines()`, when `app.shell_mode_active()`:

- Use `theme.shell_mode` as border color.
- Use `box_draw::top_border_with_label(width, "! shell mode", border_style, border_style.bold())`.
- Render first-line prefix as `  ! `.
- Keep continuation lines on the existing hanging indent.

- [ ] **Step 4: Render footer badge**

In `render_footer_lines()`, push `[shell]` after the permission badge when `app.shell_mode_active()`.

- [ ] **Step 5: Render queued shell commands**

Extend `PendingInputPreview::new()` to accept `queued_shell_commands`. Render a shell section with:

```text
❯ $ command
  ↑ to edit · will run after current task
```

Use `theme.shell_mode` for `$ command`.

- [ ] **Step 6: Verify**


Expected: PASS.

---

## Task 7: Structured Shell Command Messages

**Files:**
- Modify: `crates/neo-agent-core/src/messages.rs`
- Modify: provider conversion code that maps `AgentMessage` to `ChatMessage`
- Test: add tests in `crates/neo-agent-core/src/messages.rs` or the existing conversion test module

- [ ] **Step 1: Write tests**

Add tests that assert:

```rust
#[test]
fn shell_command_message_serializes_as_structured_variant() {
    let message = AgentMessage::shell_command(
        "whoami",
        "chenyuanhao\n",
        "",
        Some(0),
        ShellCommandOutcome::Completed,
    );
    let json = serde_json::to_value(&message).expect("serialize");
    assert!(json.to_string().contains("ShellCommand"));
    assert!(json.to_string().contains("whoami"));
}

#[test]
fn shell_command_message_converts_to_user_text_for_model() {
    let message = AgentMessage::shell_command("whoami", "me\n", "", Some(0), ShellCommandOutcome::Completed);
    let chat = message.to_chat_message();
    assert_eq!(chat.role, neo_ai::Role::User);
    let text = chat.content_text();
    assert!(text.contains("<bash-input>"));
    assert!(text.contains("whoami"));
    assert!(text.contains("<bash-stdout>"));
}
```

Adjust helper calls to match existing `neo_ai::ChatMessage` APIs.

- [ ] **Step 2: Run failing tests**


Expected: compile failure.

- [ ] **Step 3: Add types**

In `messages.rs`, add:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ShellCommandOutcome {
    Completed,
    Cancelled,
    TimedOut,
    Backgrounded { task_id: String },
}
```

Add to `AgentMessage`:

```rust
ShellCommand {
    command: String,
    stdout: String,
    stderr: String,
    exit_code: Option<i32>,
    outcome: ShellCommandOutcome,
}
```

Add constructor:

```rust
#[must_use]
pub fn shell_command(
    command: impl Into<String>,
    stdout: impl Into<String>,
    stderr: impl Into<String>,
    exit_code: Option<i32>,
    outcome: ShellCommandOutcome,
) -> Self {
    Self::ShellCommand {
        command: command.into(),
        stdout: stdout.into(),
        stderr: stderr.into(),
        exit_code,
        outcome,
    }
}
```

- [ ] **Step 4: Convert to model user text**

Update `AgentMessage::to_chat_message()` so `ShellCommand` becomes a user-role text message containing escaped XML:

```xml
<bash-input>
...
</bash-input>
<bash-stdout>
...
</bash-stdout>
<bash-stderr>
...
</bash-stderr>
<bash-status exit_code="0" outcome="completed" />
```

Escape at least `&`, `<`, and `>`.

- [ ] **Step 5: Verify**


Expected: PASS.

---

## Task 8: Extend Existing Shell Events

**Files:**
- Modify: `crates/neo-agent-core/src/events.rs`
- Modify: `crates/neo-agent-core/src/runtime.rs`
- Modify: `crates/neo-tui/src/transcript/event_handler.rs`
- Test: event serialization tests in `events.rs`

- [ ] **Step 1: Write tests**

Add an event serialization test:

```rust
#[test]
fn shell_events_include_origin_and_outcome() {
    let started = AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "shell-1".to_owned(),
        command: "whoami".to_owned(),
        cwd: std::path::PathBuf::from("/tmp"),
        origin: ShellCommandOrigin::UserShellMode,
    };
    let started_json = serde_json::to_string(&started).expect("serialize");
    assert!(started_json.contains("UserShellMode"));

    let finished = AgentEvent::ShellCommandFinished {
        turn: 1,
        id: "shell-1".to_owned(),
        exit_code: Some(0),
        stdout: "me\n".to_owned(),
        stderr: String::new(),
        truncated: false,
        origin: ShellCommandOrigin::UserShellMode,
        outcome: ShellCommandOutcome::Completed,
    };
    let finished_json = serde_json::to_string(&finished).expect("serialize");
    assert!(finished_json.contains("Completed"));
}
```

- [ ] **Step 2: Run failing tests**


Expected: compile failure.

- [ ] **Step 3: Add origin/outcome**

In `events.rs`, add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ShellCommandOrigin {
    ModelBashTool,
    UserShellMode,
}
```

Use the `ShellCommandOutcome` type from `messages.rs` or re-export it from the crate root so events and messages share the same enum.

Extend existing `ShellCommandStarted` and `ShellCommandFinished` variants with `origin` and `outcome`. Do not add old-shape compatibility branches for shell events; runtime and replay should use one current event shape after this migration.

- [ ] **Step 4: Update current Bash tool event emitters**

In `runtime.rs`:

- `emit_shell_started()` sets `origin: ShellCommandOrigin::ModelBashTool`.
- `emit_shell_finished()` sets `origin: ShellCommandOrigin::ModelBashTool` and `outcome` from result details. If result details do not contain an outcome yet, map `exit_code` to `Completed` for now and let Task 9 add full outcomes.

- [ ] **Step 5: Update TUI event matching**

In `event_handler.rs`, branch on `origin`:

- `ModelBashTool` keeps current tool-card rendering.
- `UserShellMode` updates the new shell-run transcript component.

- [ ] **Step 6: Verify**


Expected: PASS.

---

## Task 9: Shared Bash Runner With Cancellation

**Files:**
- Modify: `crates/neo-agent-core/src/tools/bash.rs`
- Test: inline tests in `bash.rs`

- [ ] **Step 1: Write tests**

Add tests that cover the shared runner directly:

```rust
#[tokio::test]
async fn shell_runner_collects_stdout_and_exit_code() {
    let ctx = ToolContext::default();
    let result = execute_shell_command(ShellExecutionRequest {
        id: "shell-1".to_owned(),
        command: "printf ok".to_owned(),
        cwd: ctx.cwd.clone(),
        origin: ShellCommandOrigin::UserShellMode,
        foreground_timeout: Duration::from_secs(5),
        background_timeout: Duration::from_secs(600),
        max_output_bytes: 1024,
        cancel_token: CancellationToken::new(),
        stream_update: None,
    })
    .await
    .expect("runner succeeds");
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.stdout, "ok");
    assert_eq!(result.outcome, ShellCommandOutcome::Completed);
}

#[tokio::test]
async fn shell_runner_cancel_kills_process_group() {
    let ctx = ToolContext::default();
    let token = CancellationToken::new();
    let cloned = token.clone();
    let task = tokio::spawn(async move {
        execute_shell_command(ShellExecutionRequest {
            id: "shell-2".to_owned(),
            command: "sleep 30".to_owned(),
            cwd: ctx.cwd.clone(),
            origin: ShellCommandOrigin::UserShellMode,
            foreground_timeout: Duration::from_secs(60),
            background_timeout: Duration::from_secs(600),
            max_output_bytes: 1024,
            cancel_token: cloned,
            stream_update: None,
        })
        .await
    });
    token.cancel();
    let result = task.await.expect("join").expect("runner returns cancelled result");
    assert_eq!(result.outcome, ShellCommandOutcome::Cancelled);
}
```

Adjust `ToolContext::default()` setup if existing tests use a helper.

- [ ] **Step 2: Run failing tests**


Expected: compile failure.

- [ ] **Step 3: Extract request/result types**

In `bash.rs`, add:

```rust
pub struct ShellExecutionRequest {
    pub id: String,
    pub command: String,
    pub cwd: PathBuf,
    pub origin: ShellCommandOrigin,
    pub foreground_timeout: Duration,
    pub background_timeout: Duration,
    pub max_output_bytes: usize,
    pub cancel_token: CancellationToken,
    pub stream_update: Option<ToolUpdateCallback>,
}

pub struct ShellExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub truncated: bool,
    pub outcome: ShellCommandOutcome,
}
```

- [ ] **Step 4: Move process logic into shared function**

Replace the private `run_command()` implementation with:

```rust
pub async fn execute_shell_command(
    request: ShellExecutionRequest,
) -> Result<ShellExecutionResult, ToolError> {
    // spawn bash -lc with process group
    // stream stdout/stderr through request.stream_update
    // select on child.wait(), foreground timeout, and request.cancel_token.cancelled()
    // kill process group on timeout/cancel
    // drain readers
    // cap details consistently with existing Bash output cap helpers
}
```

Use existing helpers:

- `spawn_bash_process`
- `spawn_streaming_output_reader`
- `kill_child`
- `drain_reader`
- `output_from_buffers`
- `cap_plain_output` / details helpers where applicable

Do not leave the old `run_command()` as a separate execution path.

- [ ] **Step 5: Update BashTool to call shared runner**

`BashTool::execute()` still performs permission checks, parses `BashInput`, and handles `run_in_background=true`. For foreground Bash calls, construct `ShellExecutionRequest { origin: ModelBashTool, ... }` and convert the result through `command_result`.

- [ ] **Step 6: Verify**


Expected: PASS.


Expected: existing Bash tests pass.

---

## Task 10: Foreground Background-Task Registration And Detach Timeout Reset

**Files:**
- Modify: `crates/neo-agent-core/src/tools/background_tasks.rs`
- Modify: `crates/neo-agent-core/src/tools/bash.rs`
- Test: inline tests in `background_tasks.rs`

- [ ] **Step 1: Write tests**

Add tests:

```rust
#[tokio::test]
async fn foreground_bash_task_can_be_detached() {
    let manager = BackgroundTaskManager::new();
    let command = test_managed_command_that_never_finishes();
    let task_id = manager
        .start_bash_foreground("long command".to_owned(), command, 1024, Duration::from_secs(600))
        .await
        .expect("foreground task id");

    let snapshot = manager.detach(&task_id).await.expect("detach");
    assert_eq!(snapshot.status, BackgroundTaskStatus::Running);
    assert_eq!(snapshot.task_id, task_id);
}

#[tokio::test]
async fn detach_resets_deadline_to_background_timeout() {
    let manager = BackgroundTaskManager::new();
    let command = test_managed_command_that_never_finishes();
    let task_id = manager
        .start_bash_foreground("long command".to_owned(), command, 1024, Duration::from_millis(50))
        .await
        .expect("foreground task id");

    tokio::time::sleep(Duration::from_millis(30)).await;
    manager.detach(&task_id).await.expect("detach");
    tokio::time::sleep(Duration::from_millis(40)).await;

    let snapshot = manager.output(&task_id, false, Duration::from_millis(1), 1024).await.expect("output");
    assert!(snapshot.content.contains("status: running"));
}
```

Use existing test helpers for `ManagedBackgroundCommand`, or add focused local helpers near current background task tests.

- [ ] **Step 2: Run failing tests**


Expected: compile failure because foreground registration/detach APIs do not exist.

- [ ] **Step 3: Add foreground-aware state**

Extend `BackgroundTaskRecord` with:

```rust
detached: bool,
deadline: Option<Instant>,
detach_timeout: Option<Duration>,
```

Existing background `start_bash()` creates `detached: true`. New foreground registration creates `detached: false`.

- [ ] **Step 4: Add APIs**

Add:

```rust
pub async fn start_bash_foreground(
    &self,
    description: String,
    command: ManagedBackgroundCommand,
    max_output_bytes: usize,
    detach_timeout: Duration,
) -> Result<String, ToolError>
```

Add:

```rust
pub async fn detach(&self, task_id: &str) -> Result<BackgroundTaskSnapshot, ToolError>
```

`detach()` sets `detached = true` and resets `deadline = Some(Instant::now() + detach_timeout)`.

- [ ] **Step 5: Enforce deadlines**

In `snapshot_inner()` or a helper used by `output/list`, if a running Bash task has `deadline <= Instant::now()`, stop it through the existing cleanup path and mark status `TimedOut`.

- [ ] **Step 6: Wire shared runner**

The shared runner registers shell-mode foreground commands with `start_bash_foreground()` so Ctrl+B has a task id to detach. Bash tool foreground calls may also use this path if it simplifies the implementation, but model Bash transcript behavior must stay unchanged.

- [ ] **Step 7: Verify**


Expected: PASS.

---

## Task 11: ShellRunComponent And Transcript Replay

**Files:**
- Create: `crates/neo-tui/src/widgets/shell_run.rs`
- Modify: `crates/neo-tui/src/widgets/mod.rs`
- Modify: `crates/neo-tui/src/transcript/entry.rs`
- Modify: `crates/neo-tui/src/transcript/pane.rs`
- Test: `crates/neo-tui/tests/shell_run.rs`

- [ ] **Step 1: Write tests**

Create `crates/neo-tui/tests/shell_run.rs`:

```rust
use neo_agent_core::ShellCommandOutcome;
use neo_tui::primitive::strip_ansi;
use neo_tui::widgets::shell_run::{ShellRunComponent, ShellRunState};

#[test]
fn running_shell_run_shows_tail_and_background_hint() {
    let mut run = ShellRunComponent::new("id", "seq 1 10");
    run.append_output("1\n2\n3\n4\n5\n6\n");
    let lines = run.render(80, &Default::default(), 0);
    let plain = lines.into_iter().map(|line| strip_ansi(&line.to_string())).collect::<Vec<_>>().join("\n");
    assert!(plain.contains("$ seq 1 10"));
    assert!(plain.contains("6"));
    assert!(plain.contains("ctrl+b to background"));
}

#[test]
fn backgrounded_shell_run_renders_background_notice() {
    let mut run = ShellRunComponent::new("id", "sleep 30");
    run.finish(ShellRunState::Finished {
        stdout: String::new(),
        stderr: String::new(),
        exit_code: None,
        outcome: ShellCommandOutcome::Backgrounded { task_id: "bash-1".to_owned() },
    });
    let lines = run.render(80, &Default::default(), 0);
    let plain = lines.into_iter().map(|line| strip_ansi(&line.to_string())).collect::<Vec<_>>().join("\n");
    assert!(plain.contains("Moved to background"));
}
```

Adjust line conversion to match the local `Line` type.

- [ ] **Step 2: Run failing tests**


Expected: compile failure.

- [ ] **Step 3: Implement component**

`ShellRunComponent` stores:

```rust
id: String,
command: String,
state: ShellRunState,
live_output: Vec<String>,
live_output_chars: usize,
started_at: Instant,
```

Cap live output to 256 KiB and keep a rendered tail. Render:

- `$ command` in `theme.shell_mode`.
- Running tail dimmed plus elapsed/background hint.
- Finished stdout/stderr using `shell_output` helpers.
- Backgrounded notice.

- [ ] **Step 4: Add transcript entry**

Add a transcript entry variant if needed:

```rust
ShellRun { component: ShellRunComponent }
```

Add copy/render support in `entry.rs` and `pane.rs`.

- [ ] **Step 5: Verify**


Expected: PASS.

---

## Task 12: TUI Event Routing For User Shell Origin

**Files:**
- Modify: `crates/neo-tui/src/transcript/event_handler.rs`
- Modify: `crates/neo-tui/src/transcript/store.rs` if helper methods are needed
- Test: `crates/neo-tui/tests/shell_events.rs`

- [ ] **Step 1: Write tests**

Create `crates/neo-tui/tests/shell_events.rs`:

```rust
use neo_agent_core::{AgentEvent, ShellCommandOrigin, ShellCommandOutcome};
use neo_tui::transcript::TranscriptPane;

#[test]
fn user_shell_origin_creates_shell_run_not_tool_card() {
    let mut pane = TranscriptPane::default();
    pane.apply_agent_event(AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "shell-1".to_owned(),
        command: "whoami".to_owned(),
        cwd: "/tmp".into(),
        origin: ShellCommandOrigin::UserShellMode,
    });
    let rendered = pane.rendered_text_for_test(80);
    assert!(rendered.contains("$ whoami"));
    assert!(!rendered.contains("Bash"));
}

#[test]
fn user_shell_finish_updates_existing_shell_run() {
    let mut pane = TranscriptPane::default();
    pane.apply_agent_event(AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "shell-1".to_owned(),
        command: "whoami".to_owned(),
        cwd: "/tmp".into(),
        origin: ShellCommandOrigin::UserShellMode,
    });
    pane.apply_agent_event(AgentEvent::ShellCommandFinished {
        turn: 1,
        id: "shell-1".to_owned(),
        exit_code: Some(0),
        stdout: "me\n".to_owned(),
        stderr: String::new(),
        truncated: false,
        origin: ShellCommandOrigin::UserShellMode,
        outcome: ShellCommandOutcome::Completed,
    });
    let rendered = pane.rendered_text_for_test(80);
    assert!(rendered.contains("me"));
}
```

Use existing transcript snapshot helpers if there is no `rendered_text_for_test`.

- [ ] **Step 2: Run failing tests**


Expected: failure because routing does not exist.

- [ ] **Step 3: Route started/updates/finished**

In `apply_tool_event()`:

- `ShellCommandStarted { origin: ModelBashTool }` uses current `start_shell_command`.
- `ShellCommandStarted { origin: UserShellMode }` creates `ShellRunComponent`.
- `ToolExecutionUpdate` checks whether id belongs to a shell-run entry first; if yes append live output there, otherwise keep existing tool update behavior.
- `ShellCommandFinished { origin: UserShellMode }` finalizes shell-run entry.

- [ ] **Step 4: Verify**


Expected: PASS.

---

## Task 13: Shell Mode Input Entry And Exit

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Test: existing interactive controller tests in the same file

- [ ] **Step 1: Add controller tests**

Add tests near existing input tests:

```rust
#[tokio::test]
async fn bang_on_empty_prompt_enters_shell_mode_without_inserting_bang() {
    let mut controller = running_turn_controller().await;
    controller.handle_input_event(InputEvent::Insert('!')).await.expect("input");
    assert!(controller.tui.chrome().shell_mode_active());
    assert_eq!(controller.tui.chrome().prompt().text, "");
}

#[tokio::test]
async fn paste_bang_command_enters_shell_mode_and_strips_prefix() {
    let mut controller = running_turn_controller().await;
    controller.handle_input_event(InputEvent::Paste("!whoami".to_owned())).await.expect("input");
    assert!(controller.tui.chrome().shell_mode_active());
    assert_eq!(controller.tui.chrome().prompt().text, "whoami");
}

#[tokio::test]
async fn backspace_on_empty_shell_prompt_exits_shell_mode() {
    let mut controller = running_turn_controller().await;
    controller.tui.chrome_mut().enter_shell_mode();
    controller.handle_input_event(InputEvent::Backspace).await.expect("input");
    assert!(!controller.tui.chrome().shell_mode_active());
}
```

- [ ] **Step 2: Run failing tests**


Expected: failures.

- [ ] **Step 3: Implement `!` and paste entry**

In `handle_insert_prompt_event()`, before inserting text:

```rust
if character == '!'
    && self.tui.chrome().prompt().text.is_empty()
    && !self.tui.chrome().shell_mode_active()
    && self.tui.chrome().mode() == ChromeMode::Editing
    && !self.tui.chrome().focused_overlay_blocks_prompt()
{
    self.tui.chrome_mut().enter_shell_mode();
    return;
}
```

In the paste handler, if current prompt is empty and text starts with `!`, enter shell mode and insert the remainder.

- [ ] **Step 4: Implement empty prompt exit**

Backspace and Esc on empty shell prompt call `exit_shell_mode()` and consume the input.

- [ ] **Step 5: Verify**


Expected: the new entry/exit tests pass.

---

## Task 14: Shell Command Dispatch And Persistence

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Modify: `crates/neo-agent-core/src/session/mod.rs` only if replay helpers need message matching updates
- Test: interactive controller tests

- [ ] **Step 1: Write tests**

Add controller tests:

```rust
#[tokio::test]
async fn enter_in_shell_mode_runs_without_starting_model_turn() {
    let shell_calls = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::clone(&shell_calls);
    let mut controller = controller_with_shell_executor(move |command| {
        calls.lock().unwrap().push(command);
        ShellExecutionResult::success("me\n")
    });
    controller.tui.chrome_mut().enter_shell_mode();
    controller.replace_prompt_text("whoami");
    controller.submit_current_prompt().await.expect("submit");
    assert_eq!(shell_calls.lock().unwrap().as_slice(), ["whoami"]);
    assert!(controller.tui.chrome().shell_mode_active());
    assert!(controller.session_messages.iter().any(|message| matches!(message, AgentMessage::ShellCommand { .. })));
}

#[tokio::test]
async fn shell_command_while_turn_running_queues_shell_command() {
    let mut controller = running_turn_controller().await;
    controller.tui.chrome_mut().enter_shell_mode();
    controller.replace_prompt_text("whoami");
    controller.submit_current_prompt().await.expect("submit");
    assert_eq!(controller.tui.chrome().pending_input().queued_shell_commands().len(), 1);
}
```

Build test helpers around the existing `TurnDriver`; shell executor injection should be explicit and testable.

- [ ] **Step 2: Run failing tests**


Expected: compile failure until shell executor exists.

- [ ] **Step 3: Add shell executor dependency**

Add a controller field:

```rust
shell_executor: ShellExecutor,
```

Define `ShellExecutor` as a callback that starts/controls one shell command and streams existing `AgentEvent` values back to the controller. It must expose:

- command id
- foreground task id for detach
- cancellation token
- completion result

Do not instantiate a second `AgentRuntime` inside the controller. Production `controller_for_config()` builds this executor from the same runtime/tool registry/config path as normal turns.

- [ ] **Step 4: Implement submit path**

At the top of `submit_current_prompt()`, after `/btw` routing:

```rust
if self.tui.chrome().shell_mode_active() {
    return self.submit_shell_command().await;
}
```

`submit_shell_command()`:

1. Reads trimmed command.
2. Returns on empty command.
3. Clears prompt but keeps shell mode active.
4. If `active_turn.is_some()` or `shell_running`, queues via `queue_shell_command`.
5. Otherwise calls `execute_shell_command(command).await`.

- [ ] **Step 5: Persist result**

When a shell command completes/cancels/times out/backgrounds, append:

```rust
AgentEvent::MessageAppended {
    message: AgentMessage::shell_command(command, stdout, stderr, exit_code, outcome),
}
```

Route that event through the same `apply_turn_event()` path used by runtime events so `session_messages` and transcript state remain in sync. Ensure JSONL writing receives the event in production.

- [ ] **Step 6: Drain queue**

After shell completion:

1. If `pending_input_mut().drain_next_shell_command()` returns a command, execute it.
2. Else if a queued follow-up exists and no active turn exists, start the follow-up using the existing follow-up mechanism.
3. Else set `shell_running = false`.

- [ ] **Step 7: Verify**


Expected: PASS.

---

## Task 15: Ctrl+B Detach

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Modify: `crates/neo-tui/src/input/keybinding.rs` only if needed
- Test: interactive controller tests

- [ ] **Step 1: Write tests**

Add:

```rust
#[tokio::test]
async fn ctrl_b_detaches_running_shell_command() {
    let detach_calls = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::clone(&detach_calls);
    let mut controller = controller_with_running_shell("bash-1", move |task_id| {
        calls.lock().unwrap().push(task_id.to_owned());
        Ok(())
    });
    controller.handle_input_event(InputEvent::Action(KeybindingAction::BackgroundTask)).await.expect("ctrl-b");
    assert_eq!(detach_calls.lock().unwrap().as_slice(), ["bash-1"]);
    assert!(!controller.tui.chrome().shell_running());
    assert!(controller.session_messages.iter().any(|message| matches!(
        message,
        AgentMessage::ShellCommand {
            outcome: ShellCommandOutcome::Backgrounded { .. },
            ..
        }
    )));
}
```

- [ ] **Step 2: Run failing test**


Expected: compile failure until action/executor support exists.

- [ ] **Step 3: Add context-sensitive action**

If Ctrl+B is currently bound to editor cursor-left, do not globally steal it. Add a context-sensitive check before normal editor handling:

```rust
if key == Ctrl+B && self.tui.chrome().shell_running() {
    self.detach_shell_command().await?;
    return Ok(false);
}
```

If the keybinding layer requires an action, add `KeybindingAction::BackgroundTask` and bind Ctrl+B to it only when shell running in the controller.

- [ ] **Step 4: Implement detach**

`detach_shell_command()`:

1. Reads the active shell command id and foreground task id.
2. Calls `BackgroundTaskManager::detach(task_id)` through the shell executor.
3. Emits/finalizes `ShellCommandFinished { outcome: Backgrounded { task_id } }`.
4. Appends `AgentMessage::ShellCommand { outcome: Backgrounded { task_id }, ... }`.
5. Sets `shell_running = false`.
6. Drains queue.

- [ ] **Step 5: Verify**


Expected: PASS.

---

## Task 16: Esc/Ctrl+C Cancellation Through Shared Runner

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Test: interactive controller tests

- [ ] **Step 1: Write tests**

Add:

```rust
#[tokio::test]
async fn esc_cancels_running_shell_command() {
    let cancel_calls = Arc::new(Mutex::new(0));
    let calls = Arc::clone(&cancel_calls);
    let mut controller = controller_with_running_shell("bash-1", move |_task_id| {
        *calls.lock().unwrap() += 1;
        Ok(())
    });
    controller.handle_input_event(InputEvent::Cancel).await.expect("esc");
    assert_eq!(*cancel_calls.lock().unwrap(), 1);
    assert!(!controller.tui.chrome().shell_running());
}
```

- [ ] **Step 2: Run failing test**


Expected: failure until cancel path exists.

- [ ] **Step 3: Implement cancel path**

At the start of `handle_cancel_input()` and `handle_interrupt_input()`:

```rust
if self.tui.chrome().shell_running() {
    self.cancel_shell_command().await?;
    return Ok(false);
}
```

`cancel_shell_command()` cancels via the active shell command's `CancellationToken` or executor handle. It must not only abort a Rust task; it must trigger the shared Bash runner's process-group cleanup.

- [ ] **Step 4: Persist cancelled result**

On cancellation, append `AgentMessage::ShellCommand { outcome: Cancelled, ... }` with any partial output captured so far.

- [ ] **Step 5: Verify**


Expected: PASS.

---

## Task 17: Non-Steerable Shell Commands And Alt+Up Editing

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Test: interactive controller tests

- [ ] **Step 1: Write tests**

Add:

```rust
#[tokio::test]
async fn ctrl_s_in_shell_mode_does_not_steer_shell_text() {
    let mut controller = running_turn_controller().await;
    controller.tui.chrome_mut().enter_shell_mode();
    controller.replace_prompt_text("whoami");
    controller.handle_prompt_steer().await.expect("steer");
    assert_eq!(controller.tui.chrome().prompt().text, "whoami");
}

#[tokio::test]
async fn alt_up_edits_latest_shell_command_before_followups() {
    let mut controller = running_turn_controller().await;
    controller.tui.chrome_mut().pending_input_mut().queue_follow_up("chat follow-up");
    controller.tui.chrome_mut().pending_input_mut().queue_shell_command("echo two");
    controller.handle_keybinding_action(KeybindingAction::EditLastQueuedMessage).await.expect("edit");
    assert!(controller.tui.chrome().shell_mode_active());
    assert_eq!(controller.tui.chrome().prompt().text, "echo two");
}
```

- [ ] **Step 2: Run failing tests**


Expected: failure until behavior is added.

- [ ] **Step 3: Implement Ctrl+S guard**

If `shell_mode_active()`, `handle_prompt_steer()` returns without changing prompt text or queues.

- [ ] **Step 4: Implement Alt+Up shell edit priority**

In the `EditLastQueuedMessage` action:

1. Pop most recent shell command first, set prompt text, enter shell mode.
2. Else pop most recent follow-up, set prompt text, exit shell mode.

- [ ] **Step 5: Verify**


Expected: PASS.

---

## Task 18: Session Resume Replay

**Files:**
- Modify: `crates/neo-tui/src/transcript/pane.rs` or `entry.rs`
- Modify: `crates/neo-agent/src/modes/interactive.rs` only if replay dispatch needs to route structured shell messages specially
- Test: existing session replay tests in `interactive.rs`

- [ ] **Step 1: Write tests**

Add replay test:

```rust
#[test]
fn replay_shell_command_message_renders_shell_transcript() {
    let loaded = LoadedSessionTranscript::new(
        "session_1",
        Vec::new(),
        [AgentMessage::shell_command(
            "echo hi",
            "hi\n",
            "",
            Some(0),
            ShellCommandOutcome::Completed,
        )],
    );
    let mut transcript = TranscriptPane::default();
    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript_snapshot(&mut transcript);
    assert!(rendered.contains("$ echo hi"));
    assert!(rendered.contains("hi"));
    assert!(!rendered.contains("<bash-input>"));
}
```

Use local snapshot helpers already present in `interactive.rs` tests.

- [ ] **Step 2: Run failing test**


Expected: failure until replay handles the structured variant.

- [ ] **Step 3: Render structured message**

Update `TranscriptPane::replay_message()` or lower-level entry rendering to match `AgentMessage::ShellCommand` and create a shell-run transcript entry in finished/backgrounded state.

Do not parse XML from normal text messages.

- [ ] **Step 4: Verify JSONL replay**

`neo_agent_core::session::replay_messages()` already collects `MessageAppended` events. Ensure it returns `AgentMessage::ShellCommand` unchanged.

- [ ] **Step 5: Verify**


Expected: PASS.

---

## Task 19: Prompt History Exclusion

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Test: existing prompt history tests in `interactive.rs`

- [ ] **Step 1: Write test**

Add:

```rust
#[tokio::test]
async fn shell_commands_are_not_added_to_prompt_history() {
    let history = test_prompt_history_store();
    let mut controller = controller_with_prompt_history(history.clone());
    controller.tui.chrome_mut().enter_shell_mode();
    controller.replace_prompt_text("whoami");
    controller.submit_current_prompt().await.expect("submit");
    assert!(history.entries().await.expect("entries").is_empty());
}
```

Use existing prompt-history test helpers.

- [ ] **Step 2: Run failing/passing guard**


Expected: PASS if Task 14 routed shell commands around `append_prompt_history`; otherwise fix.

- [ ] **Step 3: Fix if needed**

Ensure only normal chat prompt submission calls `append_prompt_history()`. Do not add shell commands to history as raw text.

---

## Task 20: Integration Tests

**Files:**
- Test: `crates/neo-agent/tests/shell_mode.rs` or inline interactive tests
- Test: `crates/neo-tui/tests/shell_mode_integration.rs`

- [ ] **Step 1: TUI lifecycle test**

Create a TUI test that renders:

1. Normal prompt.
2. Shell prompt after `enter_shell_mode()`.
3. Queued shell command preview.
4. Shell-run finished output.
5. Backgrounded shell-run notice.

- [ ] **Step 2: Controller lifecycle test**

Add a controller test that covers:

1. `!` enters shell mode.
2. Enter executes shell without model turn.
3. Result is persisted as `AgentMessage::ShellCommand`.
4. Ctrl+B detaches a running command.
5. Esc cancels a running command.
6. Queued shell commands drain FIFO.

- [ ] **Step 3: Run focused tests**

Run:

```bash
```

Expected: PASS.

---

## Task 21: Final Verification

- [ ] **Step 1: Build binary**

Run: `cargo build -p neo-agent`

Expected: build succeeds.

- [ ] **Step 2: Focused crate tests**

Run:

```bash
```

Expected: focused tests pass.

- [ ] **Step 3: Project check**

Run: `cargo fmt --all --check`

Expected: no new warnings or formatting failures.

- [ ] **Step 4: Manual smoke**

Run `cargo run -p neo-agent --` in a disposable session and verify:

1. `!` enters shell mode.
2. `echo hi` streams output and stays in shell mode.
3. `sleep 30`, then Ctrl+B moves it to `/tasks`.
4. `sleep 30`, then Esc cancels it.
5. Exit and resume; shell command transcript replays as `$ command`, not raw XML.

---

## Self-Review Checklist For Executors

- [ ] No `UserShell*` event variants were added.
- [ ] BashTool and shell mode use the same shared runner.
- [ ] Ctrl+B backgrounding is implemented and verified.
- [ ] Detaching resets timeout to the background timeout.
- [ ] Cancellation triggers process cleanup, not only task abort.
- [ ] Shell results persist as `AgentMessage::ShellCommand`.
- [ ] Resume replay uses structured messages, not XML string parsing.
- [ ] Shell commands are absent from prompt history.
- [ ] No git mutation commands were run without explicit user authorization.
