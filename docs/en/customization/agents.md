# Sub-agents

Neo can delegate a task to one or more independent sub-agents for concurrent execution. A sub-agent has its own role, tool set, context window, and conversation history, and returns a summary to the main agent when finished. The core implementation lives in `crates/neo-agent-core/src/multi_agent/` and `crates/neo-agent-core/src/tools/delegate.rs`.

## Sub-agent Concepts

| Concept | Description |
| --- | --- |
| **Main agent** | The top-level agent currently in conversation with the user |
| **Sub-agent** | Spawned by the `Delegate` / `DelegateSwarm` tools; has its own `AgentId`, role, and tool policy |
| **Role** | Determines the sub-agent's tool allowlist, system prompt patch, and permission policy |
| **Swarm** | A batch dispatch that spawns multiple sub-agents at once; supports `{{item}}` templates and a concurrency cap |
| **Context mode** | How much parent context the sub-agent can see: `inherit` / `summary` / `none` |

A sub-agent's lifecycle state is managed by `AgentLifecycleState`: `queued → running → completed / failed / cancelled / timed_out / interrupted`. By default the main agent waits in the foreground for the sub-agent to return; it can also be set to `background` so the main agent can continue.

## Delegate / DelegateSwarm

| Tool | Description |
| --- | --- |
| `Delegate` | Spawn a single sub-agent; supports `resume` to continue an existing agent_id |
| `DelegateSwarm` | Batch-spawn using `prompt_template` + `items`; supports `resume_agent_ids` to continue, and `max_concurrency` for rate limiting |

### Key Delegate Parameters

| Parameter | Default | Description |
| --- | --- | --- |
| `task` | required | Sub-agent task description |
| `role` | `coder` | Sub-agent role |
| `mode` | `foreground` | `foreground` to wait / `background` for concurrent background |
| `context` | `inherit` | Parent context passing method: `inherit` / `summary` / `none` |
| `resume` | — | An existing `agent_xxx` to continue; `role` must be omitted in this case |
| `title` | auto-derived | UI display name |

### Key DelegateSwarm Parameters

| Parameter | Description |
| --- | --- |
| `description` | Swarm title |
| `items` | Array of subtasks, each with a `title` and a `value` inserted into the template |
| `prompt_template` | Supports `{{item}}` and `{{description}}` |
| `resume_agent_ids` | `{ "agent_xxx": "continuation prompt" }` resume mapping |
| `max_concurrency` | Maximum concurrency (>0) |

## Role Types

The four built-in roles are defined by `AgentProfile::for_role`, each with its own tool allowlist and permission policy:

| Role | String | Tool set | Permission policy | When to use |
| --- | --- | --- | --- | --- |
| **Coder** | `coder` | Read/List/Grep/Find/Glob/Bash/Write/Edit/TodoList | Full access (shell + file write) | Implementation tasks; the default choice |
| **Explorer** | `explorer` | Read/List/Grep/Find/Glob/Bash (read-only) | Read-only shell, no writes | Read-only code exploration; can spawn multiple |
| **Planner** | `planner` | Read/List/Grep/Find/Glob | No shell, no writes | Implementation planning before writing code |
| **Reviewer** | `reviewer` | Read/List/Grep/Find/Glob/Bash (read-only) | Read-only shell, no writes | Read-only review after changes |

Bash for Explorer / Reviewer only permits read-only commands (`ls`, `rg`, `git status/diff/log/show`, etc.); file-writing tools are unavailable. Each role's "when to use" hint is embedded into the tool schema to guide the model in choosing correctly.

## Context Isolation

The `context` field determines how much parent context a sub-agent receives, and is the key to controlling token cost and isolation:

| Mode | Behavior |
| --- | --- |
| `inherit` (default) | Passes a curated selection of parent context |
| `summary` | Passes only a compact summary of the parent |
| `none` | Only the task text + role prompt; fully isolated |

A sub-agent has its own independent conversation history; only the result summary is returned to the main agent — the full history is not piped back. Resuming (`resume`) continues on top of the same sub-agent's history.

## Permission Inheritance

- A sub-agent's tool allowlist is determined by its role (see table above); `ToolPolicy` acts as a defensive backstop;
- File-writing and shell-mutating operations are only available to the Coder role; Bash for Explorer / Reviewer is enforced read-only at the prompt layer;
- The main agent's approval rules are not automatically passed through; sub-agent tool calls are evaluated against their own role policy;
- Sub-agents are forbidden from performing git mutations (commit/push/reset, etc.) by default, unless the parent agent explicitly requests it.

## AGENTS.md

`AGENTS.md` in the project root (case-insensitive; `CLAUDE.md` is accepted as a compatibility fallback) is a project-level context file that Neo auto-reads and injects into the main agent's context inside trusted directories. Key points:

- Priority: `AGENTS.md` takes precedence over `CLAUDE.md`;
- Scan scope: the current working directory and its trusted ancestor directories; only the first match is used;
- Recommended content: stable information such as project conventions, standards, and build commands — not details that change frequently;
- Complementary to skills: `AGENTS.md` is "the project's global statement to all agents"; skills are "reusable task flows".

Sub-agents do not read `AGENTS.md` directly by default, but the parent context (`inherit` / `summary`) carries it along as project background.

## Next Steps

- [Skills](skills.md) — Use skills to codify sub-agent workflows
- [MCP Servers](mcp.md) — Sub-agents can also call MCP tools
- [Permission Modes](../configuration/permissions.md) — Tool approval granularity
