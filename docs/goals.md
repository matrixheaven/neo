# Goals

Goals let Neo work autonomously toward a defined outcome across multiple turns.
Unlike a normal prompt, a goal describes what must become *true*, not just what
to do next.

## Start a goal directly

```text
/goal Fix every failing checkout test and run the checkout test suite successfully
```

Neo saves the objective directly as a durable goal and sends the objective as
the user message. This path is intentionally manual: `/goal <prompt>` remains
the user's way to write the goal text themselves instead of asking the model to
draft it first. After each turn, Neo checks whether the goal is `complete`,
`blocked`, or still `active`. If active, it continues automatically until the
model marks it complete or blocked.

## Goal mode

Goal mode is the AI-assisted authoring path. Shift+Tab cycles the development
mode badge through normal, `[plan]`, and `[goal]`; permissions remain separate
and still show as `[manual]`, `[auto]`, or `[yolo]`.

In goal mode, the model drafts a structured goal before any durable goal starts.
The draft should include the objective, acceptance criteria, phase plan,
risks/assumptions, and verification commands. The model then calls
`ExitGoalMode`, which opens a blocking review dialog:

- **Accept** creates the durable goal and starts ordinary goal continuation.
- **Reject** cancels the draft and does not start a goal.
- **Revise** keeps goal mode active and returns the feedback to the current
  `ExitGoalMode` tool result so the model can revise in the same turn flow.

The footer goal badge reflects durable goal state:

- `[goal]` — goal authoring mode is open, but no durable goal is running.
- `[goal•]` — an active goal is running.
- `[goal◌]` — the goal is paused.
- `[goal✗]` — the goal is blocked.

## Manage the lifecycle

| Command | Action |
| --- | --- |
| `/goal` or `/goal status` | Show the current goal |
| `/goal pause` | Pause the active goal |
| `/goal resume` | Resume a paused or blocked goal |
| `/goal cancel` | Remove the current goal |
| `/goal replace <objective>` | Replace the current goal |
| `/goal next <objective>` | Queue an upcoming goal |

A goal stops in one of three ways:

- **complete**: the objective is done; Neo clears the goal and summarizes the work.
- **paused**: you paused it, or a runtime error/interrupt occurred.
- **blocked**: Neo needs input, the goal is impossible as stated, or the turn
  budget was exceeded. The reason is shown in the transcript.

## Turn budget

Goals run with a default maximum of 30 autonomous turns. When the budget is
exceeded, the goal is marked blocked. This prevents runaway work on vague
objectives.

## Goal tools

- `StartGoal` — begin a new autonomous goal from an explicit user-controlled
  `/goal <prompt>` or other direct goal request.
- `ExitGoalMode` — approve a structured goal draft and start the durable goal.
- `GetGoalStatus` — check the active goal and turn count.
- `UpdateGoalStatus` — mark the goal as `complete`, `blocked`, or `active`.

## Storage

Goals are stored as JSON files in `~/.neo/goals/`. Each structured run also has
an artifact directory under `~/.neo/goals/runs/<goal-id>/` containing
`GOAL.md`, `ROADMAP.md`, `STATE.md`, `THINKING.md`, `PROTOCOL.md`, and
`phases/phase-N.md`. Active, paused, and blocked goals persist across sessions.
Completed goals are removed.
