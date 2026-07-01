# Shell Mode (`!`) — Design Spec

## Overview

Add a shell mode to neo's TUI: pressing `!` at an empty prompt enters a composer mode where the user can run shell commands directly, with live streaming output in the transcript. The design is based on `docs/kimi-code`, with one intentional product difference: neo does **not** auto-exit shell mode after sending a command, so consecutive commands are fast.

This feature must be implemented as a single shell execution path. User shell commands, model-initiated Bash tool calls, cancellation, backgrounding, process cleanup, timeout handling, streaming output, and transcript events all use the same shared Bash execution machinery. Do not add a parallel `UserShell*` event family or a second process runner.

## Goals

- Press `!` at an empty prompt to enter shell mode.
- Type a command and press Enter to execute it directly without starting a model turn.
- Stay in shell mode after sending a command.
- Stream stdout/stderr live into the transcript with ANSI/control-sequence sanitization.
- Queue shell commands while an AI turn, compaction, or another shell command is busy.
- Support Ctrl+B backgrounding as a required v1 feature.
- Reset the timeout to the background timeout when a foreground shell command is detached.
- Support Esc/Ctrl+C cancellation through the same process-group cleanup path as Bash.
- Inject completed/backgrounded shell command results into durable conversation context for future AI turns.
- Replay shell commands and output correctly on session resume.
- Keep shell commands out of normal prompt history.
- Use a cyan shell-mode theme, distinct from neo's brand magenta.

## Non-Goals

- No permission prompt for shell-mode commands. The user explicitly typed the command.
- No model turn is triggered by a shell command.
- Shell commands are not steerable. Ctrl+S ignores queued shell commands and only promotes queued chat follow-ups.
- No raw XML/string parsing for replay semantics. Shell replay is driven by structured persisted data.

---

## 1. UX State Machine

Shell mode is a composer input mode, not a development mode. It is independent of `normal`, `plan`, and `goal`.

### TUI state

`NeoChromeState` gains:

```rust
shell_mode_active: bool,
shell_running: bool,
```

`shell_mode_active` controls prompt rendering and Enter behavior. The `!` prefix is visual only and is never stored in the prompt buffer.

`shell_running` controls the working label, spinner, queue behavior, Ctrl+B backgrounding, and Esc/Ctrl+C cancellation.

### Enter

When the prompt buffer is empty, chrome is editing, and no blocking overlay/dialog owns input:

```text
Insert('!') -> enter_shell_mode(), do not insert '!'
Paste("!cmd") -> enter_shell_mode(), insert "cmd"
```

### Exit

| Trigger | Behavior |
| --- | --- |
| Enter with non-empty shell buffer | Execute or queue command; stay in shell mode |
| Backspace with empty shell buffer | Exit shell mode |
| Esc with empty shell buffer and no command running | Exit shell mode |
| Esc/Ctrl+C while command running | Cancel command; stay in shell mode |

---

## 2. UI Rendering

### Prompt

Normal prompt remains:

```text
> hello
```

Shell mode renders:

```text
! whoami
```

The actual buffer is `whoami`. Cursor movement cannot move into the rendered `! ` prefix.

### Border and footer

Shell mode uses `theme.shell_mode` (`Color::Rgb(86, 182, 194)`) for:

- Prompt prefix.
- Prompt border.
- Top-border label `! shell mode`.
- Footer badge `[shell]`.
- Transcript `$ command` echo.
- Queued shell command preview.

Footer order:

```text
[yolo] [shell] GLM-5.2 ~/Workspace/neo main
```

### Transcript

Running shell command:

```text
$ whoami
  chenyuanhao
  (3s · ctrl+b to background)
```

Finished success:

```text
$ whoami
  chenyuanhao
```

Finished error:

```text
$ ls /nope
  ls: /nope: No such file or directory
```

Backgrounded:

```text
$ long-running-command
  Moved to background. Use /tasks to view.
```

Rendering uses a dedicated `ShellRunComponent` rather than model-tool chrome. Model Bash tool calls continue to render as tool calls; user shell-mode commands render as `$ command` transcript entries.

---

## 3. Single Execution Path

Neo already has:

- `AgentEvent::ShellCommandStarted`
- `AgentEvent::ShellCommandFinished`
- `AgentEvent::ToolExecutionUpdate`
- Bash process-group cleanup in `crates/neo-agent-core/src/tools/bash.rs`
- background task support in `BackgroundTaskManager`

Shell mode must extend/reuse these instead of introducing `UserShellStarted`, `UserShellOutput`, or `UserShellFinished`.

### Shared Bash runner

Extract Bash execution into a reusable runner owned by `crates/neo-agent-core/src/tools/bash.rs`.

The runner accepts:

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

pub enum ShellCommandOrigin {
    ModelBashTool,
    UserShellMode,
}
```

The runner returns:

```rust
pub struct ShellExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub truncated: bool,
    pub outcome: ShellCommandOutcome,
}

pub enum ShellCommandOutcome {
    Completed,
    Cancelled,
    TimedOut,
    Backgrounded { task_id: String },
}
```

BashTool and shell mode both call this runner. BashTool keeps permission checks and model-tool semantics outside the runner; shell mode bypasses permission checks because the command is explicitly user-entered.

### Events

Keep the existing shell event names and add origin/outcome information to them:

```rust
ShellCommandStarted {
    turn: u32,
    id: String,
    command: String,
    cwd: PathBuf,
    origin: ShellCommandOrigin,
}

ShellCommandFinished {
    turn: u32,
    id: String,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    truncated: bool,
    origin: ShellCommandOrigin,
    outcome: ShellCommandOutcome,
}
```

Live output chunks continue to use `ToolExecutionUpdate` for the same command id. TUI routes updates by id into the running `ShellRunComponent` when `origin == UserShellMode`; model Bash updates continue to update the tool call card.

### Timeout

Shell-mode foreground timeout:

```rust
const SHELL_FOREGROUND_TIMEOUT: Duration = Duration::from_secs(120);
```

Background timeout:

```rust
const DEFAULT_BACKGROUND_TIMEOUT: Duration = Duration::from_secs(600);
```

Ctrl+B detaches the foreground command into `BackgroundTaskManager` and resets the command deadline to the background timeout counted from detach time. This is required v1 behavior, not a follow-up.

### Cancellation

Esc/Ctrl+C cancels with `CancellationToken`. The shared runner must terminate the spawned bash process using the same process-group cleanup path as BashTool:

1. Signal the running command.
2. Kill the process group when available.
3. Drain stdout/stderr briefly.
4. Emit `ShellCommandFinished { outcome: Cancelled, ... }`.

Aborting a Rust `JoinHandle` alone is not sufficient.

---

## 4. Queueing

`PendingInputState` gains:

```rust
queued_shell_commands: VecDeque<String>
```

Queue rules:

| State | Enter in shell mode |
| --- | --- |
| Idle | Execute immediately |
| AI turn running | Queue shell command |
| Shell command running | Queue shell command |
| Compaction running | Queue shell command |

Drain order after a shell command finishes:

1. Oldest queued shell command.
2. Oldest queued follow-up chat message.
3. Idle.

Editing uses LIFO:

1. Alt+Up pops the most recent queued shell command and re-enters shell mode.
2. If no shell command exists, it pops the most recent queued follow-up and exits shell mode.

Ctrl+S never promotes shell commands. It only steers current prompt text or queued follow-up messages.

---

## 5. Structured Context And Persistence

Shell command history must be durable and replayable without parsing XML from arbitrary text.

Add a structured message variant:

```rust
AgentMessage::ShellCommand {
    command: String,
    stdout: String,
    stderr: String,
    exit_code: Option<i32>,
    outcome: ShellCommandOutcome,
}
```

Provider request conversion maps this variant to a user-role text message:

```xml
<bash-input>
escaped command
</bash-input>
<bash-stdout>
escaped stdout
</bash-stdout>
<bash-stderr>
escaped stderr
</bash-stderr>
<bash-status exit_code="0" outcome="completed" />
```

The XML exists only at the model boundary. Internal replay and session semantics use the structured Rust enum variant.

### When to append

On command completion, cancellation, timeout, or background detach, the controller emits and persists:

```rust
AgentEvent::MessageAppended {
    message: AgentMessage::ShellCommand { ... },
}
```

That event updates:

- `AgentContext`
- `InteractiveController::session_messages`
- JSONL session file
- future model context

No model turn starts when this message is appended.

### Resume replay

`TranscriptPane::replay_message()` handles `AgentMessage::ShellCommand` directly:

- Render `$ command` in cyan.
- Render stdout dim.
- Render stderr red when `exit_code != Some(0)` or outcome is cancelled/timed out.
- Render backgrounded metadata as "Moved to background. Use /tasks to view."

No replay code should search for `<bash-input>` or `<user-shell-output>` inside normal text messages.

---

## 6. Output Sanitization

New module: `crates/neo-tui/src/utils/shell_output.rs`.

`sanitize_shell_output(raw: &str) -> String` strips:

1. OSC sequences.
2. CSI sequences.
3. single-character ESC sequences.
4. C0 controls except `\n` and `\t`.

`format_shell_output(stdout, stderr, exit_code, outcome, theme)` returns transcript lines. It caps display size so long-running commands cannot exhaust TUI memory. The live component keeps a bounded tail while the durable result stores capped details consistent with BashTool's existing output cap behavior.

---

## 7. Key Bindings

| Key | Shell mode behavior | Normal behavior |
| --- | --- | --- |
| `!` at empty prompt | Enter shell mode | Insert `!` only when prompt is not empty |
| Enter | Execute or queue command | Submit/queue chat message |
| Backspace on empty shell prompt | Exit shell mode | Existing behavior |
| Esc on empty shell prompt | Exit shell mode | Existing behavior |
| Esc/Ctrl+C while command runs | Cancel command through shared runner | Cancel AI turn |
| Ctrl+B while command runs | Detach command to background task | Existing editor/keybinding behavior |
| Alt+Up | Edit latest queued shell command first | Edit latest queued follow-up |
| Ctrl+S | No-op for shell command text; follow-up steer still works outside shell mode | Steer |
| Shift+Tab | Cycle development mode | Cycle development mode |

Ctrl+B is context-sensitive: it backgrounds only when `shell_running == true` and the active shell command has a foreground task id. Otherwise the existing keybinding behavior remains.

---

## 8. Files To Create Or Modify

### New files

| File | Purpose |
| --- | --- |
| `crates/neo-tui/src/utils/shell_output.rs` | sanitize and format shell output |
| `crates/neo-tui/src/widgets/shell_run.rs` | render user shell-mode command state |

### Modified files

| File | Changes |
| --- | --- |
| `crates/neo-agent-core/src/messages.rs` | add structured `AgentMessage::ShellCommand` and provider conversion |
| `crates/neo-agent-core/src/events.rs` | extend existing shell events with origin/outcome; do not add `UserShell*` events |
| `crates/neo-agent-core/src/tools/bash.rs` | extract shared Bash runner used by BashTool and shell mode |
| `crates/neo-agent-core/src/tools/background_tasks.rs` | add or extend required foreground detach APIs and reset the deadline to the background timeout |
| `crates/neo-agent-core/src/runtime.rs` | apply/replay `AgentMessage::ShellCommand` into context; preserve existing Bash tool event flow |
| `crates/neo-agent/src/modes/interactive.rs` | shell mode input handling, direct command dispatch, queue drain, cancellation, Ctrl+B detach, persistence |
| `crates/neo-agent/src/themes.rs` | map `shell_mode` theme key |
| `crates/neo-tui/src/shell/mod.rs` | shell mode state and working label |
| `crates/neo-tui/src/shell/pending_input.rs` | queued shell command FIFO/LIFO APIs |
| `crates/neo-tui/src/shell/theme.rs` | `shell_mode` color |
| `crates/neo-tui/src/transcript/entry.rs` | shell-run transcript entry if needed |
| `crates/neo-tui/src/transcript/event_handler.rs` | route existing shell events by origin |
| `crates/neo-tui/src/transcript/pane.rs` | prompt/footer/queue rendering |
| `crates/neo-tui/src/widgets/pending_input_preview.rs` | render queued shell commands |
| `crates/neo-tui/src/input/keybinding.rs` | expose context-sensitive background action only if current input dispatch cannot intercept Ctrl+B earlier |

---

## 9. End-To-End Flow

```text
User presses ! at empty prompt
  -> shell_mode_active = true
  -> prompt shows cyan ! prefix and ! shell mode label

User types whoami and presses Enter
  -> prompt clears, shell_mode_active stays true
  -> ShellCommandStarted { origin: UserShellMode }
  -> transcript shows "$ whoami"
  -> shared Bash runner streams ToolExecutionUpdate chunks
  -> shared Bash runner finishes
  -> ShellCommandFinished { origin: UserShellMode, outcome: Completed }
  -> MessageAppended { AgentMessage::ShellCommand { ... } }
  -> session JSONL persists the structured message
  -> next model turn sees the command/output as user context
  -> queued shell commands drain FIFO

User presses Ctrl+B while command is running
  -> foreground task detaches into BackgroundTaskManager
  -> timeout resets to 10 minutes
  -> transcript shows "Moved to background. Use /tasks to view."
  -> MessageAppended persists a ShellCommand message with outcome Backgrounded
  -> queue drains

User resumes the session
  -> JSONL replay rebuilds AgentMessage::ShellCommand
  -> TranscriptPane renders shell command/output from the structured variant
```
