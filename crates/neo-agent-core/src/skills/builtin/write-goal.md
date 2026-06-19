---
name: write-goal
description: Help the user craft a well-specified /goal objective with a clear finish line, proof, boundaries, and stop rule.
type: prompt
whenToUse: When the user asks for help writing, refining, or improving a goal or objective.
disableModelInvocation: false
---

Help the user turn their rough intention into a concrete `/goal` objective.

A good objective must have:
1. A clear, verifiable end state.
2. Concrete proof that the goal is complete.
3. Explicit boundaries (what is in scope and what is not).
4. A stop rule (when to stop or escalate).

Ask clarifying questions if any of these are missing. Output the final objective in the form:

```
/objective <concise statement>
Completion criterion: <how to verify>
Boundaries: <in-scope / out-of-scope>
Stop rule: <when to stop>
```
