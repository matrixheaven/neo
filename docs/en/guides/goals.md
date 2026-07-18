# Goal mode

Goal Mode lets Neo treat a **verifiable objective** as session-level state and autonomously drive it across multiple turns — until it is complete, blocked, or you pause it.

## What is goal mode

In a normal conversation each turn is an independent request/response. In goal mode, Neo maintains a **persistent goal record** (objective, completion criterion, phased plan, status) and at the end of each turn decides whether to keep going, mark it complete, or report it blocked.

| Element | Purpose |
| --- | --- |
| **objective** | The goal description; must have a checkable terminal state |
| **completion_criterion** | Completion check, e.g. "`cargo test` fully passes" |
| **phases** | An ordered list of phases, each a self-contained milestone |
| **status** | `active` / `paused` / `blocked` / `complete` / `queued` |
| **artifact_dir** | Directory holding goal-related artifacts (phase files, etc.) |

The goal record is persisted under the session directory and restored together with the session.

## /goal command

`/goal` is the user-facing entry point for managing goals. Common usage:

| Command | Effect |
| --- | --- |
| `/goal <objective>` | Create/replace the current goal directly |
| `/goal` or `/goal status` | View the current goal status, elapsed time, queue length |
| `/goal pause` | Pause the current goal (resumable) |
| `/goal resume` | Resume a paused or blocked goal |
| `/goal cancel` | Cancel the current goal |
| `/goal replace <objective>` | Replace the current goal with a new one |
| `/goal next <objective>` | Enqueue a goal (starts immediately if none is active) |
| `/goal next manage` | View queued goals |

You can also let the AI draft a structured goal through an `EnterPlanMode`-style conversation, then submit it to you for approval via the `ExitGoalMode` tool.

## Goal lifecycle

There are two equivalent creation paths in goal mode: **AI drafts → user approves**, or **user `/goal` directly**.

```text
          ┌──────────────┐
   /goal  │   Draft      │  AI drafts via conversation
 ───────▶ │  (authoring) │  objective / criterion / phases
          └──────┬───────┘
                 │ ExitGoalMode
                 ▼
          ┌──────────────┐   Reject    ┌────────┐
          │   Implement  │ ──────────▶ │ Draft  │
          │   (active)   │              └────────┘
          └──────┬───────┘
                 │ UpdateGoalStatus
                 ▼
          ┌──────────────┐
          │    Audit     │  complete / blocked / paused
          └──────────────┘
```

| Stage | Status | Who drives |
| --- | --- | --- |
| **Draft** | goal mode authoring | AI drafts; user can Revise/Reject |
| **Implement** | `active` | The runtime drives it continuously, deciding at each turn-end whether to continue |
| **Audit** | `complete` / `blocked` / `paused` | Switched by the AI calling `UpdateGoalStatus` |

> Goal review displays **objective**, **completion criterion**, and **phases**. **Approve** starts the durable goal; **Reject** and **Revise create no goal** (Revise returns feedback while goal authoring stays active). In **Auto**, `ExitGoalMode` skips the review and the goal starts immediately; in **Ask / Yolo**, the user must Approve / Reject / Revise in a blocking dialog.

After each turn, if the goal is still `active`, the runtime automatically injects a goal-continuation system message prompting Neo to keep going; if the goal is complete and the queue still has goals, the next one starts automatically.

## Tool overview

| Tool | Caller | Effect |
| --- | --- | --- |
| `StartGoal` | AI | Start a persistent goal directly (when the user asks explicitly) |
| `ExitGoalMode` | AI | Submit the drafted structured goal to the user for approval |
| `GetGoalStatus` | AI | Read the current goal snapshot |
| `UpdateGoalStatus` | AI | Switch `active` / `complete` / `paused` / `blocked` |

On the user side, the `/goal` family of commands does the equivalent operations.

## Examples

### Start a goal directly

```
/goal Make cargo clippy warning-free across the whole workspace
```

Neo picks its own tools and fixes lint issues across multiple turns, deciding for itself at each turn-end whether it's done. You can `/goal status` anytime to check progress, or `/goal pause` to pause.

### Let the AI draft a structured goal

```
Design and implement a CLI subcommand neo foo; show the plan first
```

In goal mode, Neo first drafts the objective, completion_criterion, and phases, then calls `ExitGoalMode` to open a review that shows those fields. You can Approve (creates the goal), Revise with feedback (no goal created), or Reject (no goal created).

### Queue a follow-up goal

```
/goal next Add Chinese docs for the new command
```

When the current goal completes, the queued next goal starts automatically.

## Next steps

- [Plan mode](plan-mode.md) — Get plan approval before any work happens
- [Interaction mode](interaction.md) — Approval dialogs and permission modes
- [Session management](sessions.md) — Goal artifacts persist with the session
