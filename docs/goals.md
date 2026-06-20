# Goals

Goals let Neo work autonomously toward a defined outcome across multiple turns.
Unlike a normal prompt, a goal describes what must become *true*, not just what
to do next.

## Start a goal

```text
/goal Fix every failing checkout test and run the checkout test suite successfully
```

Neo saves the objective, starts goal mode, and sends the objective as the user
message. After each turn, it checks whether the goal is `complete`, `blocked`,
or still `active`. If active, it continues automatically until it completes,
blocks, or hits the turn budget.

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

- `StartGoal` — begin a new autonomous goal (used by the `define-goal` skill).
- `GetGoalStatus` — check the active goal and turn count.
- `UpdateGoalStatus` — mark the goal as `complete`, `blocked`, or `active`.

## Storage

Goals are stored as JSON files in `~/.neo/goals/`. Active, paused, and blocked
goals persist across sessions. Completed goals are removed.
