# Built-in Tools Reference

Neo exposes a set of built-in tools to the model through the `ToolRegistry`. This document lists all built-in tools by category and their purposes, for use as a reference when writing Skills, prompts, or debugging.

Source location: [`crates/neo-agent-core/src/tools/`](../../../crates/neo-agent-core/src/tools/); canonical names come from `Tool::name()`.

## File Operations

| Tool | Purpose |
| --- | --- |
| `Read` | Read a UTF-8 text file, with support for paginated reading by line offset. |
| `Write` | Create or fully overwrite UTF-8 files inside the workspace via `files[]` (each with `path` and `content`). Prepares the whole batch before any write, approves a verified projection in Ask mode, commits files atomically in declaration order, and reports partial commits truthfully. Existing targets must be UTF-8 regular files (rejects binary, symlink, directory). No-op overwrites (content unchanged) fail the whole batch. Legacy top-level `path`/`content` is not accepted. |
| `Edit` | Apply ordered exact-text edits to existing UTF-8 files via a flat `edits[]` array. Each item owns `path`, `old`, `new`, and optional `expected_matches` (default 1). Prepares the whole batch before any write, approves a verified diff in Ask mode, commits files atomically in first-appearance order, and reports partial commits truthfully. Does not create files (use `Write`). The nested `files[]`/`replacements[]` shape is not accepted. |
| `List` | List directory contents as a two-level tree. |
| `Glob` | Match file/directory paths by glob pattern, sorted by modification time. |
| `Find` | Locate workspace paths by a substring of their file or directory name. |
| `Grep` | Search the contents of workspace text files using regular expressions. |

### Edit staging and commit contract

`Edit` accepts a flat `edits[]` array in declaration order. Each item contains
`path`, `old`, `new`, and optional `expected_matches` (default `1`). Edits to
the same path are grouped and applied in declaration order against staged
content, so later edits see earlier staged results. The first appearance of a
path establishes file presentation and commit order.

Before any write, Neo resolves every target, reads every existing UTF-8 regular
file without following link-like targets, applies the whole ordered batch in
memory, verifies exact match counts, and builds the approval diff. Any prepare
error fails the whole call with zero writes. In Ask mode the user approves that
verified diff. Neo then rechecks the resolved targets and contents; a stale
target before the first commit also produces zero writes.

Files commit in first-appearance order. Each file write is atomic, but the batch
is not a cross-file transaction: after a file commits, a later stale check, I/O
failure, durability failure, or cancellation does not roll it back. Structured
results distinguish `committed`, `prepare_failed`, `stale`, `cancelled`,
`commit_failed`, `partial_commit`, and `durability_uncertain`, with per-file
`committed`, `committed_unsynced`, `failed`, or `not_attempted` states.

Cancellation before the first commit writes nothing. During commit it prevents
the next file from starting and never interrupts an in-progress atomic replace.
`durability_uncertain` means the requested contents were installed but
parent-directory durability could not be confirmed. Re-read affected files and
submit a fresh `Edit`; Neo never blindly retries or rolls back a partial batch.
Use `Write` for file creation or full-file replacement.

### Write staging and commit contract

`Write` accepts `files[]` in declaration order. Each file contains `path` and
`content`. Files that do not exist are created (missing parent directories are
created during commit); existing files are completely overwritten.

Before any write, Neo resolves every target, classifies each as created or
overwritten, rejects non-UTF-8 existing files, symlinks, reparse points,
directories, no-op overwrites (content identical to current), and duplicate
resolved targets, then builds the approval projection (line-numbered content for
created files, unified diff for overwritten files). Any prepare error fails the
whole call with zero writes and zero directory creation. In Ask mode the user
approves that verified projection. Neo then rechecks every target fingerprint;
a stale or appeared target before the first commit produces zero writes.

Files commit in declaration order. Each file install is atomic (create-new or
strict-replace), but the batch is not a cross-file transaction: after a file
commits, a later stale check, I/O failure, durability failure, or cancellation
does not roll it back. Structured results distinguish `committed`,
`prepare_failed`, `stale`, `cancelled`, `commit_failed`, `partial_commit`, and
`durability_uncertain`, with per-file `committed`, `committed_unsynced`,
`failed`, or `not_attempted` states. Results report `created_directories` for
any parent directories created during commit.

Cancellation before the first commit writes nothing. During commit it prevents
the next file from starting and never interrupts an in-progress atomic install.
`durability_uncertain` means the requested contents were installed but
parent-directory durability could not be confirmed. Re-read affected files and
submit a fresh `Write`; Neo never blindly retries or rolls back a partial batch.

## Shell

| Tool | Purpose |
| --- | --- |
| `Bash` | Execute `bash` (Git Bash on Windows) commands in the workspace; supports pipes, background tasks, optional `timeout_secs`, and cancellation. Omit `timeout_secs` for no timeout; explicit values must be `300..=3600`. After a timeout, increase or double it and retry. If it is already `3600` or duration is uncertain, omit it. |
| `Terminal` | Drive a real PTY session: start / write / read / resize / stop. Suited to long-running interactive processes. `start` / `write` / `read` share one optional `yield_time_ms` (defaults 250 / 250 / 3000 ms, range `0..=30000`) that waits for incremental **raw PTY** output after admission and operation readiness; expiry returns current output with `status: running` and never stops the command. Admission queue wait stays unbounded and keeps the tool call pending. `timeout_secs` is valid only for `mode=start`; omit it for no command deadline, otherwise use `300..=3600`. After a timeout, increase or double it and retry. If it is already `3600` or duration is uncertain, omit it. Echo, ANSI, CR, and cursor control are not filtered. For `write`, `input` is a non-empty ordered array such as `[{"text":"command text"},{"control":3}]`: `text` sends UTF-8 with LF and CRLF normalized to CR, while `control` sends the exact byte `0..=31` or `127` (Ctrl+C `3`, Ctrl+D `4`, Ctrl+Z `26`, Escape `27`). Parts are sent in array order by one tool call; `{"text":"\\u0003"}` sends the printable escape text literally. Exact PTY control bytes do not guarantee portable signal behavior: the receiving application decides their meaning, Windows ConPTY behavior is receiver-dependent, and remote sessions should use `ssh -tt` when PTY allocation is uncertain. |

## Network

| Tool | Purpose |
| --- | --- |
| MCP tools | Dynamically registered, named in the form `mcp__<server_id>__<tool_name>`, and managed by `mcp_manager.rs`. Not built-in. |

> Neo's built-in toolset does not provide an HTTP fetching tool directly. Network access is available through `Bash` (`curl`/`wget`) or a user-configured MCP server.

## Plan Mode

| Tool | Purpose |
| --- | --- |
| `EnterPlanMode` | Enter plan mode (read-only research / planning) without modifying code directly. |
| `ExitPlanMode` | Exit plan mode once the plan is written and request user approval. |

## Goals

Registered by `GoalManager`; available when goal mode is enabled.

| Tool | Purpose |
| --- | --- |
| `StartGoal` | Start a structured goal that persists across multiple turns. |
| `ExitGoalMode` | Goal draft review is complete; submit it for user approval. |
| `UpdateGoalStatus` | Update the current goal status (resume / end / yield). |
| `GetGoalStatus` | Read the current goal: objective, completion criteria, status, and turns consumed. |

## Multi-Agent Collaboration (Delegate / Swarm)

| Tool | Purpose |
| --- | --- |
| `Delegate` | Delegate a bounded subtask to a sub-agent; by default waits in the foreground for the result. |
| `DelegateSwarm` | Dispatch multiple related subtasks in parallel and aggregate ordered results. |
| `ListDelegates` | List sub-agents / swarms and their current status. |
| `WaitDelegate` | Wait for all delegate/swarm IDs in `ids` to reach terminal states under one global timeout; timeout results retain completed and unfinished item snapshots. |
| `InterruptDelegate` | Interrupt and cancel a running delegate/swarm. |
| `MessageDelegate` | Send a message to a running delegate. |

## Background Task Management

| Tool | Purpose |
| --- | --- |
| `TaskList` | List background tasks and their status. |
| `TaskOutput` | Retrieve the output of a running or completed background task. Prefer `block=true` when waiting for a known task to finish. |
| `TaskStop` | Stop a running background task. |
| `TaskPause` | Request that a running workflow pause at its next durable invocation boundary; the active child finishes first. |
| `TaskResume` | Resume a paused workflow by replaying matching journaled invocations before continuing live work. |

## Timing

| Tool | Purpose |
| --- | --- |
| `Sleep` | Pause this agent for a genuine time-based wait (`duration_seconds` 1..=3600) without starting a shell command or consuming shell admission. Prefer `WaitDelegate` for a known agent/swarm and `TaskOutput` with `block=true` for a known background task. |

## Other

| Tool | Purpose |
| --- | --- |
| `TodoList` | Maintain a structured task list (pending / in_progress / done). |
| `Skill` | Invoke an available skill by name + arguments (provided by `SkillStore`). |
| `AskUserQuestion` | Ask the user a question with structured options during execution. |
| `CreateSkill` | Create a new skill at `~/.neo/skills/<name>/SKILL.md`. |
| `MoveSkill` | Move a skill directory into its parent bundle, automatically generating a timestamped backup. |
| `RunWorkflow` | Start a reviewed Lua workflow in the background. Its model input is exactly `name`, `description`, `phases`, `script`, and `args`; machine limits are runtime configuration, not model input. |
| `ListSkills` | List all discoverable skills (user / extra / builtin). |
| `SummarizeSessions` | Read and summarize a local session transcript, useful for distilling it into a skill. |

`RunWorkflow` requires the exact `/workflow` command to grant one launch capability. The launch is always background and returns a `run_id`, which is also its task ID. Use `TaskOutput` to inspect journal-backed status/output, `TaskPause` and `TaskResume` for boundary-safe control, and `TaskStop` to cancel. A pause waits for the active child invocation to finish. Ask / Auto / Yolo govern each child effect through the ordinary tool permission path; launch approval never bypasses child approval.

Workflow token handling uses actual provider usage only. There is no default token cap or wall-clock timeout, and Neo never predicts project cost, token use, duration, or agent count to pause or degrade a workflow. Historical session workflow cards remain readable, but sessions without durable workflow files cannot be resumed as workflows.

## Sub-agent Toolset

Derived agents (`Delegate` / `DelegateSwarm`) register only a subset by default, built via `ToolRegistry::with_builtin_child_tools()`:

`Read` · `List` · `Grep` · `Find` · `Glob` · `TodoList` · `Write` · `Edit` · `Bash` · `TaskList` · `TaskOutput` · `TaskStop` · `Terminal` · `EnterPlanMode` · `ExitPlanMode` · `RunWorkflow` · `Sleep`

In addition, `AgentProfile::for_role` filters by a role-specific whitelist, and any custom tools explicitly registered by the caller are always passed through.

## Permission Model Cheat Sheet

Tool execution is governed by `ToolAccess`, which controls three permission types: `file_read` / `file_write` / `shell`. External dispatch behavior — whether an approval panel is shown — is determined by the `PermissionMode` (Ask / Auto / Yolo) carried in the `ToolContext`.
