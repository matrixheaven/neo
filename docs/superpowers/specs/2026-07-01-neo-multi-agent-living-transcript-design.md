# Neo Multi-Agent Living Transcript Design

Date: 2026-07-01

Status: Option B approved by the requester; written spec ready for review before
implementation planning.

## 1. Purpose

Neo's Multi-Agent transcript now has the basic Kimi-style shape, but it is still
not a fully living display. A single delegate card can show a short title, role,
recent tool rows, one thinking row, and one final body row. That closes the
first visual regression, but it does not yet match the deeper Kimi behavior:
sub-tool phases, output previews, elapsed-time ticking, grouped same-step
delegates, backgrounded state, and animated swarm progress.

This spec closes all nine gaps found during the Kimi parity study and locks the
approved Option B visual direction: Multi-Agent entries are chat-native
transcript cards, not tool-group-like progress rows. The target is not a
TypeScript port. The target is a Rust-native Neo contract that borrows the same
product semantics:

- child agents feel alive while they run;
- the transcript stays the single source of visible state;
- foreground blocking remains the default;
- background execution is explicit and visible;
- swarm progress moves through real intermediate states rather than prompt
  snapshots or sudden jumps.
- random child-agent display names such as `Nova`, `Iris`, `Vega`, and `Rune`
  are primary visible identity; roles such as `Coder`, `Planner`, and
  `Explorer` are secondary badges and never replace the name.

This is a canonicalization pass. Do not keep dual data models such as
`failed: bool` plus a new phase enum. Replace the old shape with the new
canonical shape and update all local tests/fixtures at once.

## 2. References

Kimi reference implementation:

- `docs/kimi-code/apps/kimi-code/src/tui/components/messages/tool-call.ts`
  - Single `Agent` cards store subagent phase, running/finished sub-tools,
    text, thinking text, token usage, background terminal state, and output
    previews.
  - `buildSingleSubagentBlock` renders recent sub-tool rows, then a fixed
    thinking preview, then the latest output line.
  - Agent cards advertise `Ctrl+B` immediately because subagents are expected
    to be long-running.
- `docs/kimi-code/apps/kimi-code/src/tui/components/messages/agent-group.ts`
  - Multiple `Agent` tool calls in the same model step become one live group.
  - Phase transitions flush immediately; other changes are throttled.
- `docs/kimi-code/apps/kimi-code/src/tui/components/messages/agent-swarm-progress.ts`
  - Foreground swarms render an animated grid, latest child text, status line,
    and progress bar.
- `docs/kimi-code/apps/kimi-code/src/tui/components/messages/agent-swarm-progress-estimator.ts`
  - Swarm progress is a stateful estimator with priors, catch-up, and an
    unfinished progress cap.

Current Neo touchpoints:

- `crates/neo-agent-core/src/multi_agent/state.rs`
  - `AgentSnapshot`, `AgentActivityKind`, `SwarmSnapshot`.
- `crates/neo-agent-core/src/multi_agent/runtime.rs`
  - Child event ingestion, snapshot construction, activity trimming, tool
    argument/result summarization.
- `crates/neo-agent-core/src/multi_agent/progress.rs`
  - Current stateless swarm progress estimate.
- `crates/neo-tui/src/transcript/delegate_card.rs`
  - Current single delegate card.
- `crates/neo-tui/src/transcript/swarm_card.rs`
  - Current swarm card and expanded child rendering.
- `crates/neo-tui/src/transcript/store.rs`
  - Upsert behavior for delegate and swarm entries.
- `crates/neo-tui/src/transcript/pane.rs`
  - `render_tick()` already exists and can mark the transcript dirty.
- `crates/neo-agent/src/modes/interactive/mod.rs`
  - The terminal loop already ticks every 50ms and renders the TUI.

## 3. Gap Matrix

| Gap | Current Neo | Required end state |
| --- | --- | --- |
| 1. Thin live state model | `Tool { id, name, summary, failed }` and text rows | Tool rows carry explicit phase, argument preview, output preview, and stable order |
| 2. No sub-tool output preview | Delegate card only shows one tool summary row | Bash/Terminal/generic tools can show bounded live/final output under the tool row |
| 3. Thinking not fixed-window | One compacted single line from recent activity | Width-aware fixed thinking preview, placed before final output and not duplicated |
| 4. Elapsed does not self-tick | Header relies on runtime snapshots | Live entries mark the transcript dirty on ticks and derive elapsed from start/terminal timestamps |
| 5. Ctrl+B hint is coarse | Running foreground delegate always shows the hint | Agent hint appears only when detachable; backgrounded cards show `Backgrounded` instead |
| 6. Background terminal state is weak | Lifecycle lacks lost/killed/backgrounded nuance | Snapshot records detach source and terminal reason so UI can show backgrounded/lost/killed/timed-out correctly |
| 7. No same-step AgentGroup | Multiple delegates render as separate cards | 2+ root foreground delegates from the same turn render as one live group |
| 8. Swarm progress less alive | Static bars and stateless progress estimate | Stateful estimator, render-tick animation, queued/running/completed catch-up, latest child text |
| 9. Swarm expanded child diverges | Expanded child rows still duplicate old single-card bugs | Swarm expanded children use the same child activity renderer as single delegates |

## 4. Goals

1. Make delegate cards living displays, not compressed prompt snapshots.
2. Replace boolean tool failure state with an explicit tool activity phase.
3. Keep sub-tool output bounded and safe for transcript rendering.
4. Make live elapsed time update without requiring child agent output every
   second.
5. Represent foreground detach and background terminal reasons without
   overloading `completed`.
6. Group same-turn foreground delegates into a compact live group while keeping
   single delegates as standalone cards.
7. Upgrade swarm progress to use stateful estimates and visible animation.
8. Reuse one child activity rendering model for single delegate cards, group
   rows, and expanded swarm children.
9. Preserve foreground-by-default delegation and chat-transcript-native UI.

## 5. Non-Goals

- No separate swarm page or alternate task dashboard for foreground swarms.
- No LLM-generated display titles or display names.
- No compatibility branch for the old `failed: bool` activity shape.
- No new subagent roles in this pass.
- No nested subagent spawning policy changes.
- No hosted collaboration or remote worker surface.
- No git mutations in the implementation workflow unless the user explicitly
  authorizes the exact command.

## 6. Canonical Data Model

### 6.1 Agent Tool Activity

Replace the current boolean failure field with a phase enum:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentToolActivityPhase {
    Ongoing,
    Done,
    Failed,
}
```

Replace the `Tool` variant with this shape:

```rust
pub enum AgentActivityKind {
    Tool {
        id: String,
        name: String,
        summary: Option<String>,
        phase: AgentToolActivityPhase,
        output: Option<AgentToolOutputPreview>,
    },
    Text {
        text: String,
        thinking: bool,
    },
}
```

Add a bounded output preview:

```rust
pub struct AgentToolOutputPreview {
    pub text: String,
    pub is_error: bool,
    pub truncated: bool,
    pub tail: bool,
}
```

Rules:

- `summary` is the short key argument, such as a file path, grep pattern, or
  shell command.
- `output.text` is only for tools whose output is useful in a subagent card:
  `Bash`, `Terminal`, and generic tools without a dedicated compact renderer.
- `output.text` is bounded to a small transcript preview. The implementation
  may keep up to 50k bytes in memory, but the card renders only a few lines.
- `tail = true` means live output should render the most recent lines.
- Terminal tool phase is `Done` or `Failed`. Running tool phase is `Ongoing`.
- There is no `failed: bool` fallback.

### 6.2 Agent Timing And Background Reason

Keep `elapsed` for API compatibility inside current snapshots, but make it a
derived field for UI. Add serializable timestamp fields:

```rust
pub struct AgentSnapshot {
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub started_at_ms: Option<u64>,
    pub terminal_at_ms: Option<u64>,
    pub detached_from_foreground: bool,
    pub terminal_reason: Option<AgentTerminalReason>,
    pub elapsed: Duration,
    ...
}
```

Add terminal reason:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentTerminalReason {
    Completed,
    Error,
    CancelledByUser,
    TimedOut,
    Killed,
    Lost,
}
```

Rules:

- `started_at_ms` is set when a queued delegate starts running.
- `terminal_at_ms` is set exactly once when the agent enters a terminal state.
- `detached_from_foreground` is true only when a foreground delegate was moved
  to background by `Ctrl+B`.
- A detached running delegate displays as `Backgrounded` even though its
  lifecycle state remains `Running`.
- `terminal_reason = Lost` must not render as completed.
- `elapsed` is still populated in tool/API responses, but TUI cards derive live
  elapsed from timestamps while state is not terminal.

## 7. Runtime Event Contract

`MultiAgentRuntime::apply_child_event` is responsible for keeping snapshots live.

Required mappings:

| Event | Snapshot update |
| --- | --- |
| `ToolExecutionStarted` | Upsert tool with `phase = Ongoing`, summary from arguments, no output |
| `ToolExecutionUpdate` | Keep `phase = Ongoing`; update output preview for Bash/Terminal/generic tools |
| `ToolExecutionFinished` success | Upsert same tool with `phase = Done`; increment `tool_count`; set final output preview if useful |
| `ToolExecutionFinished` error | Upsert same tool with `phase = Failed`; increment `tool_count`; set error output preview |
| `ThinkingDelta` | Append/merge thinking text into activity, bounded by activity cap |
| `TextDelta` | Append/merge normal text and update `latest_text` |
| `MessageAppended Assistant` | Store final text without duplicating the previous text row |
| `TokenUsage` | Add usage into `token_count` |
| `Error` | Update latest text, append error text, and prepare terminal error reason if the run ends failed |

Activity trimming remains bounded, but trimming must not remove the only visible
ongoing tool when a long-running tool has produced many text deltas. The trim
policy should preserve:

- the newest ongoing tool per id;
- the newest final text row;
- the newest thinking tail;
- enough recent rows to render the visible card.

## 8. Shared Child Activity View

Create one Rust TUI-side view model used by both single delegates and swarm
expanded children:

```rust
pub struct ChildActivityView<'a> {
    pub tools: Vec<ChildToolRow<'a>>,
    pub thinking: Option<String>,
    pub final_text: Option<String>,
    pub final_is_error: bool,
}

pub struct ChildToolRow<'a> {
    pub name: &'a str,
    pub summary: Option<&'a str>,
    pub phase: AgentToolActivityPhase,
    pub output: Option<&'a AgentToolOutputPreview>,
}
```

The view model owns these display rules:

- show at most four tool rows in collapsed single delegate cards;
- show at most two output preview rows under each visible tool;
- show one thinking block after tool rows;
- preserve recent thinking chunks as a coherent preview, while normal body text
  uses the latest non-thinking child text;
- show one final body row after thinking;
- suppress duplicate final text when `outcome.summary`, `latest_text`, and a
  final text activity carry the same content;
- render successful final body text with `theme.text_primary`, not success
  green;
- render failed final body text with `theme.status_error`;
- make final body the last row in the card.

## 9. Option B Visual System

Option B is the canonical design. Multi-Agent rendering must feel like a set of
living child conversations embedded in the main chat transcript. The card may
show progress, but progress is secondary to readable child-agent activity:
tools, thinking, body text, errors, and final summaries.

The rejected alternatives are documented here so the implementation does not
drift back:

```text
┌─ Multi Agents Card 设计方向 ───────────────────────────────┐
│ A. 进度列表型     Dense, but tool-group-like              │
│ B. Transcript 型  Canonical: child-agent dialogue         │
│ C. Timeline 型    Useful for audit, too cold for live     │
└────────────────────────────────────────────────────────────┘
```

Global visual rules:

- Agent `display_name` is the first human-readable identity in every agent row.
- `role_label` is rendered as a compact secondary badge: `[Coder]`,
  `[Planner]`, `[Explorer]`, etc.
- `task_title` is descriptive metadata. It must never replace or visually
  outrank `display_name`.
- Child tool rows, thinking rows, body rows, and final rows share the same
  grammar across single delegate cards, delegate groups, and expanded swarm
  children.
- Collapsed swarm rows may summarize activity, but expanded swarm rows must use
  the same child transcript renderer as single delegates.
- Width truncation drops task title and optional stats before it drops the
  agent name.
- No separate page, side panel, or `/tasks` view is introduced for this
  foreground transcript experience.

## 10. Child Activity Grammar

All Multi-Agent card bodies use this grammar:

```text
┌─ Child Activity Grammar ───────────────────────────────────┐
│   • Used  Read  path-or-summary                            │
│   • Using Bash  command                                    │
│       live stdout/stderr preview line                      │
│   ✗ Used  Bash  command · exit 101                         │
│       error preview line                                   │
│   ◌ thinking                                               │
│     last 1-2 thinking lines, muted theme                   │
│   │ assistant body / streaming child text                  │
│   └ final child answer or error summary                    │
└────────────────────────────────────────────────────────────┘
```

Rules:

- `Using` is reserved for `AgentToolActivityPhase::Ongoing`.
- `Used` is used for `Done` and `Failed`; failed rows use the error marker and
  error color.
- Tool names are theme brand color.
- Tool summaries and output previews are muted unless the output is an error.
- Thinking is a distinct muted block, not folded into final body text.
- Streaming child body text uses a leading `│`.
- Final child output uses a leading `└` and is the last row for that child.
- The renderer shows the newest useful rows, bounded by
  `MAX_CHILD_TOOL_ROWS`, output preview line limits, and width truncation.

## 11. Delegate Card UI Contract

Single delegate cards are standalone transcript entries for one foreground or
background child agent. The name comes first, the role is a badge, and the
header carries state, task, elapsed time, tool count, and tokens.

Running single delegate:

```text
┌─ Delegate · Single Agent · Running ────────────────────────┐
│ ● Nova  [Coder]  角色对比测试 coder · running · 21s · 22.7k │
│   • Used Read  crates/neo-agent-core/src/tools/delegate.rs │
│   • Used Read  crates/neo-agent-core/src/multi_agent/rt.rs │
│   • Using Bash cargo nextest run -p neo-agent-core ...     │
│       running: cargo nextest run -p neo-agent-core ...     │
│       Compiling neo-agent-core v0.1.0                      │
│   ◌ thinking                                               │
│     Let me verify the state mutation path before editing.  │
│   │ I found the foreground aggregation issue. Next I will...│
│   Ctrl+B detach to background                              │
└────────────────────────────────────────────────────────────┘
```

Completed single delegate:

```text
┌─ Delegate · Single Agent · Completed ──────────────────────┐
│ ✓ Nova  [Coder]  角色对比测试 coder · done · 4 tools · 31s │
│   • Used Read  crates/neo-tui/src/transcript/swarm_card.rs │
│   • Used Edit  crates/neo-tui/src/transcript/swarm_card.rs │
│   • Used Bash  cargo nextest run -p neo-tui --test ...     │
│   └ All edits applied. The card now shows agent name first.│
└────────────────────────────────────────────────────────────┘
```

Rules:

- Header status labels are lowercase display text: `queued`, `running`,
  `backgrounded`, `done`, `failed`, `cancelled`, `timed out`, `lost`, `killed`.
- The marker communicates state without relying on color:
  `●` running/backgrounded, `◌` queued/thinking, `✓` completed, `✗` failed-like.
- The detach hint appears only when the agent is a running foreground delegate
  that can still be detached.
- A detached foreground agent shows `backgrounded` and does not show the detach
  hint.
- A started-in-background agent may still show `running` while active.
- Long task titles are truncated before stats and before agent identity.
- Successful final body text uses `theme.text_primary`, not success green.

The previous abstract header shape is no longer canonical:

```text
● Gibbs Coder Agent Running (Implement Task 1: PlanBox border fix) · 3 tools · 24s · 25.6k tok
```

It may remain useful as a test fixture reference, but implementation should
render the Option B shape where `Gibbs` is the primary identity and `Coder` is a
badge.

## 12. Delegate Group UI Contract

When the same parent turn starts two or more root foreground delegates, replace
the separate standalone cards with one group entry. A group is still a chat
transcript card, not a tool group. Each child row is a miniature agent
transcript with visible name, role badge, stats, and recent activity.

```text
┌─ Delegate Group · Same Turn Multiple Agents ───────────────┐
│ ● Running 2 agents · 1 running · 1 queued · 21s            │
│ ├─ Nova  [Coder]  角色对比测试 coder · 21s · 22.7k tok     │
│ │  • Used Read  crates/neo-agent-core/src/tools/delegate.rs│
│ │  • Using Bash cargo run -p xtask -- test ...             │
│ │      ... 2 earlier lines                                 │
│ │  ◌ thinking                                              │
│ │    The interrupt path needs one narrow assertion.         │
│ │  │ Let me make these edits.                              │
│ └─ Vega  [Explorer]  搜索历史卡片回归点 · queued            │
│    ◌ Waiting for scheduler slot                            │
└────────────────────────────────────────────────────────────┘
```

Rules:

- Grouping only applies to root `Delegate` cards from the same parent turn.
- `DelegateSwarm` children stay inside the swarm card.
- The group persists once formed. It does not split back into standalone cards.
- Phase transitions update immediately.
- Text/tool/token churn may update on render ticks; no unbounded re-render storm.
- Completed/backgrounded group children may collapse to one final row when no
  useful activity preview exists, but name and role remain visible.
- The group header totals tools/tokens for terminal children and shows the max
  elapsed among children.
- If any child can detach, the group may show one shared detach hint at the end.

## 13. Swarm UI Contract

Swarm cards remain transcript-native. They do not move to `/tasks`.
Collapsed swarm keeps the Bayesian progress semantics, but the rows still read
as agents with identities and current activity rather than anonymous progress
items.

Collapsed swarm:

```text
┌─ DelegateSwarm · Collapsed Live ───────────────────────────┐
│ ● Swarm: 角色对比测试 · 4 agents · 2 run · 1 done · 1 wait │
│   progress [■■■■■·····] 54% · bayes estimate · max 2       │
│ ├─ Nova  [Coder]    ● [■■■■■■··] 68% · Using Bash · 22.7k  │
│ ├─ Iris  [Planner]  ✓ [■■■■■■■■] done · 3 tools · 8.2k     │
│ ├─ Vega  [Explorer] ● [■■■■····] 43% · thinking · 14.1k    │
│ └─ Rune  [Coder]    ◌ [········] queued                    │
│   ◌ Working... some agents are still streaming             │
└────────────────────────────────────────────────────────────┘
```

Expanded swarm:

```text
┌─ DelegateSwarm · Expanded Live ────────────────────────────┐
│ ● Swarm: 角色对比测试 · 4 agents · 54% · 41s               │
│   schedule: max 2 concurrent · completed 1/4 · failed 0    │
│                                                            │
│ ├─ Nova  [Coder]  ● running · 21s · 4 tools · 22.7k tok    │
│ │  • Used Read  crates/neo-agent-core/src/tools/delegate.rs│
│ │  • Used Read  crates/neo-agent-core/src/multi_agent/rt.rs│
│ │  • Using Bash cargo nextest run -p neo-agent-core ...    │
│ │      running: cargo nextest run -p neo-agent-core ...    │
│ │      Finished test profile in 38.23s                     │
│ │  ◌ thinking                                              │
│ │    Now I can apply the minimal renderer change.           │
│ │  │ All edits applied. Now let me verify the paths.        │
│ │                                                         │
│ ├─ Iris  [Planner]  ✓ done · 3 tools · 12s · 8.2k tok      │
│ │  • Used Read  docs/superpowers/plans/...                 │
│ │  • Used Read  crates/neo-tui/src/transcript/store.rs     │
│ │  └ The implementation should stay inside transcript cards.│
│ │                                                         │
│ ├─ Vega  [Explorer]  ● running · 19s · 2 tools · 14.1k tok │
│ │  • Used RG  "DelegateGroupComponent|SwarmCardComponent"  │
│ │  ◌ thinking                                              │
│ │    I am checking whether grouped delegates already expose │
│ │    display_name separately from role_label.               │
│ │                                                         │
│ └─ Rune  [Coder]  ◌ queued                                 │
│    ◌ Waiting for scheduler slot                            │
└────────────────────────────────────────────────────────────┘
```

Rules:

- Queued members start at zero visual progress.
- Running members get an initial non-zero tick only after start or child model
  text/tool activity.
- Completed/failed/cancelled members animate to a filled terminal state for a
  short catch-up window.
- The rightmost child label shows latest meaningful activity in this priority:
  ongoing tool, finished tool, final output, latest child text, task title.
- The row must not show the original full prompt after activity exists.
- The bottom status bar is segmented by completed, working, suspended, queued,
  cancelled, and failed counts.
- Progress never reaches 100% while any child is non-terminal.
- Uses the shared child activity renderer from section 8.
- Does not duplicate final text.
- Does not render success summaries in green body text.
- Shows output previews under child Bash/Terminal/generic tool rows.
- Collapsed rows show `display_name` first and role badge second.
- Expanded rows must preserve full child activity, not only the latest summary.

## 14. Status, Theme, And Width Rules

```text
┌─ Status / Theme Rules ─────────────────────────────────────┐
│ ● running/backgrounded: brand color                        │
│ ◌ queued/thinking: muted or warn                           │
│ ✓ completed: status_ok                                     │
│ ✗ failed/timed out/lost/killed: status_error               │
│ Agent display_name: primary identity, colored by status    │
│ [Coder]/[Planner]/[Explorer]: small secondary role badge   │
│ task_title: secondary, never replaces display_name         │
│ tool name: brand, tool result preview: muted/error         │
│ child body: primary, thinking: muted, final error: error   │
└────────────────────────────────────────────────────────────┘
```

Compact width rule:

```text
┌─ Compact Width Rule ───────────────────────────────────────┐
│ ● Nova [Coder] running · 21s · 22.7k                       │
│   • Using Bash cargo nextest run ...                       │
│   ◌ thinking: checking renderer state                      │
│   │ Let me make these edits.                               │
│                                                            │
│ Rule: narrow screens drop task_title first, never drop name.│
└────────────────────────────────────────────────────────────┘
```

Additional rules:

- Every visible row must truncate to the available terminal width.
- Tool output previews use hanging indentation under their tool row.
- Wide CJK or Unicode bar characters must be measured through existing
  `visible_width`/`truncate_to_width` helpers.
- Header stats must survive long titles by truncating the title area first.
- Color is meaningful but not the only state signal; markers and labels must
  also communicate state.

## 15. Stateful Swarm Progress Estimator

Replace the current static-only estimate with a stateful estimator that mirrors
the Kimi shape in Rust:

```rust
pub struct SwarmProgressEstimator {
    members: BTreeMap<String, MemberProgressState>,
}

pub struct SwarmProgressEstimate {
    pub raw_ticks: f32,
    pub display_ticks: f32,
    pub estimated_progress: Option<f32>,
    pub target_progress: Option<f32>,
    pub boosted: bool,
}
```

Behavior:

- `mark_started(member_id, now_ms)` sets at least one raw tick.
- `record_tool_call(member_id, tool_call_id, now_ms)` increments raw ticks only
  once per tool call id.
- `mark_completed`, `mark_failed`, and `mark_cancelled` pin terminal state.
- Completed children provide priors for typical duration and tool-call count.
- Running children can be boosted toward the prior estimate, capped below 85%
  per unfinished child.
- Display ticks catch up gradually so progress does not jump abruptly.
- `has_pending_catchup()` tells the TUI whether render ticks should continue.

## 16. Transcript Tick Contract

`TranscriptPane::render_tick()` already advances an activity frame. Extend it so
live delegate, group, and swarm entries can request redraws:

```rust
pub trait LiveTick {
    fn on_render_tick(&mut self, now_ms: u64) -> bool;
}
```

Practical implementation can be an inherent method on `TranscriptEntry` rather
than a public trait. The requirement is:

- live delegate/group/swarm entries update internal elapsed/progress animation
  state on ticks;
- `render_tick()` marks the transcript dirty when any live entry changed;
- finalized entries do not request redraws;
- active thinking behavior remains intact.

## 17. Test Strategy

Core tests:

- `crates/neo-agent-core/tests/multi_agent_runtime.rs`
  - tool phase transitions from ongoing to done/failed;
  - live output preview is bounded and tail-preserving;
  - timestamps produce live elapsed and fixed terminal elapsed;
  - detach marks `detached_from_foreground`;
  - terminal reason distinguishes completed, failed, timed out, killed, lost.
- `crates/neo-agent-core/tests/multi_agent_roles.rs`
  - update fixtures for canonical `AgentActivityKind::Tool` shape.

TUI tests:

- `crates/neo-tui/tests/multi_agent_transcript.rs`
  - single delegate renders `Using` for ongoing tools from explicit phase;
  - output preview appears under Bash and is bounded;
  - thinking uses a fixed preview window and is not duplicated;
  - final summary is one body-colored row;
  - backgrounded state suppresses the detach hint;
  - same-turn delegates form an agent group;
  - agent display names render before role badges in single, group, and swarm
    cards;
  - swarm rows show latest activity instead of full prompt;
  - expanded swarm children match single delegate rendering rules;
  - narrow widths preserve agent name and role while truncating task title;
  - live ticks mark transcript dirty for elapsed/progress updates.

Focused commands:

```bash
cargo nextest run -p neo-agent-core --test multi_agent_runtime <filter>
cargo nextest run -p neo-agent-core --test multi_agent_roles <filter>
cargo nextest run -p neo-tui --test multi_agent_transcript <filter>
cargo clippy -p neo-agent-core --test multi_agent_runtime -- -D warnings -A clippy::pedantic
cargo clippy -p neo-tui --test multi_agent_transcript -- -D warnings -A clippy::pedantic
```

## 18. Acceptance Criteria

- The nine gaps in section 3 each have a passing regression test.
- Single delegate, delegate group, collapsed swarm, and expanded swarm all use
  Option B transcript-card composition.
- Random agent names are visible as the primary identity in every agent row;
  `Coder`, `Planner`, and `Explorer` never appear as the only visible name.
- No source file keeps both `failed: bool` and `AgentToolActivityPhase` for
  agent activity.
- A long-running foreground delegate continues to show changing elapsed time
  even with no new child output.
- A running Bash sub-tool inside a delegate can show a bounded live output
  preview.
- A completed delegate with identical latest text and summary renders exactly
  one final `└` row.
- A detached foreground delegate says `Backgrounded`, not `Completed`.
- Multiple same-turn root delegates render as one group.
- Swarm progress starts near zero for queued children, moves through
  intermediate states, and does not hit 100% before all children are terminal.
- Expanded swarm child cards and single delegate cards use the same row
  semantics.
- Theme colors follow the section 14 state mapping and every state also has a
  non-color marker or label.

## 19. Self-Review

- Placeholder scan: no placeholder token or incomplete requirement remains.
- Scope check: all requirements are within Multi-Agent runtime state,
  transcript rendering, and focused tests. No provider/model rewrite is
  included.
- Consistency check: sections 6, 8, 10, 11, 12, and 13 all depend on the same canonical
  `AgentToolActivityPhase` and `AgentToolOutputPreview` model.
- Ambiguity check: "backgrounded" is a display phase derived from
  `detached_from_foreground`, not a lifecycle terminal state.
- User-review gate: this file is ready for requester review before the
  implementation plan is written.
