# Built-in Tools Reference

Neo exposes a set of built-in tools to the model through the `ToolRegistry`. This document lists all built-in tools by category and their purposes, for use as a reference when writing Skills, prompts, or debugging.

Source location: [`crates/neo-agent-core/src/tools/`](../../../crates/neo-agent-core/src/tools/); canonical names come from `Tool::name()`.

## File Operations

| Tool | Purpose |
| --- | --- |
| `Read` | Read a UTF-8 text file, with support for paginated reading by line offset. |
| `Write` | Create or fully overwrite one UTF-8 file inside the workspace with `path` and `content`. The target is prepared and rechecked before one atomic install. Existing targets must be UTF-8 regular files; binary files, links, directories, and no-op overwrites are rejected. Emit multiple `Write` calls in one model response for independent files. |
| `Edit` | Apply one exact-text replacement to one existing UTF-8 file with `path`, `old`, `new`, and optional `expected_matches` (default 1). The target is prepared and rechecked before one atomic replace. Does not create files; emit multiple `Edit` calls in one model response for independent changes. |
| `List` | List directory contents as a two-level tree. |
| `Glob` | Match file/directory paths by glob pattern, sorted by modification time. |
| `Find` | Locate workspace paths by a substring of their file or directory name. |
| `Grep` | Search the contents of workspace text files using regular expressions. |

### Edit staging and commit contract

`Edit` accepts exactly one object: `path`, `old`, `new`, and optional
`expected_matches` (default `1`). Before writing, Neo resolves and reads the
existing UTF-8 regular file without following links, verifies the exact match
count, stages the replacement, and builds the approval diff. In Ask mode the
user approves that verified diff. Neo then rechecks the resolved target and
content before atomically replacing the file. Prepare, stale, or cancellation
failures before commit write nothing. `durability_uncertain` means the requested
content was installed but parent-directory durability could not be confirmed.
Re-read the file before a fresh call. Use `Write` for creation or full replacement.

### Write staging and commit contract

`Write` accepts exactly one object with `path` and complete UTF-8 `content`.
Before writing, Neo resolves and classifies the target, rejects unsafe or no-op
overwrites, and builds the approval projection. In Ask mode the user approves
that verified content or diff. Neo then rechecks the target before one atomic
create or replace. Missing parent directories are created only during commit.
Prepare, stale, or cancellation failures before commit write nothing. Results
report any `created_directories`; `durability_uncertain` means contents were
installed but parent-directory durability could not be confirmed.

## Shell

| Tool | Purpose |
| --- | --- |
| `Bash` | Execute non-interactive `bash` (Git Bash on Windows) commands in the workspace; standard input is closed, so use `Terminal` for prompts, terminal state, keystrokes, or control bytes. Supports pipes, background tasks, optional `timeout_secs`, and cancellation. Omit `timeout_secs` for no timeout; explicit values must be `300..=3600`. After a timeout, increase or double it and retry. If it is already `3600` or duration is uncertain, omit it. |
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
| `TaskList` | List background tasks and their status. Workflow entries may include phase, admission wait reason, and awaiting-user metadata. Supports pagination cursors rather than a hard 50-item cut. |
| `TaskOutput` | Retrieve output for a running or completed background task. Prefer `block=true` when waiting for a known task to finish. For **workflow** tasks, use explicit views (`summary`, `journal`, `result`, `artifacts`, `artifact_content`) with opaque cursors; Neo never loads a complete journal into one result. Every view exposes actionable `pending_user` fields while waiting: `request_id`, `prompt`, `answer_schema`, optional `default`, `answer_policy`, and `next_action`. |
| `TaskStop` | Stop a running background task or cancel a workflow run. |
| `TaskPause` | Request that a running workflow pause at its next durable invocation boundary; the active child finishes first. |
| `TaskResume` | Resume a paused workflow by replaying matching journaled invocations before continuing live work. Cannot answer `awaiting_user` without a typed answer. |
| `TaskAnswer` | Answer a durable workflow `awaiting_user` request with `task_id`, `request_id`, and typed `answer`, only when its policy allows the model actor. Human-only gates are answered by the user in the TUI or human CLI. |

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
| `Workflow` | Canonical assistant-native workflow tool. Its flat actions are `list`, `show`, `validate_inline`, `validate_saved`, `save`, `run_inline`, and `run_saved`; inline and saved runs return a task ID. |
| `ListSkills` | List all discoverable skills (user / extra / builtin). |
| `SummarizeSessions` | Read and summarize a local session transcript, useful for distilling it into a skill. |

### Workflow tools and control

Use `Workflow` for every assistant workflow lifecycle action. Activate
`create-workflow` when authoring guidance is useful; known saved workflows may
use `list`/`show`/`run_saved` directly. `run_inline`, `run_saved`, and `save`
validate their definitions internally. Use `validate_inline` or
`validate_saved` only when the user explicitly wants a check without running or
persisting. No route needs a slash command, capability, or CLI. Every run action
is background and returns a task ID (also the `run_id`).

| Action | How |
| --- | --- |
| Discover, validate, save, or run | `Workflow(list|show|validate_inline|validate_saved|save|run_inline|run_saved)` |
| View workflow output | `TaskOutput` with workflow views/cursors; summary never embeds full journals or large artifacts |
| Pause / resume / stop | `TaskPause`, `TaskResume`, `TaskStop` at durable boundaries |
| Answer `awaiting_user` | Follow `TaskOutput.pending_user.next_action`; call `TaskAnswer` with the exact IDs only when it says `TaskAnswer`. Resume alone is not enough. |

Child agents from workflow Lua use required per-child `output_schema` values. Invalid child JSON receives **exactly one** tools-disabled repair turn in the same child session; no fuzzy JSON extraction. Swarm fan-out is heterogeneous and has no hard-coded total child cap; host `swarm_concurrency` is default concurrency only. Ask / Auto / Yolo govern every child and tool effect; launch approval never bypasses them.

Usage accounting is **actual provider usage only**. There is no predictive token budget, agent budget, or workflow wall-clock timeout used to pause or degrade a run. Global admission is actual occupancy (VMs, workers, executors, storage). Historical cards without durable `workflows/<run_id>/` files remain readable but cannot be resumed. Full authoring guide: [Workflows](../guides/workflows.md).

## Sub-agent Toolset

Derived agents (`Delegate` / `DelegateSwarm`) register only a subset by default, built via `ToolRegistry::with_builtin_child_tools()`:

`Read` · `List` · `Grep` · `Find` · `Glob` · `TodoList` · `Write` · `Edit` · `Bash` · `TaskList` · `TaskOutput` · `TaskStop` · `Terminal` · `EnterPlanMode` · `ExitPlanMode` · `Sleep`

`Workflow` and `TaskAnswer` are root-agent-only and are not in this toolset.

In addition, `AgentProfile::for_role` filters by a role-specific whitelist, and any custom tools explicitly registered by the caller are always passed through.

## Permission Model Cheat Sheet

Tool execution is governed by `ToolAccess`, which controls three permission types: `file_read` / `file_write` / `shell`. External dispatch behavior — whether an approval panel is shown — is determined by the `PermissionMode` (Ask / Auto / Yolo) carried in the `ToolContext`.
