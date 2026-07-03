# Neo Event-Sourced Living Transcript Design

## Summary

Neo already has the core shape of multi-agent living transcripts:

- main sessions live under `agents/main/wire.jsonl`
- subagents can own `agents/<agent_id>/wire.jsonl`
- `state.json` records agent ownership, roles, and swarm membership
- TUI Delegate and DelegateSwarm cards update in place by `agent_id` and
  `swarm_id`
- recovered running or queued delegates are marked lost instead of pretending
  the old in-process runtime still exists

This design upgrades that snapshot-based model into an event-sourced model.
Every persisted event gets a stable event envelope with identity, ordering, and
causality metadata. Child wires can then replay into the same transcript tree
the user saw while the child was running, and interrupted child agents can
resume under the same `agent_id` and child wire.

The recommended first version does not introduce a daemon, supervisor, hosted
service, or process-level live reattach. A Neo process cannot reattach to a
Tokio task that died with a previous process. Instead, Neo restores the
agent's identity, transcript, context, and UI position, marks unfinished child
agents as interrupted and resumable, then appends a new turn to the same child
wire when the user or main agent resumes them.

## Goals

- Persist every main-agent and child-agent event with a stable `event_id`.
- Preserve per-agent ordering with a monotonic sequence number.
- Attach enough metadata to each persisted event to replay transcript UI
  without guessing from file position.
- Replay child wires into rich child transcript entries, including text,
  thinking, tool runs, partial updates, tool results, token usage, and errors.
- Preserve existing Delegate and DelegateSwarm chat cards as the primary UI
  surface; do not add a separate multi-agent page.
- Replace restored running or queued child agents with an explicit
  interrupted/resumable state, not a generic failed/lost state.
- Resume interrupted child agents under the same `agent_id`, same child wire,
  and a new child turn.
- Keep the existing Neo preference of deleting obsolete paths instead of
  maintaining long-term compatibility branches.

## Non-Goals

- Do not implement a local daemon or supervisor in this phase.
- Do not claim that a previous process's live child task can be reattached.
- Do not keep raw `AgentEvent` JSONL as a long-term parallel persistence
  format after migration.
- Do not embed full child transcripts inside the main agent wire.
- Do not create a hosted collaboration, sync, marketplace, or remote execution
  surface.
- Do not redesign the DelegateSwarm TUI into a separate panel. It remains a
  chat transcript tool card.

## Current State

The current `AgentEvent` enum contains useful event-local ids such as message
ids, tool ids, shell ids, terminal ids, question ids, agent ids, and swarm ids.
However, the persisted JSONL record is the bare event payload. There is no
uniform persisted event identity, sequence number, parent event reference, or
agent ownership metadata around every event.

Examples:

- `MessageStarted { turn, id }` has a message id.
- `ToolExecutionStarted { turn, id, name, arguments }` has a tool execution id.
- `DelegateSwarmUpdated { turn, swarm }` contains a `swarm_id`.
- `MessageAppended { message }` does not carry a persistent message event id.
- `TextDelta` and `ThinkingDelta` do not identify which persisted event they
  are, beyond turn order.

The child runtime already appends child events to a child wire when a
`child_wire_path` is available. The TUI already upserts Delegate and
DelegateSwarm entries by `agent_id` and `swarm_id`. Runtime replay can restore
delegate and swarm snapshots from parent-level events, but restored
running/queued agents are marked lost because the old in-process runtime cannot
exist after process exit.

That means Neo currently has:

- agent-level stable identity
- swarm-level stable identity
- snapshot-based living transcript cards
- child wire persistence foundation

It does not yet have:

- event-level stable identity
- rich child transcript replay from child wires
- interrupted/resumable semantics for restored unfinished children
- resumed child turns that continue the same child wire sequence

## Target Model

Neo will persist a wire envelope around every `AgentEvent`:

```rust
pub struct WireEvent {
    pub schema_version: u32,
    pub event_id: EventId,
    pub session_id: String,
    pub agent_id: AgentId,
    pub turn_id: TurnId,
    pub seq: u64,
    pub timestamp_ms: u64,
    pub parent_event_id: Option<EventId>,
    pub causal_id: Option<String>,
    pub event: AgentEvent,
}
```

The `event` field remains the domain event. The envelope is the durable wire
contract. It answers questions that a replay engine, TUI, exporter, RPC server,
or future app-server cannot answer reliably from the bare event alone:

- which agent owns this event?
- what is the durable id of this event?
- what order should replay use if the file is copied or partially recovered?
- which previous event does this one refine or complete?
- which tool/message/swarm/delegate object is this event about?

## Identity Rules

Use stable, readable ids where possible:

| Object | Rule |
| --- | --- |
| Session | Existing `session_*` id |
| Main agent | Fixed `main` |
| Child agent | Existing `agent_*` id |
| Swarm | Existing `swarm_*` id |
| Turn | `turn_<agent_id>_<n>` or a typed equivalent |
| Event | `evt_<agent_id>_<seq>` |
| Message | Existing message id when present; generated by the writer when absent |
| Tool execution | Existing tool execution id |
| Child activity | Derived from `event_id` plus `causal_id`; no separate durable id needed |

`seq` is monotonic per agent wire. It starts at 1 for each agent and continues
when the agent is resumed. The writer determines the next sequence by reading
the last valid `WireEvent` in the agent wire before appending.

`event_id = evt_<agent_id>_<seq>` is deterministic and easy to inspect. It is
stable as long as Neo never rewrites existing wire records. Neo should keep
wires append-only.

## Causality Rules

The writer fills `causal_id` from the event payload:

| Event family | `causal_id` |
| --- | --- |
| `MessageStarted`, `MessageFinished` | message id |
| `TextDelta` | active message id when known |
| `ThinkingStarted`, `ThinkingFinished` | thinking block id |
| `ThinkingDelta` | active thinking id when known |
| `ToolCall*` | tool call id |
| `ToolExecution*` | tool execution id |
| `ApprovalRequested` | approval id |
| `QuestionRequested` | question id |
| `ShellCommand*` | shell id |
| `TerminalSession*` | terminal id |
| `Delegate*` | agent id |
| `DelegateSwarm*` | swarm id |
| `Workflow*` | workflow id |

`parent_event_id` links refinement events to their start event when the writer
can know it cheaply:

- text deltas can point at `MessageStarted`
- thinking deltas can point at `ThinkingStarted`
- tool execution updates and finishes can point at `ToolExecutionStarted`
- tool call argument deltas and finishes can point at `ToolCallStarted`

If the parent is unknown, `parent_event_id` is `None`. Replay must still work
from `(agent_id, turn_id, seq, causal_id)`.

## Wire Format

The wire metadata line remains first. It must advance the schema version and
declare that following records are wire envelopes:

```json
{
  "kind": "neo.session.metadata",
  "format": "neo.session.wire",
  "schema_version": 2,
  "created_at": "..."
}
```

Every non-metadata line is a `WireEvent`.

Raw `AgentEvent` lines are supported only by a one-shot migration command or
migration routine. Runtime readers should fail closed on raw records after the
migration step has run. Neo should not keep dual runtime paths for raw
`AgentEvent` and `WireEvent`.

## Writer API

Replace bare event appends with an agent-scoped writer:

```rust
pub struct AgentWireWriter {
    session_id: String,
    agent_id: AgentId,
    next_seq: u64,
    active_message_event: Option<EventId>,
    active_message_id: Option<String>,
    active_thinking_event: Option<EventId>,
    active_thinking_id: Option<String>,
    active_tool_events: BTreeMap<String, EventId>,
}
```

The public append method accepts an `AgentEvent` and writes a `WireEvent`:

```rust
impl AgentWireWriter {
    pub async fn append_event(&mut self, event: &AgentEvent) -> Result<WireEvent, SessionError>;
}
```

Callers that currently use `JsonlSessionWriter::append_event(&AgentEvent)` do
not generate ids themselves. They pass the domain event; the writer supplies
wire identity and causality metadata.

The main agent uses `agent_id = "main"`. Child agents use their stable
`agent_*` id. Resumed child agents open the same child wire and continue from
the next sequence number.

## Reader API

Add a wire reader that returns envelopes:

```rust
pub struct AgentWireReader;

impl AgentWireReader {
    pub async fn read_wire(path: impl AsRef<Path>) -> Result<Vec<WireEvent>, SessionError>;
    pub async fn replay_messages(path: impl AsRef<Path>) -> Result<Vec<AgentMessage>, SessionError>;
    pub async fn replay_context(path: impl AsRef<Path>) -> Result<AgentContext, SessionError>;
    pub async fn replay_transcript(path: impl AsRef<Path>) -> Result<TranscriptReplay, SessionError>;
}
```

`replay_messages` and `replay_context` project from `WireEvent.event`, so the
runtime behavior stays familiar. `replay_transcript` consumes the full envelope
metadata to rebuild UI entries and child transcript structure.

## Child Transcript Replay

Child replay produces a transcript model, not just messages:

```rust
pub struct ChildTranscriptReplay {
    pub agent_id: AgentId,
    pub status: ReplayedAgentStatus,
    pub entries: Vec<ChildTranscriptEntry>,
    pub latest_snapshot: AgentSnapshot,
    pub last_seq: u64,
}
```

Replay rules:

- `TextDelta` updates the active assistant text block.
- `ThinkingDelta` updates the active thinking block.
- `ToolExecutionStarted` creates or updates a running child tool row.
- `ToolExecutionUpdate` updates partial tool output.
- `ToolExecutionFinished` marks the tool row done or failed.
- `MessageAppended` finalizes durable message content when present.
- `TokenUsage` updates child token counters.
- `Error` appends an error row and marks the child degraded.
- `TurnFinished` closes the active turn.

The replay engine must be tolerant of partial trailing records. A child wire
can end mid-turn if Neo was killed. In that case the child replay is valid but
the child status becomes interrupted/resumable.

## TUI Behavior

The TUI remains chat-first. Delegate and DelegateSwarm cards stay inside the
normal transcript. There is no separate multi-agent dashboard.

Default collapsed DelegateSwarm card:

```text
DelegateSwarm · running · research docs · 5 agents · 2 run · 2 done · 1 wait · progress [...] 64% · bayes estimate · max 3
│ swarm_...
├─ child A [done] ...
├─ child B [running] ...
└─ child C [interrupted · resumable] ...
```

Expanded child rows can show replayed child transcript entries under each
agent. The default view should stay compact enough for long chats. Full child
details appear only when expanded, copied, exported, or requested via task
browser/RPC.

The existing user preference remains binding: tree connector glyphs use muted
text color, while Delegate labels and names retain their highlight styles.

## Restore Semantics

On `neo resume`, Neo loads the main wire and `state.json`, then restores the
multi-agent runtime and TUI transcript:

1. Replay main wire into the main transcript.
2. Restore parent-level Delegate and DelegateSwarm snapshots.
3. Load every child record from `state.json`.
4. Replay each child wire into a `ChildTranscriptReplay`.
5. Merge child transcript status into the corresponding `AgentSnapshot`.
6. Update Swarm cards from the restored child snapshots.

Finished terminal states remain terminal:

- completed
- failed
- cancelled
- timed out

Unfinished states become interrupted/resumable:

- queued
- running
- backgrounded

This replaces the current generic lost behavior for readable child wires. A
child should be marked lost only when Neo cannot read or reconcile its durable
child wire.

## Interrupted and Resumable State

Add explicit lifecycle vocabulary:

```rust
AgentLifecycleState::Interrupted
AgentTerminalReason::ProcessExited
```

`Interrupted` is not the same as `Failed`. It means:

- the old in-process runtime is gone
- the durable child wire is readable
- Neo can continue the child with `Delegate(resume = "...")` or
  `DelegateSwarm(resume_agent_ids = {...})`

The user-facing copy should be direct:

```text
Interrupted because the previous Neo process exited.
Resume with Delegate(resume="agent_x", task="continue").
```

If the child wire is missing or corrupted, use `Lost` or a failed state with a
clear diagnostic. Do not offer a resume hint that cannot work.

## Resume Flow

When resuming a child agent:

1. Validate that the target `agent_id` exists in `state.json`.
2. Validate that it is a subagent owned by the current session.
3. Read its child wire as `WireEvent` records.
4. Replay messages into child `AgentContext`.
5. Replay transcript into a child activity summary.
6. Open the same child wire for append.
7. Start a new child turn with the same `agent_id`.
8. Continue `seq` from the last valid wire event.
9. Emit parent-level `DelegateUpdated` or `DelegateSwarmUpdated` events.
10. On completion, emit the usual parent-level finished event.

The same child agent identity continues across turns:

```text
agents/agent_x/wire.jsonl
  evt_agent_x_1   first run starts
  evt_agent_x_2   first run tool starts
  ...
  evt_agent_x_48  process interrupted
  evt_agent_x_49  resumed turn starts
  ...
```

The resumed child is not a new agent. It is the same agent with a new turn in
the same wire.

## Error Handling

- Metadata line says schema v2 but a record is raw `AgentEvent`: fail closed
  with a migration error.
- Metadata line says schema v1 raw event wire: runtime should require
  migration rather than silently using the old path.
- Envelope `agent_id` does not match the wire path: fail closed.
- Envelope `seq` is duplicate or out of order: stop replay at the last valid
  event and mark the child interrupted with a diagnostic.
- Child wire is missing for a child listed in `state.json`: mark the child
  lost and do not offer resume.
- Child wire contains a partial final line: ignore that line, keep prior valid
  events, and mark interrupted/resumable if the child was unfinished.
- Parent swarm references a child missing from `state.json`: preserve the
  parent snapshot but show the child as lost with a diagnostic.

## Migration

This feature should be delivered with a one-shot migration from current
`AgentEvent` wire records to `WireEvent` records.

Migration rules:

- Read each existing agent wire.
- Skip metadata.
- Wrap each raw `AgentEvent` with generated envelope metadata.
- Use the agent id implied by the wire path.
- Generate `seq` in file order.
- Generate `event_id = evt_<agent_id>_<seq>`.
- Infer `causal_id` and `parent_event_id` using the same writer rules.
- Write a new v2 wire.
- Replace the old wire atomically after successful validation.

After migration, runtime code should expect v2 `WireEvent` records. Neo should
not keep a long-term branch that reads raw records in normal operation.

## Testing

Use narrow tests that prove each touched boundary:

- session wire writer wraps events with stable `event_id`, `agent_id`,
  `turn_id`, and `seq`
- writer continues `seq` after reopening an existing child wire
- reader rejects mismatched `agent_id` vs wire path
- child replay reconstructs text, thinking, tool start/update/finish, token
  usage, error, and turn finish entries
- partial trailing child wire marks child interrupted/resumable
- restored running child with readable wire becomes interrupted/resumable
- restored running child with missing wire becomes lost
- `Delegate(resume = "...")` appends to the same child wire and preserves
  `agent_id`
- `DelegateSwarm(resume_agent_ids = {...})` resumes children under the same
  swarm card
- TUI transcript store updates the same DelegateSwarm card and shows replayed
  child transcript details when expanded

Verification should follow Neo's narrow-test rule. Prefer exact unit or target
filters over broad `cargo test`.

## Rollout Order

This spec is design-only. A later implementation plan should split work into
small, verifiable phases:

1. Add `WireEvent`, id newtypes, writer metadata, and reader envelope parsing.
2. Migrate main and child wire writing from raw `AgentEvent` to `WireEvent`.
3. Add child transcript replay.
4. Add interrupted/resumable lifecycle state and restore semantics.
5. Wire child transcript replay into Delegate and DelegateSwarm TUI cards.
6. Update resume flows to append to the same child wire with continued `seq`.
7. Add one-shot migration and remove raw-event runtime read paths.

## Future: True Live Runtime Reattach

True live runtime reattach is intentionally deferred. It requires a local
supervisor or daemon that owns child runtime tasks independently of the TUI/CLI
process:

```text
neo daemon / local supervisor
  owns child runtimes
  writes wire logs
  exposes local authenticated RPC
  lets TUI/CLI reconnect to live tasks
```

That is a larger app-server or daemon design with security, lifecycle,
cross-platform process management, and local authentication implications. The
event-sourced wire model in this spec is compatible with that future, but does
not require it.

## Success Criteria

- A restored DelegateSwarm card can show all children from durable wires.
- Child transcript detail is reconstructed from child wire events, not from
  lossy snapshot summaries alone.
- Interrupted children are clearly resumable when their wires are readable.
- Resuming a child keeps the same `agent_id` and appends to the same wire.
- Event ids remain stable across replay, export, and resume.
- Raw `AgentEvent` runtime persistence is migrated away rather than preserved
  as a parallel long-term path.
