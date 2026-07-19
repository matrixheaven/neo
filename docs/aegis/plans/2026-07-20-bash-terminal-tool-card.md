# Bash and Terminal Tool Call Presentation Implementation Plan

**Goal:** Implement the approved shell tool-call presentation so top-level
`Bash` and `Terminal` operations remain auditable, syntax-highlighted, and
width-safe, while Delegate-family shell summaries retain both command ends
without changing card structure or persisted state.

**Architecture:** `ToolCallComponent` remains the only stateful card owner. A
new private transcript module derives shell presentation rows from the existing
arguments, result, details, theme, width, and global expansion flag. Existing
generic header/body renderers remain canonical for every other tool and for
malformed legacy shell entries that cannot produce a structured shell view.

**Tech Stack:** Rust 2024, Neo transcript primitives (`Line`, `Span`, `Text`),
the existing syntect-backed `highlight_code_lines`, existing ANSI sanitization,
and the current `ToolCallState` / `AgentActivityEntry` contracts. No dependency,
parser, theme, event, schema, or persistence change is authorized.

**Baseline / Authority Refs:**

- `docs/aegis/specs/2026-07-20-bash-terminal-tool-card-brief.md`
- `docs/aegis/specs/2026-07-19-transcript-overflow-tool-results-design.md`
- `docs/aegis/specs/2026-07-17-ctrl-o-review-chrome-design.md`
- Current source owners in `crates/neo-tui/src/transcript/`,
  `crates/neo-tui/src/markdown.rs`, and
  `crates/neo-agent-core/src/multi_agent/runtime.rs`

**Compatibility Boundary:** Keep tool schemas, runtime execution, admission,
timeouts, guardian behavior, approval/permission policy, events, replay data,
global `Ctrl+O`, Delegate-family row budgets, and multi-agent snapshot shapes
unchanged. Old or malformed replay entries may use the existing generic body;
no second shell-card state or compatibility renderer is added.

**TDD Route:**

- Mode: `off`
- Decision: `skipped`
- Strict authority: `not applicable`
- Test posture: `post-change regression`
- Reason: the approved spec fixes the behavior and the project does not request
  strict test-first TDD for this work.
- Verification: exact `neo-tui` library/integration tests and one exact
  `neo-agent-core` library test, followed by touched-file rustfmt and diff
  checks.

## Scope Check

**Aegis Visibility:** Planning is useful because one user-visible transcript
change crosses the top-level tool-card renderer and the separately bounded
multi-agent activity-summary projection, while both persistence contracts must
remain unchanged.

### Plan Basis

- Fact: `ToolCallComponent::render_with_theme` owns header, body, live output,
  queue metadata, replay rendering, and the global expansion flag.
- Fact: Bash and Terminal are not generic grouped tools; they already render
  through `ToolCallComponent`.
- Fact: `highlight_code_lines(..., "command.sh", theme)` selects the existing
  Bash grammar and already falls back to character-identical plain spans.
- Fact: `markdown::wrap_spans` already performs styled visible-width wrapping
  and hard-wraps unbroken tokens, but is currently private.
- Fact: `summarize_tool_arguments` has six live/folded call sites and currently
  applies a 96-character prefix-only projection without knowing the tool name.
- Fact: Delegate/Swarm renderers currently apply an additional prefix-only
  truncation after the core summary, which would otherwise discard the newly
  retained command tail.
- Assumption: canonical tool names remain exactly `Bash` and `Terminal`; all
  other tools keep the existing projection.
- Assumption: the current on-disk Terminal argument/detail fields are the
  presentation input, but `crates/neo-agent-core/src/tools/terminal.rs` itself
  is outside this plan and may contain concurrent user work.
- Unknowns: none block implementation. Missing structured fields must be
  omitted or routed to the existing generic fallback rather than inferred from
  human-readable result text.

### BaselineUsageDraft

- Required baseline refs: approved shell card brief, transcript overflow/card
  output lock, shared Ctrl+O review semantics, current transcript renderer,
  markdown highlighter, Terminal typed fields, and multi-agent summary owner.
- Delivered context refs: approved spec plus current source and call-path audit.
- Acknowledged before plan refs: all required refs above.
- Cited in plan refs: all required refs above.
- Missing refs: none.
- Decision: `continue`.

### Requirement Ready Check

- Requirement source refs: approved shell tool-card Spec Brief.
- Goals and scope refs: Spec Brief Goal, Decisions, Ownership and File Boundary,
  Non-Goals, and Acceptance sections.
- User / scenario refs: users auditing long Bash commands, Terminal PTY input,
  and child-agent shell activity without losing dangerous suffixes.
- Requirement item refs: top-level syntax-highlighted command body, Terminal
  operation rows, head/omission/tail preview, shared expansion, and bounded
  child summaries.
- Acceptance / verification criteria refs: all ten Spec Brief acceptance items.
- Open blocker questions: none.
- Decision: `ready`.

### Ripple Signal Triage

- Signal: `summarize_tool_arguments` feeds both live child snapshots and folded
  replay/progress activity; its string is then consumed by Delegate,
  DelegateGroup, and DelegateSwarm presentation.
- Downstream consumers: six core event call sites, `child_activity.rs`, and
  three tool-status branches in `swarm_card.rs`.
- Required response: change generation and final bounded presentation together,
  then verify Delegate and Swarm with the same shell summary fixture.
- Scope result: contained to the existing summary and transcript presentation
  owners; no schema or runtime expansion.

### Change Necessity

- User-visible need: the current generic header silently hides most shell
  commands and can omit the dangerous suffix.
- No-change / non-code option: documentation cannot make truncated runtime UI
  text inspectable or width-safe.
- Why code change is necessary: command text must move out of the one-row header
  and be reflowed at render time; child summaries must change their bounded
  projection.
- Minimum change boundary: one private shell presentation module, narrow wiring
  in existing card owners, and local shell-aware summary projection in core and
  Delegate-family text compaction.
- Decision: `code-change`.

### Existence Check

- Proposed new surface: `transcript/shell_tool_presentation.rs`, a private pure
  renderer.
- Existing owner / reuse candidate: `tool_renderers.rs` already renders generic
  tool headers and bodies.
- Why existing surface is insufficient: it is already about 1,216 lines and
  owns many unrelated tool presentations; adding structured Bash/Terminal
  parsing, highlighting, wrapping, elision, and PTY input escaping there would
  add another mixed responsibility under strong complexity pressure.
- Creation proof: the new module has exactly one responsibility, no state, no
  public API, and is reached only through `ToolCallComponent` wiring.
- Entropy / retirement impact: retires shell command-in-header formatting; does
  not create a second card architecture or persisted artifact.
- Decision: `add-with-proof`.

### Architecture Integrity Lens

- Invariant: state and lifecycle stay in `ToolCallComponent`; presentation is
  recomputed from raw existing state on every width/theme render.
- Canonical owner / contract: top-level card state remains
  `ToolCallComponent`; child summary state remains `AgentActivityKind::Tool`.
- Responsibility overlap: avoided by routing shell-only rows to one pure module
  and leaving generic result preview in `tool_renderers.rs`.
- Higher-level simplification: expose the existing styled wrapper and one
  themed text-preview wrapper instead of duplicating wrapping/result preview.
- Retirement / falsifier: if the new module requires state, runtime fields, or
  a second expansion control, return to design; those are out of scope.
- Verdict: existing owner plus a private render helper is the smallest stable
  architecture.

### Complexity Budget

- Artifact class: `Source Complexity`.
- Target files / artifacts: `tool_renderers.rs` (~1,216 lines),
  `multi_agent/runtime.rs` (~3,136 lines), and the new private renderer.
- Current pressure: both existing owners are over the strong 1,200-line signal.
- Projected post-change pressure: wiring-only/local projection changes in the
  oversized files; the new rendering responsibility lives in a focused module.
- Budget result: `at-risk`.
- Planned governance: no new responsibility in the oversized owners; keep
  `tool_renderers.rs` changes to routing/shared wrappers and `runtime.rs`
  changes to the existing summary helper and its six call sites.

### Plan-Time Complexity Check

- Target files: `tool_renderers.rs`, `tool_call.rs`, `runtime.rs`,
  `tool_cards.rs` (~1,381 lines), and `multi_agent_transcript.rs` (~3,197 lines).
- Existing size / shape signals: generic renderers and integration-test files
  are already large and mixed-purpose.
- Owner fit: shell rendering belongs in a new private module; shell summary
  generation and final child-row bounding remain with their existing owners.
- Add-in-place risk: another large special-case block in `tool_renderers.rs`,
  or duplicate new integration tests for every lifecycle state.
- Better file boundary: private source module plus replacement/extension of
  existing tests; add only one focused core unit test.
- Recommendation: `extract helper` for shell presentation, `wiring-only` in
  generic renderers, and `local-fix-without-new-responsibility` in core/child
  summary owners.

### Plan Pressure Test

- Owner / contract / retirement: one state owner, one render-only helper, and
  explicit retirement of command-in-header/prefix-only shell summary behavior.
- Architecture integrity / higher-level path: existing highlighter, wrapper,
  result preview, expansion state, and activity schema are reused.
- Verification scope: exact tests cover top-level shell cards, Terminal modes,
  replay/resize/expansion, width, failure fallback, and Delegate/Swarm summary.
- Task executability: every task below names files, signatures, algorithms,
  exact tests, and stop conditions.
- Pressure result: `proceed`.

## File Map

| File | Action | Boundary |
| --- | --- | --- |
| `crates/neo-tui/src/transcript/shell_tool_presentation.rs` | Create | Pure Bash/Terminal argument projection, header metadata, syntax highlighting, styled wrapping, command elision, Terminal input escaping, and mode-specific result rows. |
| `crates/neo-tui/src/transcript/mod.rs` | Modify | Declare the private module only. |
| `crates/neo-tui/src/markdown.rs` | Modify | Make `wrap_spans` `pub(crate)`; do not change its algorithm. |
| `crates/neo-tui/src/transcript/tool_renderers.rs` | Modify | Skip generic shell key arguments, consume shell header metadata, expose workspace-relative and themed result-preview helpers. |
| `crates/neo-tui/src/transcript/tool_call.rs` | Modify | Route shell body rendering before the generic body while preserving queue/live/result ordering. |
| `crates/neo-tui/src/transcript/child_activity.rs` | Modify | Apply shell-aware head-tail compaction inside the actual row width. |
| `crates/neo-tui/src/transcript/swarm_card.rs` | Modify | Reuse the same bounded shell status projection in collapsed/expanded child summaries. |
| `crates/neo-agent-core/src/multi_agent/runtime.rs` | Modify | Make the existing 96-character tool summary projection tool-name aware at six call sites. |
| `crates/neo-tui/tests/tool_cards.rs` | Modify | Replace obsolete header assertions and extend existing Bash/Terminal/width/replay/expansion regressions. |
| `crates/neo-tui/tests/multi_agent_transcript.rs` | Modify | Extend the existing Delegate/Swarm shell-row regression; do not add a new target. |

Explicitly do not modify or stage `crates/neo-agent-core/src/tools/terminal.rs`,
`docs/en/reference/tools.md`, or `docs/zh/reference/tools.md`; those files have
pre-existing user changes outside this plan.

## Execution Readiness View

- Intent Lock: make model-issued shell operations inspectable without width
  failures or hidden command suffixes.
- Scope Fence: transcript presentation and bounded child summary text only;
  runtime, policy, execution, schemas, and persistence are excluded.
- Baseline Lock: approved 2026-07-20 Spec Brief plus existing overflow and
  global Ctrl+O contracts.
- Approved Behavior: syntax-highlighted top-level command bodies, safe wrapping,
  head/omission/tail collapse, Terminal operation rows, and 96-character child
  head-tail summaries.
- Owner / Contract Constraints: `ToolCallComponent` and
  `AgentActivityKind::Tool` remain canonical; the new module is stateless and
  private.
- Compatibility Boundary: existing events, session JSONL, replay arguments,
  result details, card row counts, and shortcuts remain valid.
- Retirement Boundary: remove Bash/Terminal command text from generic headers
  and retire prefix-only shell elision; retain generic fallback only when a
  structured shell projection cannot be derived.
- Task Batches: shared renderer, Bash/Terminal wiring, child-summary projection,
  then focused verification and one commit.
- Test Obligations: exact selectors only; no package-wide or workspace-wide
  Cargo command.
- Review Gates: line-width invariant, character-identical highlighted text,
  failure/background visibility, unchanged Delegate/Swarm structure, and no
  schema/state growth.
- Drift / Rewind Rules: stop and return to design if implementation needs a new
  event, persisted field, parser/dependency, expansion state, or runtime change.
- Evidence Required Before Completion: exact tests, touched-file rustfmt,
  scoped diff check, staged-path review, and a single conventional commit.
- Advisory Boundary: method-pack execution guidance only; not `GateDecision`,
  `PolicySnapshot`, or completion authority.

## Task 1: Add the Private Shell Presentation Engine

**Files:**

- Create: `crates/neo-tui/src/transcript/shell_tool_presentation.rs`
- Modify: `crates/neo-tui/src/transcript/mod.rs`
- Modify: `crates/neo-tui/src/markdown.rs`
- Modify: `crates/neo-tui/src/transcript/tool_renderers.rs`

**Why:** Commands need syntax-aware, width-safe rendering from existing raw
arguments, but the large generic renderer must not acquire another full tool
presentation responsibility.

**Change Necessity:** Existing generic key-argument formatting truncates before
rendering and cannot produce styled multiline rows. The minimum source boundary
is one private pure module plus visibility-only reuse of existing helpers.

**Impact / Compatibility:** No state or data contract changes. Highlighting is
presentation-only; plain fallback must reproduce the sanitized characters.

**Implementation:**

1. Change only the visibility of the existing markdown helper:

   ```rust
   pub(crate) fn wrap_spans(spans: &[Span], max_width: usize) -> Vec<Vec<Span>>
   ```

   Keep its current visible-width and hard-wrap behavior byte-for-byte.

2. Add `mod shell_tool_presentation;` to `transcript/mod.rs`; do not re-export
   the module outside `transcript`.

3. In `tool_renderers.rs`, change `make_workspace_relative` to
   `pub(super)` and add one narrow themed wrapper around the existing result
   preview:

   ```rust
   pub(super) fn render_text_preview_themed(
       text: &str,
       expanded: bool,
       width: usize,
       theme: &TuiTheme,
   ) -> Vec<Line>
   ```

   It must call `render_result_preview` with `ToolBodyPalette::themed(theme)`;
   do not duplicate output preview limits or wrapping.

4. Give the new module exactly two sibling-visible entry points:

   ```rust
   pub(super) fn header_metadata(
       state: &ToolCallState,
       theme: &TuiTheme,
   ) -> Option<Vec<Span>>;

   pub(super) fn render_body(
       state: &ToolCallState,
       expanded: bool,
       width: usize,
       theme: &TuiTheme,
       workspace_dir: Option<&Path>,
   ) -> Option<Vec<Line>>;
   ```

   Return `None` for non-shell tools and for shell entries where neither typed
   arguments nor a safe structured fallback can be derived.

5. Parse only existing typed fields:

   - Bash: `command`, `cwd`, `run_in_background`, `description`.
   - Terminal: `mode`, `command`, `handle`, `input`, `cwd`, `cols`, `rows`.
   - Result details: `handle`, `output`, `outcome`, `task_id`, `description`,
     `cols`, and `rows` when present.

   Parse complete JSON first. During streamed/incomplete arguments, reuse
   `partial_json::extract_partial_string_field` for `command`, `cwd`, `mode`,
   `handle`, and `input`. Never parse `cd` or a path from command text.

6. Sanitize command display text with the existing shell-output sanitizer
   before highlighting. Remove ANSI/non-printing control effects according to
   existing transcript policy; do not mutate `ToolCallState.arguments`.

7. Highlight commands with:

   ```rust
   highlight_code_lines(&sanitized_command, "command.sh", theme)
   ```

   Reconstructing all returned span text must equal the sanitized command text
   by logical line. Do not add a custom theme or syntax fallback abstraction;
   `highlight_code_lines` already owns fallback.

8. For each highlighted logical line, call `wrap_spans` with the width remaining
   after the four-column body prefix. Prefix only the first visual command row
   with `"  $ "`; prefix later logical or width-generated rows with `"    "`.
   Never emit a `Line` containing an embedded newline.

9. In collapsed mode, show all command rows when there are at most four. For
   longer commands, retain rows `0..3`, append a muted omission row, then retain
   the final row. Compute the hidden character count from omitted span text and
   render:

   ```text
       ... N characters hidden · ctrl+o to expand
   ```

   Expanded mode renders every wrapped row. Recompute on every render width;
   do not cache spans or wrapped rows.

10. Render a typed `cwd` as a muted `"  cwd <relative>"` row before the command
    using `make_workspace_relative`. Omit the row when the typed field is absent.

11. Add one internal unit test named
    `command_preview_preserves_text_head_tail_and_width`. Its fixture must include
    multiline shell text, quoting, Unicode, an ANSI sequence, and one unbroken
    token longer than the available width. Assert reconstructed visible text,
    the retained first/final command fragments, the omission hint, expanded
    completeness, and `visible_width(row) <= width` for narrow and wide widths.

**Verification:**

```bash
rtk cargo test --package neo-tui --lib -- transcript::shell_tool_presentation::tests::command_preview_preserves_text_head_tail_and_width --exact --nocapture
```

**Stop:** The helper is private and stateless, uses existing highlighting and
wrapping, and every generated row is width-safe.

## Task 2: Route Top-Level Bash Through the Shell Body

**Files:**

- Modify: `crates/neo-tui/src/transcript/tool_renderers.rs`
- Modify: `crates/neo-tui/src/transcript/tool_call.rs`
- Modify: `crates/neo-tui/tests/tool_cards.rs`

**Why:** Bash commands must leave the shortened one-row header and remain stable
across preparing, queued, running, and terminal states without hiding output or
failure reconstruction.

**Change Necessity:** The new renderer cannot affect users until the canonical
card owner routes Bash header/body presentation through it. The minimum change
is two wiring branches and focused regression updates.

**Impact / Compatibility:** Queue chips, status symbols/colors, live output,
result previews, failure reconstruction, background results, replay, and global
expansion remain on their current paths.

**Implementation:**

1. In `tool_header_spans_with_elapsed`, call
   `shell_tool_presentation::header_metadata` after the common status/name
   spans. When it returns `Some`, append those spans and skip only
   `extract_key_argument`; continue through the existing list/result chip logic
   for all tools.

2. For Bash metadata:

   - preparing/running: no command text in the header;
   - foreground terminal states with non-empty result: append ` · N lines`;
   - `details.outcome == "backgrounded"`: append ` · background` instead of a
     line count.

3. In `ToolCallComponent::render_with_theme`, keep the existing header,
   `ExitPlanMode`, and Write/Edit streaming branches. In the ordinary body
   branch, use:

   ```rust
   if let Some(shell_rows) = shell_tool_presentation::render_body(
       &self.state,
       self.expanded,
       width,
       theme,
       self.workspace_dir.as_deref(),
   ) {
       rows.extend(shell_rows);
   } else {
       rows.extend(render_tool_body_themed(...));
   }
   ```

   Keep running live output appended after this body exactly as today.

4. Bash `render_body` must append the existing themed result preview after the
   cwd/command rows. For background results, replace the verbose structured
   result with one muted row derived from `task_id` and `description`, for
   example `"  task task_42 · Running development server"`. For failed,
   cancelled, timed-out, resource-limited, or malformed results, preserve the
   current authoritative generic/failure text rather than returning an empty
   successful-looking body.

5. Replace `long_command_header_keeps_closing_paren` with
   `bash_tool_card_renders_command_body_across_lifecycle_states`. Use a small
   table of `ToolCallState` fixtures to assert:

   - headers contain status/name but no command parentheses;
   - preparing, queued, running, succeeded, failed, cancelled, and background
     states retain a `$` command row;
   - failure/background text remains visible after the command region.

6. Update `bash_queue_event_renders_position_and_wait_in_original_card` so its
   header assertion is `Queued Bash · #2 · waiting 18s`, its body assertion is
   `$ cargo test`, and only one Bash card exists.

7. Extend `transcript_pane_expansion_reaches_rendered_bash_tool_body` with a
   command that wraps beyond four visual rows. In collapsed mode assert both
   command suffix omission and output omission; after the existing global
   expansion toggle, assert the complete command and final output line are both
   visible and no expansion hint remains.

8. Add `bash_tool_card_replay_resize_and_expansion_use_original_arguments`.
   Replay an assistant Bash tool call plus result into a pane, render at narrow
   and wide widths, and assert the same original command fragments reflow
   without stored styling or altered arguments.

9. Extend `tool_card_lines_do_not_exceed_terminal_width_after_gutter` with a
   long Bash command containing an unbroken token; retain the existing final
   frame/gutter width invariant rather than adding a duplicate width test.

10. Keep `bash_shell_failure_summary_survives_empty_tool_result_finish`
    unchanged and run it as a regression proving shell presentation cannot
    suppress reconstructed failures.

**Verification:**

```bash
rtk cargo test --package neo-tui --test tool_cards -- bash_tool_card_renders_command_body_across_lifecycle_states --exact --nocapture
rtk cargo test --package neo-tui --test tool_cards -- bash_queue_event_renders_position_and_wait_in_original_card --exact --nocapture
rtk cargo test --package neo-tui --test tool_cards -- transcript_pane_expansion_reaches_rendered_bash_tool_body --exact --nocapture
rtk cargo test --package neo-tui --test tool_cards -- bash_tool_card_replay_resize_and_expansion_use_original_arguments --exact --nocapture
rtk cargo test --package neo-tui --test tool_cards -- tool_card_lines_do_not_exceed_terminal_width_after_gutter --exact --nocapture
rtk cargo test --package neo-tui --test tool_cards -- bash_shell_failure_summary_survives_empty_tool_result_finish --exact --nocapture
```

**Stop:** Bash command text is never placed in the header, every lifecycle state
remains meaningful, and command/output expansion uses only the existing global
flag.

## Task 3: Render Terminal Operations as Distinct Auditable Rows

**Files:**

- Modify: `crates/neo-tui/src/transcript/shell_tool_presentation.rs`
- Modify: `crates/neo-tui/tests/tool_cards.rs`

**Why:** Terminal operations have different audit needs: `start` launches shell
text, `write` sends raw PTY input, `read` shows output, `resize` changes geometry,
and `stop` terminates a process tree.

**Change Necessity:** Generic argument/result rendering cannot safely distinguish
control input from shell source or remove Terminal's structured result wrapper.
The minimum change stays inside the shell presentation module.

**Impact / Compatibility:** Terminal runtime modes and result fields are read,
not changed. Missing fields are omitted; no result text parsing is allowed.

**Implementation:**

1. `header_metadata` returns Terminal metadata as muted spans in this order:
   ` · <mode>` and then ` · <handle>` when available. Read `handle` from typed
   arguments for write/read/resize/stop and from structured result details for
   completed start.

2. Render modes as follows:

   - `start`: optional cwd, highlighted/wrapped command, then structured
     `details.output` through `render_text_preview_themed`; when successful with
     no output, show muted `Terminal started.`.
   - `write`: one `stdin › <escaped-input>` row, followed by non-empty structured
     output preview. Never syntax-highlight input.
   - `read`: structured output preview only.
   - `resize`: one `size <cols> × <rows>` row from typed arguments; omit missing
     dimensions rather than parsing result text.
   - `stop`: non-empty structured output preview followed by
     `Process tree stopped.` for a successful structured result.

3. Escape Terminal input into visible text before wrapping:

   - `\n`, `\r`, and `\t` use those literal escapes;
   - ESC uses `\x1b`;
   - other C0/C1 controls use Rust's visible default escape;
   - printable Unicode remains unchanged.

   Wrap the escaped text after the fixed `"  stdin › "` prefix and use a
   continuation indent of the same visible width. The generated spans must not
   contain live terminal-control bytes.

4. When structured Terminal details are absent or the operation is unknown,
   return `None` or append the existing generic result preview so replayed
   legacy/errors remain inspectable. Never reconstruct `handle`, dimensions, or
   output by parsing the human-readable result string.

5. Add one integration test named
   `terminal_tool_card_renders_operation_specific_body`. Use subcases for all
   five modes and assert:

   - `start.command` uses `$` rows and Bash styling path;
   - `write.input` containing CR, newline, ESC, Ctrl+C, and Unicode is visible as
     escaped audit text with no raw control bytes;
   - `read` shows output without the structured `handle/status/output:` wrapper;
   - `resize` shows the requested dimensions;
   - `stop` shows output plus the terminal stop result;
   - every row stays within narrow and wide widths.

**Verification:**

```bash
rtk cargo test --package neo-tui --test tool_cards -- terminal_tool_card_renders_operation_specific_body --exact --nocapture
```

**Stop:** Every Terminal mode has a distinct inspectable body and PTY input
cannot inject terminal control sequences into the transcript.

## Task 4: Preserve Shell Summary Heads and Tails in Delegate-Family Cards

**Files:**

- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify: `crates/neo-tui/src/transcript/child_activity.rs`
- Modify: `crates/neo-tui/src/transcript/swarm_card.rs`
- Modify: `crates/neo-tui/tests/multi_agent_transcript.rs`

**Why:** Parent cards must remain compact, but prefix-only truncation can hide the
same dangerous command suffix that the top-level card now preserves.

**Change Necessity:** Changing only the core summary is insufficient because the
TUI adds status text and then truncates the complete row from the front again.
Generation and final line-width projection must be fixed together.

**Impact / Compatibility:** `AgentActivityKind::Tool.summary`, the 96-character
limit, 24-entry cap, event flow, row counts, progress, ordering, output previews,
and expansion behavior remain unchanged. The Unicode middle ellipsis keeps the
96-character bound; serialized UTF-8 byte count may differ by at most two bytes
from the old ASCII ellipsis, but no schema or payload field grows.

**Implementation:**

1. Change the core helper signature to:

   ```rust
   fn summarize_tool_arguments(
       name: &str,
       arguments: &serde_json::Value,
   ) -> Option<String>
   ```

2. Update all six call sites: live queued/started, folded queued/started, and
   folded finished/update. For the two `and_then` sites, use a closure that
   passes the event's `name`; do not change upsert/activity signatures.

3. Keep `compact_line` unchanged for non-shell tools. Only when
   `name == "Bash" || name == "Terminal"` and a non-empty top-level `command`
   exists, normalize whitespace and middle-elide to exactly the existing
   96-character maximum. Use a balanced standard-library character projection:
   46 head characters, `" … "`, and 47 tail characters. Short commands remain
   unchanged. Terminal write/read/resize/stop, which have no command, retain the
   existing fallback behavior.

4. Add the core unit test
   `shell_tool_summary_preserves_head_and_tail_within_budget`. Cover Bash and
   Terminal start with a long command and assert the 96-character count, command
   prefix, `" … "`, and `--exact --nocapture` suffix. Also assert a long Read
   path keeps the previous prefix-only `...` behavior.

5. In `child_activity.rs`, add one bounded status formatter that receives the
   verb, tool name, optional summary, fixed suffix, and available character
   budget. For Bash/Terminal, reserve the fixed prefix/parentheses/suffix first
   and middle-elide only the summary to the remaining budget. For every other
   tool, apply existing `compact_chars` to the complete status text.

6. Use that formatter in `render_child_tool_row` with the actual space left
   after indent, marker, and separator. Retain final `truncate_to_width(width)`
   as an invariant guard; do not wrap or add rows.

7. In `swarm_card.rs`, replace the three tool-status `compact_chars(..., 96)`
   paths, including `waiting on`, with the same bounded formatter. Leave task,
   final-text, and fallback-item compaction unchanged.

8. Rename and extend `delegate_and_swarm_render_same_queued_shell_row` to
   `delegate_and_swarm_render_same_bounded_shell_summary`. Use one long command
   ending in `--exact --nocapture` and assert queued and done rows in Delegate,
   collapsed Swarm, and expanded Swarm retain both ends, queue metadata, existing
   output preview, and the same row count. Render at narrow and wide widths and
   assert the final width invariant.

**Verification:**

```bash
rtk cargo test --package neo-agent-core --lib -- multi_agent::runtime::tests::shell_tool_summary_preserves_head_and_tail_within_budget --exact --nocapture
rtk cargo test --package neo-tui --test multi_agent_transcript -- delegate_and_swarm_render_same_bounded_shell_summary --exact --nocapture
rtk cargo test --package neo-tui --test multi_agent_transcript -- option_b_expanded_swarm_preserves_full_child_transcripts --exact --nocapture
```

**Stop:** Nested shell summaries preserve command head and tail within existing
single-line budgets, while non-shell tools and card structure remain unchanged.

## Task 5: Run Focused Verification and Commit the Logical Feature Once

**Files:** All files listed in the File Map, excluding the three pre-existing
out-of-scope modified files.

**Why:** This feature crosses two crates and shared transcript invariants; the
handoff must prove the exact changed behavior without consuming unrelated CI
scope or user work.

**Change Necessity:** Verification and a scoped commit are required by the
project work loop. No additional implementation is authorized in this task.

**Impact / Compatibility:** Commands must name one package, one target, and one
exact test selector. Do not run broad `cargo test`, package-wide nextest, or a
workspace-wide check.

**Implementation:**

1. Run touched-file formatting without rewriting unrelated files:

   ```bash
   rtk rustfmt --check --edition 2024 \
     crates/neo-tui/src/markdown.rs \
     crates/neo-tui/src/transcript/mod.rs \
     crates/neo-tui/src/transcript/shell_tool_presentation.rs \
     crates/neo-tui/src/transcript/tool_renderers.rs \
     crates/neo-tui/src/transcript/tool_call.rs \
     crates/neo-tui/src/transcript/child_activity.rs \
     crates/neo-tui/src/transcript/swarm_card.rs \
     crates/neo-agent-core/src/multi_agent/runtime.rs \
     crates/neo-tui/tests/tool_cards.rs \
     crates/neo-tui/tests/multi_agent_transcript.rs
   ```

2. Run every exact command from Tasks 1-4. A passing result must report one
   selected test, not `0 tests`.

3. Inspect only the authorized diff and whitespace:

   ```bash
   rtk git diff --check -- \
     crates/neo-tui/src/markdown.rs \
     crates/neo-tui/src/transcript/mod.rs \
     crates/neo-tui/src/transcript/shell_tool_presentation.rs \
     crates/neo-tui/src/transcript/tool_renderers.rs \
     crates/neo-tui/src/transcript/tool_call.rs \
     crates/neo-tui/src/transcript/child_activity.rs \
     crates/neo-tui/src/transcript/swarm_card.rs \
     crates/neo-agent-core/src/multi_agent/runtime.rs \
     crates/neo-tui/tests/tool_cards.rs \
     crates/neo-tui/tests/multi_agent_transcript.rs
   rtk git status --short
   ```

4. Inspect the existing staged set before adding anything. If any path is
   already staged, stop and resolve ownership with the user; do not unstage,
   rewrite, or accidentally include it:

   ```bash
   rtk git diff --cached --name-only
   ```

5. Confirm `terminal.rs` and the English/Chinese tool docs remain unstaged.
   Stage only the authorized implementation files:

   ```bash
   rtk git add \
     crates/neo-tui/src/markdown.rs \
     crates/neo-tui/src/transcript/mod.rs \
     crates/neo-tui/src/transcript/shell_tool_presentation.rs \
     crates/neo-tui/src/transcript/tool_renderers.rs \
     crates/neo-tui/src/transcript/tool_call.rs \
     crates/neo-tui/src/transcript/child_activity.rs \
     crates/neo-tui/src/transcript/swarm_card.rs \
     crates/neo-agent-core/src/multi_agent/runtime.rs \
     crates/neo-tui/tests/tool_cards.rs \
     crates/neo-tui/tests/multi_agent_transcript.rs
   rtk git diff --cached --name-only
   rtk git diff --cached --check -- \
     crates/neo-tui/src/markdown.rs \
     crates/neo-tui/src/transcript/mod.rs \
     crates/neo-tui/src/transcript/shell_tool_presentation.rs \
     crates/neo-tui/src/transcript/tool_renderers.rs \
     crates/neo-tui/src/transcript/tool_call.rs \
     crates/neo-tui/src/transcript/child_activity.rs \
     crates/neo-tui/src/transcript/swarm_card.rs \
     crates/neo-agent-core/src/multi_agent/runtime.rs \
     crates/neo-tui/tests/tool_cards.rs \
     crates/neo-tui/tests/multi_agent_transcript.rs
   rtk git diff --cached --stat
   ```

   The cached name list must equal the allowlist above before continuing.

6. Commit the one logical feature:

   ```bash
   rtk git commit -m "feat(tui): improve shell tool call visibility"
   ```

**Verification:** The exact tests, rustfmt check, scoped diff check, staged-path
review, and commit all succeed without staging unrelated work.

**Stop:** One scoped commit contains the complete presentation feature and no
runtime/tool-doc changes.

## Risks and Controls

- **Partial JSON hides a command:** use the existing partial string extractor;
  if extraction still fails, fall back to generic width-safe rendering.
- **Highlighting changes text:** reconstruct span text in the unit test and rely
  on the existing plain fallback owner.
- **Long token exceeds width:** reuse `wrap_spans` and retain the final frame
  width invariant.
- **Command preview displaces failure/output:** shell rows precede, but never
  replace, authoritative Bash result/failure rows.
- **Terminal input controls the user's terminal:** escape control characters
  before constructing spans; assert no raw controls in rendered rows.
- **Core preserves the tail but TUI removes it again:** reserve fixed status
  text before shell-summary elision in both child and Swarm paths.
- **Large files grow further:** keep oversized source files wiring/local-only
  and replace/extend existing integration tests instead of duplicating suites.
- **Concurrent user changes are committed:** stage explicit paths only and
  inspect the cached diff before committing.

## Retirement

- Retire Bash/Terminal command text inside generic `(...)` header arguments.
- Retire prefix-only elision for Bash/Terminal child command summaries.
- Keep generic tool rendering unchanged for non-shell tools.
- Keep the generic shell fallback only for missing/malformed legacy data; it is
  not a second normal presentation path.
- Add no migration or cleanup task because stored arguments/results remain the
  canonical replay source and require no conversion.

## Completion Evidence

Completion requires all exact tests to pass, every rendered test row to satisfy
the width invariant, character-identical command projection through
highlighting/fallback, unchanged Delegate-family structure, and a scoped commit
that excludes the pre-existing Terminal/runtime documentation edits.
