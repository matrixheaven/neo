# Built-in Tools Reference

Neo exposes a set of built-in tools to the model through the `ToolRegistry`. This document lists all built-in tools by category and their purposes, for use as a reference when writing Skills, prompts, or debugging.

Source location: [`crates/neo-agent-core/src/tools/`](../../../crates/neo-agent-core/src/tools/); canonical names come from `Tool::name()`.

## File Operations

| Tool | Purpose |
| --- | --- |
| `Read` | Read a UTF-8 text file, with support for paginated reading by line offset. |
| `Write` | Create or fully overwrite a UTF-8 file inside the workspace. |
| `Edit` | Apply ordered exact replacements across existing UTF-8 regular files via `files[]` / `replacements[]` / `expected_matches` (default 1). Prepares the whole batch before any write, approves a verified diff in Ask mode, commits files atomically in declaration order, and reports partial commits truthfully. Does not create files (use `Write`). Legacy top-level `path`/`old`/`new`/`replace_all` is not accepted. |
| `List` | List directory contents as a two-level tree. |
| `Glob` | Match file/directory paths by glob pattern, sorted by modification time. |
| `Find` | Locate workspace paths by a substring of their file or directory name. |
| `Grep` | Search the contents of workspace text files using regular expressions. |

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
| `RunWorkflow` | Run a Lua workflow script (can call `neo.delegate` / `neo.swarm`, etc.). |
| `ListSkills` | List all discoverable skills (user / extra / builtin). |
| `SummarizeSessions` | Read and summarize a local session transcript, useful for distilling it into a skill. |

## Sub-agent Toolset

Derived agents (`Delegate` / `DelegateSwarm`) register only a subset by default, built via `ToolRegistry::with_builtin_child_tools()`:

`Read` · `List` · `Grep` · `Find` · `Glob` · `TodoList` · `Write` · `Edit` · `Bash` · `TaskList` · `TaskOutput` · `TaskStop` · `Terminal` · `EnterPlanMode` · `ExitPlanMode` · `RunWorkflow` · `Sleep`

In addition, `AgentProfile::for_role` filters by a role-specific whitelist, and any custom tools explicitly registered by the caller are always passed through.

## Permission Model Cheat Sheet

Tool execution is governed by `ToolAccess`, which controls three permission types: `file_read` / `file_write` / `shell`. External dispatch behavior — whether an approval panel is shown — is determined by the `PermissionMode` (Ask / Auto / Yolo) carried in the `ToolContext`.
