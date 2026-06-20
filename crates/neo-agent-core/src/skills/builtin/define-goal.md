---
name: define-goal
description: Help the user craft a well-specified goal and start it immediately with the StartGoal tool.
type: prompt
whenToUse: When the user asks for help writing, refining, or improving a goal or objective.
disableModelInvocation: false
---

Help the user turn their rough intention into a concrete autonomous goal.

A good objective must have:
1. A clear, verifiable end state.
2. Concrete proof that the goal is complete.
3. Explicit boundaries (what is in scope and what is not).
4. A stop rule (when to stop or escalate).

Ask clarifying questions if any of these are missing. Once the user approves the wording, **call the `StartGoal` tool** with the final objective (and optional completion_criterion). Do not ask the user to run `/goal` manually.

When calling `StartGoal`, use:
- `objective`: the concise, actionable goal statement.
- `completion_criterion`: a short sentence describing how to verify the goal is done.
