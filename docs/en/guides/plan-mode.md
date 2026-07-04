# Plan mode

Plan Mode lets Neo investigate the codebase with read-only tools, write up a plan, and hand it to you for approval before it changes any code. It suits tasks with an uncertain path, multiple viable options, or multi-file changes.

## Plan mode concepts

Once in plan mode, Neo **can only use read-only tools** (Read / Grep / Glob, etc.) to investigate, and is allowed to write to the **plan file**; other file writes and shell commands are forbidden until plan mode is exited.

```text
  EnterPlanMode  ──▶  Read-only exploration + write plan file  ──▶  ExitPlanMode ──▶ Approval
                                                                          │
                                                  ┌───────────────────────┼───────────────────────┐
                                                  ▼                       ▼                       ▼
                                               Approve                Revise                  Reject
                                                  │                       │                       │
                                            Continue              Back to editing the plan    Cancel the plan
```

## Entering / exiting plan mode

| Method | Action |
| --- | --- |
| Slash command | `/plan`, `/plan on`, `/plan off` |
| Clear the plan file | `/plan clear` |
| Shortcut | `Shift+Tab` cycles Normal → Plan → Goal |
| AI-triggered | Neo calls the `EnterPlanMode` tool itself (good for non-trivial implementation tasks) |

`EnterPlanMode` enters **in every permission mode** without an approval dialog. Once inside, the status bar shows `Plan Mode On`.

## Approval flow

When Neo finishes the plan and calls `ExitPlanMode` (in Ask/YOLO mode), an approval dialog appears:

| Option | Meaning |
| --- | --- |
| **Approve** | Approve the plan, exit plan mode, and start executing |
| **Reject** | Reject the plan, exit plan mode, but do not execute |
| **Revise** | Reject with feedback; Neo rewrites the plan file based on your note and resubmits |

`ExitPlanMode` can also carry:

- **`plan_summary`**: a short overview of the plan (the plan body itself should already be written to the plan file).
- **`options`**: up to 3 alternative plans, each with a label and description; the recommended option's label gets a `(Recommended)` suffix and is placed first. The system automatically appends Reject / Revise controls.
- **`suggestions`**: up to 5 preset "revision suggestions"; selecting one auto-fills the feedback text.

> Reserved labels: `approve` / `reject` / `revise` / `reject and exit` cannot be used as option labels.

### Behavior by permission mode

| Permission mode | `EnterPlanMode` | `ExitPlanMode` |
| --- | --- | --- |
| **Ask** | Enter directly | Pops approval dialog |
| **YOLO** | Enter directly | Pops approval dialog |
| **Auto** | Enter directly | **No dialog** — exits plan mode and starts executing |

## Plan file

The "plan body" in plan mode is a **plan file** stored in the `plans/` subdirectory of the current session:

```
<session-bucket>/agents/main/plans/<plan-file>
```

Neo uses `Write` / `Edit` to write the plan into that file; `ExitPlanMode` takes no plan-content argument — it reads the plan file you wrote and shows it to the user. `/plan clear` clears the current plan file.

After exiting plan mode, the approved plan becomes context for the following execution turns, and Neo starts making the actual code changes from it.

## When to use, when not to

| Scenario | Recommendation |
| --- | --- |
| New feature, architectural decision, multi-file change | ✅ Use plan mode |
| Multiple viable options that need your call | ✅ Use it, paired with `options` |
| An obvious one/two-line fix or a typo | ❌ Just do it |
| The user already gave detailed steps | ❌ Just execute |
| Pure investigation / understanding code | ❌ Use read-only tools directly; no `ExitPlanMode` needed |

## Next steps

- [Goal mode](goals.md) — Autonomously drive a verifiable objective across multiple turns
- [Interaction mode](interaction.md) — Approval dialogs and permission modes
- [Use-case recipes](use-cases.md) — Templates for implementing features, fixing bugs, refactoring, and more
