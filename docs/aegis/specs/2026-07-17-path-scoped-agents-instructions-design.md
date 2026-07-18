# Path-Scoped AGENTS.md Instructions Design

Status: Approved

Date: 2026-07-17

## Summary

Neo will replace its startup-only project context loader with one canonical,
session-scoped instruction runtime. The runtime will:

- load the initial trusted `AGENTS.md` chain once per session;
- discover nested `AGENTS.md` files from structured tool paths before tools run;
- expand strict standalone `@path` imports;
- defer the complete tool batch when the model has not seen an applicable
  instruction revision;
- append model-visible instruction epochs without rewriting the existing request
  prefix;
- preserve current instructions verbatim across compaction and replay;
- expose activation, update, omission, and failure through compact transcript
  cards; and
- remove the `CLAUDE.md` fallback and the old parallel loading path.

The implementation is local-only, cross-platform, trust-gated, and limited to
the primary workspace. It does not recursively preload the workspace tree.

## Problem

Neo currently reads `$NEO_HOME/AGENTS.md` and one context file from each trusted
ancestor of the startup project directory. The content is appended to the system
prompt as raw text. This has four limitations:

1. `AGENTS.md` files below the startup directory are not discovered when tools
   enter their directory trees.
2. A line such as `@~/.neo/CX.md` remains literal text. Neo does not read or
   inline the referenced file.
3. The project context is rebuilt with the runtime system prompt instead of
   being a durable, versioned session state. Dynamic updates would therefore
   risk changing early request bytes and reducing provider prefix-cache reuse.
4. There is no transcript evidence showing which instructions were loaded,
   ignored, replaced, or rejected.

Relying on the model to notice a nested file or interpret an import line is not a
correctness boundary. A direct `Write`, `Edit`, or `Bash` call can mutate a
subtree before the model has seen its local rules.

## Reference Findings

The design takes specific strengths from mature implementations without copying
their gaps.

### Codex

Codex reads a project-root-to-cwd chain at session initialization. Nested files
below cwd are not loaded automatically; the base prompt tells the model to look
for them. Instruction updates are represented as append-only replacement or
removal messages, and world state is persisted for replay. The session cache key
stays stable.

Useful ideas:

- append replacement state rather than mutating old request bytes;
- persist a full state baseline and later changes;
- keep the session cache key stable; and
- restore state explicitly after compaction.

Rejected gaps:

- nested discovery is a soft model instruction, not a host guarantee;
- ordinary cwd changes do not reliably refresh instruction content; and
- `@path` is not expanded.

Relevant reference code:

- `.references/codex/codex-rs/core/src/agents_md.rs`
- `.references/codex/codex-rs/core/src/context/world_state/agents_md.rs`
- `.references/codex/codex-rs/core/src/agents_md_manager.rs`
- `.references/codex/codex-rs/core/tests/suite/prompt_caching.rs`

### OpenCode

OpenCode resolves nested instructions after a successful `Read` and appends them
to that tool result. Loaded paths are recorded in tool metadata, so replay and
compaction can determine whether a source should be injected again.

Useful ideas:

- resolve instructions from a target path;
- persist loaded provenance;
- claim sources to deduplicate concurrent reads; and
- permit reinjection after compacted tool results disappear.

Rejected gaps:

- only `Read` participates; `Write`, `Edit`, shell, list, and search paths can
  bypass local instructions;
- instruction errors are commonly swallowed;
- instruction content has no aggregate budget;
- path containment is lexical rather than canonical; and
- content changes are not versioned while an old read remains in history.

Relevant reference code:

- `.references/opencode/packages/opencode/src/session/instruction.ts`
- `.references/opencode/packages/opencode/src/tool/read.ts`
- `.references/opencode/packages/opencode/test/session/instruction.test.ts`

### Claude Code

Claude Code has the strongest applicable behavior: it loads nested memory when a
path is first touched, supports recursive `@path` imports, deduplicates within a
session, limits recursion depth, and distinguishes external imports.

Useful ideas:

- path-triggered nested discovery;
- recursive import expansion with cycle protection;
- session-level source deduplication; and
- explicit handling for imports outside the project boundary.

Neo will adapt these ideas to its own trust, tool-batch, context-budget,
compaction, replay, and transcript contracts rather than copy Claude-specific
internals.

Relevant reference code:

- `.references/claude-code/src/utils/claudemd.ts`
- `.references/claude-code/src/utils/attachments.ts`

## Goals

- Make every applicable nested instruction revision visible to the model before
  any corresponding tool side effect.
- Keep normal single-session provider requests append-only for prefix-cache
  stability.
- Treat instruction content as exact rules, not summarizable conversation text.
- Preserve a continuous autonomous workflow: activation causes an internal
  model replan, not a new user turn.
- Expand deterministic local Markdown imports without opening a general file or
  network include mechanism.
- Make every activation, replacement, omission, and failure inspectable in the
  transcript without showing instruction bodies.
- Keep discovery cross-platform and bounded by canonical filesystem roots.
- Use one runtime state for main and child agents while keeping model-visible
  activation state agent-local.
- Delete obsolete compatibility paths instead of maintaining two semantics.
- Update the matching English and Chinese documentation in the same change.

## Non-Goals

- Instruction trust for additional workspace roots.
- Scoped `AGENTS.md` discovery for paths outside the primary workspace.
- URL, network, environment-variable, or arbitrary-file imports.
- Parsing shell command text to infer every filesystem path it may touch.
- Guessing path semantics for MCP or third-party tools.
- User configuration for import depth, source count, hard file limits, or the
  dynamic instruction budget in the first version.
- Displaying instruction bodies in the transcript.
- Automatically generating or modifying nested `AGENTS.md` files.
- Keeping `CLAUDE.md`, old event shapes, or the old loader as compatibility
  fallbacks.

## Terminology

- **Primary workspace**: `AppConfig.project_dir`, which defaults to the directory
  from which Neo starts. It is the downward discovery boundary.
- **Initial ancestor chain**: the existing trusted filesystem ancestor chain,
  ordered from the outermost ancestor to the primary workspace.
- **Scope**: the directory tree rooted at the directory containing an
  `AGENTS.md` file.
- **Source**: one Markdown file read by the instruction runtime, including an
  imported file.
- **Bundle**: one `AGENTS.md` plus its complete recursive import graph. A bundle
  is the unit of budget admission.
- **Revision**: the content-addressed snapshot of a bundle.
- **Instruction epoch**: an append-only semantic event that changes the rules
  visible to one agent context.
- **Baseline**: the global and initial project instruction state admitted before
  the first user model request.

## Architecture

```text
                       Session-scoped
+---------------------------------------------------------+
| InstructionRegistry                                     |
|                                                         |
| source path . scope . content hash . revision           |
| expanded content . imports . activation state           |
+---------------+-----------------------+-----------------+
                |                       |
                v                       v
+--------------------------+  +---------------------------+
| InstructionResolver      |  | InstructionContextBridge  |
|                          |  |                           |
| path -> applicable chain |  | registry -> model epoch   |
| parse @path              |  | append-only injection     |
| trust/budget validation  |  | compaction rehydration    |
+---------------+----------+  +--------------+------------+
                |                            |
                v                            v
+--------------------------+  +---------------------------+
| ToolBatchPreflight       |  | AgentEvent / Transcript   |
|                          |  |                           |
| inspect typed paths      |  | loaded / updated / failed |
| defer entire batch       |  | compact semantic card     |
| never partially execute  |  | durable replay            |
+--------------------------+  +---------------------------+
```

### InstructionRegistry

`InstructionRegistry` is the only session-level source of truth. It owns:

- canonical source paths and display paths;
- source metadata and content hashes;
- parsed import graphs;
- expanded bundle revisions;
- token estimates;
- admitted and ignored bundle selections;
- failure fingerprints; and
- per-agent model-visible generations.

Resolver, runtime, compaction, replay, and TUI must not maintain independent
`loaded` sets.

### InstructionResolver

The resolver is deterministic and side-effect-free after filesystem reads. It
maps a set of canonical tool target directories to:

- the applicable initial and nested scope chain;
- complete atomic bundles;
- a stable bundle order;
- a revision delta against the registry; and
- structured failures or omissions.

### ToolBatchPreflight

Preflight runs after tool arguments are parsed but before permission prompts,
scheduling, or execution. It handles the complete assistant tool-call batch as
one unit.

### InstructionContextBridge

The context bridge converts registry changes into model-visible instruction
epochs, accounts for their token cost, keeps the session prefix stable, and
rehydrates current rules after compaction.

### Transcript Projection

The TUI consumes the same instruction epoch used by replay and model context.
It displays metadata only and absorbs internal tool exchanges whose only outcome
was instruction deferral.

## Core Invariants

1. `AGENTS.md` is the only project instruction filename. Matching remains
   case-insensitive for cross-platform behavior, but multiple case-folded
   variants in one directory are an error.
2. `$NEO_HOME/AGENTS.md` is user-global and always trusted.
3. Project instructions remain gated by the existing project trust decision.
4. Downward discovery never crosses the primary workspace boundary.
5. New rules enter model context before corresponding tool execution.
6. A deferred or blocked parallel batch never partially executes.
7. Instruction updates append an epoch; they never modify an old system prompt
   or message in place.
8. Instruction content is never summarized, truncated, or micro-projected.
9. Full compaction is the only normal operation allowed to establish a new
   provider-visible prefix baseline.
10. Replay restores historical state from events before reconciling current
    disk state.
11. No instruction failure is silent.
12. Transcript cards never expose instruction bodies.

## Initial Baseline

At new-session initialization, Neo resolves:

1. `$NEO_HOME/AGENTS.md`, if present; and
2. trusted `AGENTS.md` files on the existing filesystem ancestor chain ending at
   the primary workspace.

The old `resources::load_context_files` path is removed. Project instructions
are no longer rebuilt into `AgentConfig.system_prompt` on each runtime creation.
Instead, Neo emits one baseline instruction epoch before appending the first
user message. This makes the baseline durable, gives the model an unambiguous
instruction-before-request ordering, and keeps later system-prompt bytes
independent from project instruction changes.

The baseline transcript projection is one aggregated `Instructions ready` card.

When a pre-feature session is resumed and contains no instruction epoch, Neo
establishes a fresh baseline from current disk state. It does not infer or
reconstruct legacy `CLAUDE.md` behavior.

## Scope Discovery

Neo derives target directories only from typed tool arguments:

| Tool class | Scope probe |
|---|---|
| `Read`, `Write`, `Edit` | Parent of the target file |
| `List`, `Grep`, `Find`, `Glob` | Explicit root or path directory |
| `Bash`, `Terminal` | Explicit `cwd`, otherwise primary workspace |
| Other tools | No instruction scope probe |

Neo does not parse shell command strings. A model that intends to run a command
under a nested subtree must use the tool's `cwd` field. The base runtime guidance
must state this requirement.

For every target directory inside the primary workspace, the resolver scans only
the directory chain from the primary workspace to the target. It does not walk
siblings or descendants.

```text
workspace/
|-- AGENTS.md                 initial workspace bundle
|-- crates/
|   |-- AGENTS.md             discovered for crates/**
|   `-- neo-tui/
|       |-- AGENTS.md         discovered for crates/neo-tui/**
|       `-- src/lib.rs        tool target
`-- docs/AGENTS.md            not read for the src/lib.rs target
```

The resolver merges all target chains in a batch, deduplicates directories and
sources, and sorts deterministically by depth and canonical path. Rendering the
selected rules remains general-to-specific so deeper instructions appear later
and can override broader project guidance.

### Directory and Source Caching

- Directory results may be memoized within one batch.
- Positive source metadata may be cached across turns.
- Missing `AGENTS.md` results are not cached across turns, so a newly created
  file is discovered promptly.
- Metadata change triggers a reread; content hash, not mtime, decides whether a
  new revision exists.
- Rewriting identical content does not create an epoch or card.

## Tool Preflight State Machine

```text
assistant emits tool batch
           |
           v
parse and normalize all tool arguments
           |
           v
collect canonical target directories
           |
           v
retain targets inside primary workspace
           |
           v
resolve union of applicable scope chains
           |
           v
InstructionRegistry::reconcile()
     |
     |-- NoChange ----------------------> execute complete batch
     |
     |-- Activated / Updated / Removed
     |          |
     |          v
     |    execute none of the batch
     |    complete every call with a deferred result
     |    append InstructionEpoch
     |    continue to the next model step
     |
     `-- Blocked
                |
                v
          execute none of the batch
          append structured failure results and epoch
```

Preflight precedes permission dialogs so the model can revise an operation before
Neo asks the user to approve it. If a permission dialog waits for input, Neo
performs a lightweight fingerprint recheck after approval and before execution.
A changed source returns to the defer path instead of executing an operation
approved against stale instructions.

### Deferred Tool Results

The assistant tool-call message is already part of provider history, so every
deferred call must receive a matching result. Deferred results are non-error,
non-terminating results with a machine-readable reason indicating that no side
effect occurred because project instructions changed.

The provider-visible sequence remains valid:

```text
assistant(tool calls)
tool(deferred)
tool(deferred)
...
contextual instruction message(InstructionEpoch)
assistant(replanned tool calls)
tool(actual results)
```

The runtime never silently replays the original calls. The model must issue new
calls after reading the rules. The registry marks the epoch visible before the
next model request, so the retried batch does not loop through the same defer
state.

This continuation stays inside the current `run_agent_turn`; it does not require
a new prompt or end the user's workflow.

## Canonical `@path` Imports

Only a standalone line with one leading `@` outside fenced code is an import:

```md
@./docs/rust-rules.md
@~/.neo/CX.md
```

The following remain ordinary text:

- `@@./rules.md`;
- inline mentions such as `See @docs/rules.md`;
- import-looking text inside fenced code;
- URLs; and
- environment-variable expressions.

### Resolution Rules

- Relative paths resolve from the directory containing the importing file.
- `~` uses the platform home directory.
- The final canonical path must remain inside the primary workspace or
  `$NEO_HOME`.
- Imported sources must be regular UTF-8 `.md` files.
- Directories, devices, sockets, URLs, and other special files are rejected.
- `..` and symlinks are permitted only when canonicalization remains inside an
  allowed root.
- Canonical paths drive cycle detection and deduplication.
- A source imported more than once expands only at its first occurrence.
- Imported content replaces the directive at its original position and is
  wrapped with source provenance:

```text
<included_instructions path="~/.neo/CX.md">
...exact source content...
</included_instructions>
```

Path attributes must be escaped. Source bodies remain exact UTF-8 text.

### Structural Safety Limits

These are host safety limits, not the model-context budget:

| Limit | Value |
|---|---:|
| Maximum recursive import depth | 5 |
| Maximum sources in one import graph | 32 |
| Maximum bytes in one source | 1 MiB |
| Maximum bytes in one complete graph | 8 MiB |

The parser does not truncate structural overages. It reports a blocked bundle.

### Atomic Bundle Semantics

One `AGENTS.md` and every required recursive import form one atomic bundle. For
normal parse, trust, and I/O validation, the complete bundle either activates or
does not activate. Neo never presents a partially parsed import graph as if it
were complete.

## Trust and Filesystem Boundary

- `$NEO_HOME/AGENTS.md` and imports under `$NEO_HOME` are user-global.
- Project `AGENTS.md` content and its workspace-local imports are loaded only
  when the primary project is trusted.
- An import outside both the primary workspace and `$NEO_HOME` is rejected.
- There is no temporary external-import approval branch in the first version.
- Workspace-external absolute `Read` paths do not trigger scoped instruction
  discovery.
- Additional workspace roots do not participate in instruction discovery.
- Canonical containment uses `Path`/`PathBuf` semantics, not string prefix tests.

This deliberately keeps one trust model. A user who needs a shared instruction
file can place it under the workspace or `$NEO_HOME`.

## Failure Semantics

| Condition | Outcome |
|---|---|
| No `AGENTS.md` in a directory | No scope in that directory; not an error |
| Missing import | `Blocked: missing import` |
| Permission or I/O failure | `Blocked: unreadable source` |
| Invalid UTF-8 | `Blocked: invalid encoding` |
| Import cycle | `Blocked: include cycle` |
| Structural limit exceeded | `Blocked: instruction limit exceeded` |
| Canonical path leaves allowed roots | `Blocked: untrusted import` |
| Multiple case-folded `AGENTS.md` variants | `Blocked: ambiguous AGENTS.md` |
| Source changes repeatedly while read | `Blocked: unstable source` |

A failed bundle does not inject the successfully read subset. The model receives
one compact failure notice containing paths and reasons, not partial instruction
bodies.

The same `source + failure kind + fingerprint` is injected and displayed once.
While a scope is blocked:

- read-only `Read`, `List`, `Grep`, `Find`, and `Glob` operations may proceed for
  diagnosis after the failure epoch is visible;
- `Write`, `Edit`, `Bash`, and `Terminal` remain blocked; and
- a mixed batch containing any mutation or execution tool is blocked as a whole.

When the source fingerprint changes, Neo retries resolution automatically. A
successful complete bundle replaces the failure state and follows the ordinary
activation and replan path.

## Dynamic Instruction Budget

Instruction content is pinned request context. It is counted by the existing
context estimator and cannot be removed by request-time projection.

```text
nominal_instruction_budget =
    max(65_536, effective_max_tokens / 8)

actual_instruction_budget =
    min(nominal_instruction_budget, tokens safely available in the request)
```

`effective_max_tokens` is Neo's effective model limit, including observed
provider-overflow correction, not only the catalog's advertised window.

Examples:

| Effective model window | Nominal instruction budget |
|---:|---:|
| 32K | 64K, then clamped to safe request capacity |
| 128K | 64K |
| 512K | 64K |
| 1M | 128K |
| 2M | 256K |

The existing reserved output headroom, fixed request overhead, and ordinary
context still participate in `ContextBudgetSnapshot`; a nominal budget never
authorizes an overflowing request.

### Admission Algorithm

```text
discover complete instruction graph
          |
          v
host reads, validates, and estimates every bundle
          |
          v
does complete selection fit current request safely?
     |
     |-- yes -> inject every selected bundle
     |
     `-- no  -> full compact ordinary history first
                    |
                    v
              recompute budget
                    |
                    |-- fits -> inject all
                    |
                    `-- exceeds nominal cap
                           -> deterministic whole-bundle selection
```

Neo reads candidate files on the host so it can validate the graph, calculate
cost, and report exact omissions. Content excluded by model-context admission is
discarded after measurement and is not written into an instruction epoch.
Registry metadata retains only path, hash, token estimate, and omission reason.

### Budget Selection Priority

Admission and model rendering use different orders.

Admission priority:

1. `$NEO_HOME/AGENTS.md` bundle;
2. primary workspace root `AGENTS.md` bundle;
3. nested target bundles from deepest to shallowest; and
4. trusted ancestors above the primary workspace, nearest first.

This retains user-global and workspace-root policy while preventing a broad
ancestor bundle from starving the most specific target rules.

After selection, model-visible content is rendered in this exact semantic order:

1. `$NEO_HOME/AGENTS.md`;
2. selected trusted ancestors, outermost to nearest;
3. primary workspace root; and
4. selected nested scopes, shallowest to deepest.

Deeper instructions therefore remain later in the message and win project-scope
conflicts even though admission reserves their budget earlier.

### Over-Budget Behavior

Token-budget overflow differs from structural or integrity failure:

- the selected complete bundles activate;
- bundles that do not fit are ignored as whole units;
- the workflow continues after one instruction-aware model replan;
- the model receives a small omission notice and must not claim compliance with
  omitted rules; and
- the transcript displays a warning listing loaded and ignored bundles with
  token estimates.

The same selection and source hashes do not produce repeated warnings. Model
window changes, source changes, scope changes, or compaction may make an ignored
bundle admissible later, in which case Neo emits a new activation epoch.

## Prefix-Cache Stability

Initial instructions are an immutable contextual baseline before the first user
request. Later changes are appended as epochs.

```text
Request N     = P + H
Request N + 1 = P + H + deferred results + instruction epoch
Request N + 2 = P + H + deferred results + instruction epoch
                  + replanned tool exchange
```

`P` includes stable base instructions, tool definitions, and other request
properties. Scope activation does not change earlier system-prompt bytes, tool
schema order, reasoning settings, or the session cache key.

An update appends a replacement epoch:

```text
InstructionEpoch revision=1
...
InstructionEpoch revision=2 replaces=revision=1
```

The old revision remains historical bytes. The replacement marker changes its
semantic authority without rewriting the prior provider prefix.

Tests must compare adjacent `ChatRequest.messages` and prove that the complete
earlier request message sequence is the exact prefix of the next request before
full compaction.

## Compaction Integration

Instruction admission happens before a model request. If the pending bundle
would cross a compaction threshold, Neo compacts ordinary history before adding
the new epoch.

```text
deferred tool exchange complete
          |
          v
pending bundle held outside AgentContext
          |
          v
full compact ordinary history if required
          |
          v
rehydrate exact current instruction baseline
          |
          v
admit pending bundle and resnapshot
          |
          |-- fits -> model replans
          `-- cannot fit -> budget omission or blocked context error
```

Neo must not inject a large epoch and immediately summarize it. The compaction
summary request excludes instruction bodies. Rules are restored from the
registry verbatim after the summary is applied.

After compaction, Neo rehydrates:

- `$NEO_HOME` instructions;
- the initial workspace baseline; and
- the current or most recently used nested scope chain.

Previously visited sibling scopes remain in registry metadata but not pinned
model context. Re-entering one emits a `Reactivated` epoch. Manual compaction
without a pending target preserves the most recently successful tool scope.

Full compaction already establishes a new provider prefix, so exact rehydration
there does not create an additional cache regression. Micro projection must
never alter instruction epochs.

## Durable Event and Replay Model

One canonical semantic event owns both model-visible content and transcript
metadata:

```text
AgentEvent::InstructionEpoch {
  agent_id,
  generation,
  outcome,
  scopes,
  sources,
  revisions,
  expanded_content,
  ignored,
  replaces,
  failure,
  deferred_tool_ids,
}
```

The exact Rust representation may use typed nested structs, but it must preserve
these semantics.

- Live `AgentContext` converts the event into one contextual instruction
  message.
- Replay performs the same conversion and reconstructs the registry and
  per-agent visibility state.
- The TUI projects the event into a metadata-only transcript card.
- JSONL stores expanded content once; a duplicate `MessageAppended` event is not
  emitted for the same epoch.
- Resume restores historical events before checking current files.
- The first live provider boundary reconciles active sources and appends a
  replacement or removal epoch if disk state changed.
- Replaying an unchanged session does not create a new card or duplicate model
  message.

Outcomes include:

- `Ready`;
- `Activated`;
- `Updated`;
- `Removed`;
- `Reactivated`;
- `PartiallyLoaded`; and
- `Blocked`.

## Transcript Design

Neo will not load instructions silently. It displays one compact semantic card
per meaningful baseline, activation, revision, omission selection, or failure.
It does not display one card per imported file.

### Compact Forms

```text
◆ Instructions ready · workspace
  3 sources · 2 imports · 18.6K tokens

◆ Instructions loaded · crates/neo-tui/**
  AGENTS.md · 2 imports · 31.8K tokens

↻ Instructions updated · crates/neo-tui/**
  revision 7af13c2e

◆ Instructions reactivated · crates/neo-tui/**

− Instructions removed · crates/neo-tui/**

⚠ Instructions partially loaded · crates/neo-tui/**
  92K of 64K tokens · 2 bundles ignored

✕ Instructions blocked · crates/neo-tui/**
  Missing import: ~/.neo/CX.md
```

`Loaded`, `Ready`, and `Reactivated` use brand styling with muted metadata.
`Updated` and `PartiallyLoaded` use `status_warn`. `Blocked` uses
`status_error`.

### Expanded Form

`Ctrl+O` shows:

```text
Scope
  crates/neo-tui/**

Loaded
  ~/.neo/AGENTS.md                         8.2K
  ./AGENTS.md                             17.4K
  crates/neo-tui/AGENTS.md                31.8K

Ignored
  crates/AGENTS.md                        22.1K  budget exceeded
  crates/neo-tui/src/AGENTS.md            12.5K  bundle did not fit

Imports
  ~/.neo/CX.md
  crates/neo-tui/docs/testing.md

Revision
  7af13c2e
```

Paths are workspace-relative or `~/`-relative. Cards never expose absolute home
paths or instruction bodies.

### Deferred Tool Presentation

Instruction activation happens after the model has emitted tool-call
placeholders but before those tools execute. The TUI must avoid rendering an
internal failed-tool sequence followed by duplicate retried tools.

```text
model emits:  [Read pending] [Grep pending] [Bash pending]
                            |
                            v InstructionEpoch(deferred_tool_ids)
presentation: [Instructions loaded]
                            |
                            v model replans
              [Read actual] [Grep actual] [Bash actual]
```

`TranscriptStore` places the instruction card at the earliest deferred tool
entry's canonical position and absorbs the remaining unexecuted placeholders.
The card is finalized there and does not drift to the transcript bottom.
Provider history and JSONL still retain every valid tool call and deferred
result. Replay uses `deferred_tool_ids` to reconstruct the same visible order.

Instruction cards are finalized semantic entries, not long-running spinner
cards. Identical scope, revision, selection, and failure fingerprints do not
produce duplicate cards.

Compaction rehydration of the already-current scope does not create a card.
Leaving and later re-entering a dropped scope does create `Reactivated`.

## File Races and Consistent Reads

Sources are read with a bounded stability check:

```text
metadata A
    |
    v
read complete bytes
    |
    v
metadata B
    |
    |-- A == B -> accept and hash
    `-- A != B -> retry once
                   |-- stable  -> accept
                   `-- changed -> Blocked: unstable source
```

The final identity is a content hash. Metadata is only a fast path.

A batch uses one frozen registry generation. All scope resolution and final
fingerprint checks finish before any call begins execution. Neo does not attempt
to hold cross-platform filesystem locks across model or user interaction.

If a tool modifies `AGENTS.md` or an imported source, the old revision governs
that tool. After the tool result and before the next model request, reconciliation
appends an update or removal epoch. A newly created nested `AGENTS.md` is loaded
before any later tool in its scope.

## Multi-Agent Semantics

Source bytes and revision graphs are session-shared, but visibility is
agent-local:

```text
Session InstructionRegistry
|-- shared source cache
|-- main AgentInstructionState
|-- child AgentInstructionState
`-- reviewer AgentInstructionState
```

- Concurrent reads of one source use keyed single-flight.
- Each agent independently records which generations its model has seen.
- A main-agent activation does not imply child-agent visibility.
- Child spawn materializes a child-owned baseline epoch containing global,
  workspace, and currently applicable parent scope rules before the child's
  first model request. Full-context inheritance may seed already visible
  revisions, but summary inheritance does not imply visibility by itself.
- Child tool dispatch uses the same preflight state machine.
- Events carry `agent_id` and are written to the matching agent JSONL.
- Cards stay in the corresponding agent transcript. The first version does not
  duplicate child instruction cards into the main transcript.

## Canonical Migration

The implementation removes, rather than wraps:

- `resources::load_context_files` and its startup prompt concatenation;
- the `CLAUDE.md` context candidate;
- candidate-order behavior that treats multiple filenames as equivalent;
- tests and docs describing startup-only project-root behavior; and
- any temporary duplicate loaded-source state outside `InstructionRegistry`.

There is no feature flag and no dual runtime. Existing sessions without an
instruction epoch establish the new baseline on their next live turn.

## Testing Strategy

Tests must be narrow and prove behavioral boundaries rather than derived data
round trips.

### Resolver and Import Tests

- primary-workspace-to-target scope chain;
- multiple nested scopes and deterministic order;
- case-insensitive `AGENTS.md` matching and ambiguous collision;
- relative and `~` imports;
- import position and nested source provenance;
- code-fence, inline, `@@`, URL, and environment-variable non-imports;
- cycle, depth, count, file-size, graph-size, UTF-8, and special-file failures;
- canonical containment and symlink escape;
- metadata-only change versus content revision;
- stable-read retry and unstable-source failure; and
- no cross-turn negative cache.

### Runtime Tests

- first `Read`, `Edit`, `Write`, and nested-cwd shell call defers before any
  side effect;
- one new scope pauses an entire mixed parallel batch;
- every deferred assistant tool call receives a provider-valid result;
- the model's retried batch executes once without a defer loop;
- permission wait rechecks instruction fingerprints;
- read-only diagnosis proceeds after a failure epoch while mutation remains
  blocked;
- changing or deleting an active source appends replacement or removal;
- one agent's visibility does not suppress another agent's activation; and
- additional workspaces and external reads do not trigger discovery.

### Budget, Cache, and Compaction Tests

- nominal budget uses `max(65_536, effective_max_tokens / 8)` and safe request
  clamping;
- context pressure triggers compaction before pending epoch admission;
- over-budget selection is deterministic and bundle-atomic;
- global/root and deepest-scope priorities are honored;
- ignored metadata and omission notices contain no instruction bodies;
- adjacent pre-compaction requests preserve an exact message prefix;
- full compaction excludes instruction bodies from the summary input;
- current rules are restored byte-for-byte after compaction;
- old sibling scopes reactivate when re-entered; and
- micro projection never changes instruction events.

### Replay and TUI Tests

- JSONL replay reconstructs registry, per-agent visibility, model context, and
  cards from one event stream;
- unchanged resume emits no duplicate epoch or card;
- live source changes after resume append a replacement;
- compact and expanded card text, path redaction, colors, and warning icon;
- ignored bundle lists and token estimates;
- deferred placeholders are absorbed at the earliest canonical position; and
- later transcript updates do not move finalized cards.

Verification must use one package, one explicit target selector, and an exact or
narrow test-name filter. Broad workspace tests are not completion evidence.

## Documentation

Implementation must update these paired English and Chinese surfaces:

- `docs/{zh,en}/customization/agents.md`;
- `docs/{zh,en}/configuration/config-files.md`;
- `docs/{zh,en}/configuration/data-locations.md`;
- `docs/{zh,en}/reference/slash-commands.md`; and
- repository `AGENTS.md` where its runtime quick reference describes current
  behavior.

The documentation must explain:

- the initial global/ancestor chain and nested path scopes;
- standalone `@path` syntax, recursion, and allowed roots;
- preflight defer and model-replan behavior;
- trust and failure boundaries;
- dynamic budget, atomic omission, and transcript warnings;
- cache, compaction, resume, and multi-agent semantics;
- that `/init` creates or refreshes only the workspace-root `AGENTS.md`; and
- that `CLAUDE.md` is no longer a fallback.

Before completion, scan source, tests, README, repository guidance, and both
documentation locales for stale `CLAUDE.md` fallback and startup-only scope
descriptions.

## Acceptance Criteria

The feature is complete when all of the following are true:

1. A nested rule cannot be bypassed by the first typed filesystem mutation or a
   shell command using that nested cwd.
2. A new or changed scope pauses the full tool batch, appends one durable epoch,
   and continues the same user workflow through model replan.
3. Standalone recursive imports work inside the approved roots and every failure
   is visible.
4. Normal instruction changes preserve the previous request as an exact prefix.
5. Compaction preserves current instruction bytes without summarizing them.
6. Token overflow selects whole bundles deterministically, continues the
   workflow, and displays exactly which bundles were ignored.
7. Replay and multi-agent execution never confuse source availability with
   model-visible activation.
8. Transcript cards remain compact, durable, correctly positioned, and free of
   instruction bodies or absolute home paths.
9. The old loader and `CLAUDE.md` fallback have been deleted with no dual path.
10. English and Chinese documentation describe the same canonical behavior.
