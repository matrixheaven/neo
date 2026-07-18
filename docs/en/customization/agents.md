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
| **Coder** | `coder` | Read/List/Grep/Find/Glob/Bash/Write/Edit/TodoList/Sleep | Full access (shell + file write) | Implementation tasks; the default choice |
| **Explorer** | `explorer` | Read/List/Grep/Find/Glob/Bash (read-only)/Sleep | Read-only shell, no writes | Read-only code exploration; can spawn multiple |
| **Planner** | `planner` | Read/List/Grep/Find/Glob/Sleep | No shell, no writes | Implementation planning before writing code |
| **Reviewer** | `reviewer` | Read/List/Grep/Find/Glob/Bash (read-only)/Sleep | Read-only shell, no writes | Read-only review after changes |

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

`AGENTS.md` is Neo's only project instruction filename. Matching is case-insensitive for cross-platform behavior, but multiple case-folded variants in one directory are a blocking ambiguity (`Blocked: ambiguous AGENTS.md`). There is no `CLAUDE.md` fallback anywhere. Project instructions are session-scoped state: Neo delivers them as durable instruction epochs recorded in the session's JSONL event stream, and they never mutate the system prompt or earlier request bytes.

Recommended content: stable information such as project conventions, standards, and build commands — not details that change frequently. `AGENTS.md` is complementary to skills: it is "the project's global statement to all agents", while skills are "reusable task flows".

### Baseline: Global, Trusted Ancestors, Workspace Root

At new-session initialization, before the first user message, Neo resolves one baseline instruction epoch from:

1. `$NEO_HOME/AGENTS.md` (user-global, always trusted); and
2. the trusted `AGENTS.md` files on the filesystem ancestor chain of the primary workspace, ordered outermost-first and ending at the workspace root.

Project instructions load only when the project is trusted (`~/.neo/trust.json`). A session resumed from before this feature establishes a fresh baseline from current disk state on its next live turn; Neo does not reconstruct legacy behavior.

### Nested Scopes Discovered from Tool Paths

Neo discovers nested `AGENTS.md` files from typed tool arguments before tools run — never by parsing shell command strings:

| Tool class | Scope probe |
| --- | --- |
| `Read`, `Write`, `Edit` | Parent directory of the target file |
| `List`, `Grep`, `Find`, `Glob` | Explicit root or path directory |
| `Bash`, `Terminal` | Explicit `cwd`, otherwise the primary workspace |
| Other tools | No instruction scope probe |

A shell command that intends to work inside a nested subtree must set the tool's `cwd` field; Neo never infers paths from the command string. For each target directory inside the primary workspace, Neo scans only the directory chain from the workspace root to the target — not siblings or descendants:

```text
workspace/
|-- AGENTS.md
|-- crates/
|   |-- AGENTS.md
|   `-- neo-tui/
|       |-- AGENTS.md
|       `-- src/lib.rs
`-- docs/AGENTS.md   # not loaded for crates/neo-tui/src/lib.rs
```

When a batch touches new or changed scopes, preflight defers the entire tool batch — it never partially executes — appends one instruction epoch, and the model replans within the same turn. Rules render general-to-specific: global first, then trusted ancestors outermost-to-nearest, then the workspace root, then nested scopes shallowest-to-deepest, so deeper files override broader guidance. A missing `AGENTS.md` in a directory is not an error, and absence is not cached across turns, so a newly created file is discovered before the next tool runs in its scope. A tool that modifies an active source is still governed by the old revision; after that tool completes, Neo appends an update or removal epoch. Rewriting identical content creates no epoch.

### Instruction Imports

Only a standalone line with one leading `@` outside fenced code is an import:

```md
@./docs/project-rules.md
@~/.neo/shared-rules.md
```

Local Markdown links to `.md` files are imports too:

```md
Read the [project rules](./docs/project-rules.md) before acting.
```

`[project rules](./docs/project-rules.md)` and a standalone `@./docs/project-rules.md` load the same file and use the same recursion, trust, deduplication, and size limits. The only presentation difference is that Neo preserves the Markdown link, while the `@` directive is replaced by the imported body.

Neo preserves the original link and inserts the imported body immediately after it. Images, links inside inline or fenced code, URLs, and fragment-only links are not imported. For `@`, these also remain ordinary text: `@@./rules.md`, inline mentions such as `See @docs/rules.md`, URLs, and environment-variable expressions.

Resolution rules:

- Relative paths resolve from the directory containing the importing file; `~` uses the platform home directory.
- Project and ancestor bundles may import only from the primary workspace or `$NEO_HOME`.
- The user-global `$NEO_HOME/AGENTS.md` bundle may additionally import Markdown under the platform home. If the project is untrusted, its workspace subtree remains excluded.
- Imported sources must be regular UTF-8 `.md` files; directories, devices, sockets, URLs, and other special files are rejected.
- Canonical paths drive cycle detection and deduplication; a source imported more than once expands only at its first occurrence.
- Imported content is wrapped in an `<included_instructions path="...">` provenance element; it replaces an `@` directive or follows a preserved Markdown link. Source bodies remain exact UTF-8 text.

Structural safety limits (maximum recursive import depth 5, at most 32 sources per import graph, 1 MiB per source, 8 MiB per complete graph) are host safety limits, not the model-context budget:

| Limit | Value |
| --- | ---: |
| Maximum recursive import depth | 5 |
| Maximum sources in one import graph | 32 |
| Maximum bytes in one source | 1 MiB |
| Maximum bytes in one complete graph | 8 MiB |

One `AGENTS.md` plus its complete recursive import graph forms one atomic bundle: it activates whole or not at all, and Neo never presents a partially parsed import graph as complete.

### Trust and Filesystem Boundary

- `$NEO_HOME/AGENTS.md` is user-global and always trusted; its imports may read Markdown under `$NEO_HOME` or the platform home, except an untrusted workspace subtree.
- Project `AGENTS.md` files and their workspace-local imports load only when the primary project is trusted.
- Downward discovery never crosses the primary workspace boundary; workspace-external absolute `Read` paths and additional workspace roots do not trigger scoped discovery.
- Canonical containment uses `Path`/`PathBuf` semantics, not string prefix tests.

### Failure Semantics and Blocked Scopes

| Condition | Outcome |
| --- | --- |
| No `AGENTS.md` in a directory | No scope in that directory; not an error |
| Missing import | `Blocked: missing import` |
| Permission or I/O failure | `Blocked: unreadable source` |
| Invalid UTF-8 | `Blocked: invalid encoding` |
| Import cycle | `Blocked: include cycle` |
| Structural limit exceeded | `Blocked: instruction limit exceeded` |
| Canonical path leaves allowed roots | `Blocked: untrusted import` |
| Multiple case-folded `AGENTS.md` variants | `Blocked: ambiguous AGENTS.md` |
| Source changes repeatedly while read | `Blocked: unstable source` |

A failed bundle never injects its successfully read subset; the model receives one compact failure notice containing paths and reasons. While a scope is blocked, read-only `Read`, `List`, `Grep`, `Find`, and `Glob` operations may proceed for diagnosis, but `Write`, `Edit`, `Bash`, and `Terminal` remain blocked, and a mixed batch containing any of them is blocked as a whole. When the source fingerprint changes, Neo retries resolution automatically; a complete successful bundle replaces the failure state through the ordinary activation path.

### Dynamic Instruction Budget

Instruction content is pinned request context, counted by the existing context estimator:

```text
nominal_instruction_budget = max(65_536, effective_max_tokens / 8)
actual_instruction_budget  = min(nominal_instruction_budget, tokens safely available in the request)
```

`effective_max_tokens` is Neo's effective model limit, including observed provider-overflow correction. Admission priority reserves budget for the global bundle first, then the workspace root, then nested bundles deepest-to-shallowest, then trusted ancestors nearest-first; rendering still places deeper scopes last so they win project-scope conflicts.

If the complete selection does not fit safely, Neo compacts ordinary history first and then applies deterministic whole-bundle omission: the bundles that fit activate, the rest are ignored as whole units, and the workflow continues after one instruction-aware model replan. The transcript shows a `⚠ Instructions partially loaded` warning naming the loaded and ignored bundles with token estimates, and the model must not claim compliance with omitted rules. The same selection and source hashes do not warn twice; a later model-window, source, or scope change can make an ignored bundle admissible, in which case Neo emits a new activation epoch.

### Prefix-Cache Stability and Compaction

Instruction changes are append-only epochs; they never rewrite earlier request bytes, so the previous provider request remains an exact prefix of the next one until full compaction. Compaction summaries exclude instruction bodies, and afterwards Neo rehydrates the current rules byte-for-byte from the registry: the global instructions, the workspace baseline, and the current nested scope chain. Re-entering a previously dropped sibling scope emits a `Reactivated` epoch. Transcript cards (`◆ ready/loaded`, `↻ updated`, `− removed`, `⚠ partially loaded`, `✕ blocked`) show metadata only — never instruction bodies and never absolute home paths.

### Resume and Multi-Agent Visibility

Instruction epochs are durable JSONL events. On resume, Neo replays historical events and rebuilds the registry and per-agent visibility state before reconciling current disk state at the first live boundary; an unchanged session produces no duplicate epoch or card, while a changed source appends a replacement or removal epoch.

Source bytes and revision graphs are shared per session, but visibility is agent-local: the main agent and each Delegate sub-agent independently record which revisions their model has seen. Spawning a child materializes a child-owned baseline epoch (global, workspace, and applicable parent scopes) before the child's first model request; one agent's activation never implies another agent's visibility, and instruction cards stay in their own agent's transcript.

### `/init` and the Removed `CLAUDE.md` Fallback

`/init` creates or refreshes only the workspace-root `AGENTS.md`; nested `AGENTS.md` files are user-authored, and `/init` never generates or modifies them. The old startup-only loader and the `CLAUDE.md` compatibility candidate have been removed — `AGENTS.md` is the single canonical instruction filename.

## Next Steps

- [Skills](skills.md) — Use skills to codify sub-agent workflows
- [MCP Servers](mcp.md) — Sub-agents can also call MCP tools
- [Permission Modes](../configuration/permissions.md) — Tool approval granularity
