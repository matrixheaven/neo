# Neo 2026-07-26 Final-Review Residuals Design

Date: `2026-07-26`
Status: `approved by user on 2026-07-26 (recommended options; subagent-driven execution)`
ArchitectureReviewRequired: `yes`

## 1. Goal

Close the five residual correctness and ownership gaps left after the
2026-07-25 crates-audit remediation and the 2026-07-26 final review. Each
repair must live in the existing canonical owner, keep Neo local-only and
cross-platform, and delete or permanently segregate any dual owner that the
audit already intended to retire.

Success is not “add a guard.” False auth settlement must stop, Kitty image
identity must not be forgotten without terminal delete, multi-tool stream
finish must not leave open tool lifecycles, HTML vs model-envelope escape
must have one explicit policy each, and delegate discovery must not leak
through any TaskList-facing metadata path.

## 2. Requirement Basis

### 2.1 Source

1. Final multi-subagent review of remediation range `a876b544..73dc5b5c`
   (reports under session scratch `review-*-ce04dcbb.md`).
2. User-facing expansion of the five intentionally deferred items
   (conversation 2026-07-26).
3. Parent design:
   `docs/aegis/specs/2026-07-25-crates-audit-remediation-design.md`
   (F3, F10, F11, C1/C2 contracts remain authoritative where overlapping).
4. Follow-up fix already landed outside this workstream:
   `c6b748b8` (live-steer drop, catalog abort, VT restore retry, notify
   deadline). Those paths are **out of scope** here.

### 2.2 User decisions locked for this design

| # | Residual | Decision |
|---|---|---|
| R1 | OAuth refresh rewrites non-auth failures to `NeedsAuth` | Classify by typed OAuth error kind. Only true re-auth cases settle `needs_auth` without reconnect. Store / transport / flow setup failures remain non-auth and stay reconnectable. |
| R2 | `LiveRenderer::reset` clears Kitty IDs without delete sequences | Reset that forgets image IDs must either emit deletes first or be unreachable after a path that already deleted. Resume / review transitions must not leave ghost placements. |
| R3 | Multi-tool Anthropic finish can open tools then fail without matching `End` | Shared `StreamingToolCallAssembler::finish_all` must not discard `End` for already-started siblings when a later slot lacks a name. Protocol error remains authoritative for the nameless slot. |
| R4 | Model-envelope `xml_escape` vs HTML export dual helpers | Keep two intentional policies: model/XML envelope vs browser HTML. Delete accidental third copies. Document the split. Do not force HTML `&#39;` into model envelopes. |
| R5 | `list_metadata` still enumerates delegate/swarm kinds | Manager metadata used for TaskList discovery must not return delegate/swarm rows. ListDelegates remains the sole discovery owner. BackgroundTaskManager may still track delegate projection state for other reasons if needed, but TaskList-facing enumeration must not surface them. |

### 2.3 Authority

- `AGENTS.md`: scope, canonical owner, cross-platform, exact tests, Git.
- Parent remediation design F3 / F10 / F11 / C2.
- Delegate-family cards: layout/content/expansion unchanged.
- ShellRuntime: no admission or unbounded-timeout change.
- `.references/`: comparison only.

Requirement Ready Check:

- Requirement source refs: complete (final review + user expansion).
- Canonical owners: identified for R1–R5.
- User-facing choices: resolved in §2.2.
- Acceptance criteria: §12.
- Open blocker questions: none.
- Decision: `ready for design approval`.

## 3. Scope

### 3.1 Included residuals

| ID | Residual | Canonical repair owner |
|---|---|---|
| R1 | OAuth refresh / token path maps non-auth errors into `NeedsAuth`, stranding servers without reconnect | `McpOAuthError` classification + `oauth/service.rs` refresh mapping + `mcp_manager` connect error application |
| R2 | `LiveRenderer::reset` drops `previous_kitty_image_ids` without `delete_kitty_images` | `LiveRenderer` image-id lifecycle + `InlineTerminal` call sites |
| R3 | `finish_all` returns `MissingName` and discards pending `End` events for already-started tool slots | `StreamingToolCallAssembler` finish contract + Anthropic (and other) consumers |
| R4 | Dual escape owners: `xml_escape` vs `escape_html_text` / OAuth HTML helpers | Documented dual policy + retire accidental duplicates into one HTML owner or keep two named modules with zero third copy |
| R5 | `BackgroundTaskManager::list_metadata` returns all kinds; only `TaskListTool` filters | TaskList discovery owner: metadata enumeration must exclude delegate/swarm before tool retain |

### 3.2 Explicit non-goals

- No redesign of full OAuth login UX, discovery, or credential store schema.
- No change to HTTP Display-text 401 retirement already completed in F10.
- No Delegate / DelegateGroup / DelegateSwarm card redesign.
- No Bash/Terminal admission or ShellRuntime timeout changes.
- No session JSONL / event schema migration.
- No new third-party dependency, unsafe Neo code, or hosted service.
- No reopening of remediation Tasks 1–18 product decisions.
- No broad HTML sanitizer / XSS product; only escape ownership.
- No removal of BackgroundTaskManager’s ability to mirror delegate status for
  non-discovery purposes, unless required to enforce R5 without dual discovery.

### 3.3 Change Necessity

```text
Change Necessity:
- User-visible need: false re-auth, ghost Kitty images, incomplete tool
  lifecycle, escape dual-owner drift, TaskList discovery leak surface
- No-change / non-code option: documentation alone cannot stop false
  NeedsAuth settlement or ghost images
- Why code change is necessary: each residual is runtime behavior owned by
  existing modules; callers cannot fix classification or assembler finish
- Minimum change boundary: five file-local canonical owners listed in §3.1
- Decision: code-change
```

## 4. Design Choice

Selected approach: **repair each residual at its canonical owner; delete or
forbid dual discovery/escape paths; no compatibility adapters.**

Rejected alternatives:

| Alternative | Why rejected |
|---|---|
| Caller-side guards only (manager callers filter, renderer callers delete) | Sibling paths remain wrong; next caller reintroduces the bug |
| Collapse HTML into `xml_escape` without policy split | Breaks intentional `&#39;` HTML behavior and export snapshots |
| Abort whole multi-tool turn without emitting sibling `End` and document only | Leaves open lifecycle events already pushed mid-stream unless runtime hard-aborts every partial start (harder to prove) |
| Keep `list_metadata` full-kind + rely on TaskList retain forever | Dual discovery residual; future tools re-open F11 |

Architecture invariants:

1. **Auth settlement is typed.** `NeedsAuth` means the user must re-authorize.
   Store, parse, and transport failures are not re-auth.
2. **Terminal image ids are forgotten only after delete is emitted or the
   terminal is known torn down.**
3. **Tool lifecycle events that were started must be finished or the turn must
   hard-abort with no open starts left unaccounted.** This design chooses
   finish-started-then-error for assembler completeness.
4. **Escape policy is named by medium** (model envelope vs HTML), never
   copy-pasted anonymously.
5. **Delegate/swarm discovery has one public owner: ListDelegates.**

## 5. R1 — OAuth Error Classification Contract

### 5.1 Problem

`oauth/service.rs` refresh path:

```text
Err(MissingTokens | NeedsAuth(_)) => propagate
Err(other) => rewrite as NeedsAuth(other.to_string())
```

`Store`, flow/setup, and other non-auth failures become `NeedsAuth`. Downstream
`apply_connect_error` calls `set_needs_auth` and **does not schedule reconnect**.
Transient store/network blips look like “login required.”

F10 already fixed HTTP Display-text `401` matching. R1 is a separate false-
positive on the OAuth service boundary.

### 5.2 Classification table

| `McpOAuthError` / situation | Kind after repair | Manager effect |
|---|---|---|
| Missing tokens; expired access without refresh; missing client registration that requires user login; authorization required / invalid_grant style refresh rejection | `NeedsAuth` | `needs_auth`, no reconnect |
| Token store load/save IO failure | `Store` (non-auth) | failed / reconnectable per existing non-auth connect policy |
| Discovery / HTTP transport failure while refreshing when not an auth challenge | non-auth (`Store` or dedicated transport/protocol mapping already used elsewhere) | reconnectable; not sticky login |
| Parse / registration metadata corruption | non-auth or NeedsAuth only if identity is unusable and user must re-register — prefer non-auth `Store`/`Flow` unless product already treats missing registration as NeedsAuth **and** tests document it | must not blanket-rewrite all `Err` |

### 5.3 Repair

1. Delete the catch-all `Err(err) => Err(McpOAuthError::NeedsAuth(err.to_string()))`
   in the refresh/access-token path.
2. Propagate typed variants unchanged.
3. Ensure `oauth_error_to_http` / `streamable_http_needs_auth` / `apply_connect_error`
   only treat true NeedsAuth as auth settlement.
4. Add regressions:
   - store failure during refresh → **not** `McpErrorKind::NeedsAuth`, manager
     does **not** sticky `needs_auth` without reconnect path.
   - true missing refresh token → still `NeedsAuth`.

### 5.4 Non-goals for R1

- Do not invent a full OAuth error taxonomy redesign beyond existing variants.
- Do not reintroduce Display-text classification.
- Do not change successful refresh behavior.

## 6. R2 — Kitty Image Identity On Reset

### 6.1 Problem

`LiveRenderer::clear_at_origin` and full redraw emit `delete_kitty_images`.
`LiveRenderer::reset` clears `previous_kitty_image_ids` without delete. Call
sites include `InlineTerminal` resume / review transitions
(`inline_terminal.rs` ~221, ~467, ~491, ~535). After reset, ghost placements
cannot be deleted because IDs are forgotten.

### 6.2 Contract

```text
Forget(id) is allowed only if:
  (a) delete_kitty_images({id}) was emitted into a buffer that the caller
      will write to the terminal before next frame, OR
  (b) the terminal surface is being destroyed and no further image IDs
      will be reused on that surface.
```

`reset` as “forget software state only” is forbidden while IDs may still be
live on a continuing terminal.

### 6.3 Repair options (choose one implementation; preferred A)

**A (preferred): `reset` returns delete bytes**

```text
fn reset(&mut self) -> String {
  let deletes = delete_kitty_images(&self.previous_kitty_image_ids);
  // clear software state
  deletes
}
```

Every call site must append/write the returned string into the same
transaction/transition that previously called bare `reset()`.

**B: ban bare reset**

Remove `reset` or make it private; force callers through `clear_at_origin` or a
named `teardown_images` that returns bytes.

This design selects **A** for minimal call-site churn with explicit write
obligation.

### 6.4 Verification

- Unit test: after drawing frames that track fake kitty ids, `reset()` result
  contains delete sequences for those ids and internal set is empty.
- Call-site audit: every `live.reset()` / `next_live.reset()` consumes return
  value into the transition buffer (rg must show no discarded `reset()`).

## 7. R3 — Assembler Finish Completeness

### 7.1 Problem

`StreamingToolCallAssembler::finish_all` iterates slots and on first
`MissingName` returns `Err` **without** flushing `End` events already built for
prior started slots in the same call (and mid-stream those slots may already
have emitted `Start`/`ArgsDelta` via `ingest`). Multi-tool Anthropic messages
with one nameless block can leave valid siblings without `ToolCallEnd`.

Single-tool missing-name tests remain green today; multi-tool is the gap.

### 7.2 Contract

```text
On finish_all:
  For each unfinished slot that has a name and has started (or would start):
    emit End (and Start/Args if required by existing rules).
  If any unfinished slot lacks a name after processing started slots:
    return Err(MissingName { id }) AFTER emitting Ends for started named
    slots, OR return Ok(ends) plus a side channel — this design chooses
    Err after partial Ends.
```

Consumers that currently treat any `Err` as “discard all pending events from
this finish call” must still **retain already-pushed stream events** and must
not execute tools that never received End if the runtime policy is abort-on-
protocol-error. Prefer:

1. Assembler emits Ends for started named tools, then returns Err for nameless.
2. Anthropic `finish_events` pushes successful tool events collected before the
   error when using a split API, **or** finish_all returns
   `Result<Vec<Event>, (Vec<Event>, Error)>` — **rejected** as API noise.

**Selected shape:** change `finish_all` so that:

- First pass: finish all slots that have names (emit Start if needed + End).
- Second pass: if any unfinished slot lacks a name, return
  `Err(MissingName)` **after** the named finishes were appended to `out`.
- Callers that do `let tool_events = finish_all()?; push(tool_events)` must
  change to push partial output on error:

```text
match assembler.finish_all() {
  Ok(events) => push(events),
  Err(err) => {
    // finish_all now returns partial ends via a dedicated method OR
    // Error carries flushed events — prefer method split:
  }
}
```

**Minimal API that avoids Result-tuple noise:**

```text
fn finish_all(&mut self) -> Result<Vec<ToolCallAssemblyEvent>, ToolCallAssemblyError>
```

Implementation order inside `finish_all`:

1. Collect keys.
2. For each slot with `name.is_some()` and not finished → emit Start if needed,
   Args if needed, End; mark finished.
3. For each remaining unfinished slot with `name.is_none()` → return
   `Err(MissingName { id })` **with the understanding that `out` already holds
   sibling ends**. Because Rust `Result` cannot return both, use:

```text
pub struct FinishAllOutcome {
  pub events: Vec<ToolCallAssemblyEvent>,
  pub error: Option<ToolCallAssemblyError>,
}
pub fn finish_all(&mut self) -> FinishAllOutcome
```

Anthropic (and OpenAI-compatible) callers:

```text
let outcome = assembler.finish_all();
push(outcome.events);
if let Some(err) = outcome.error {
  return protocol_error(err);
}
```

This is the **selected** contract. Deprecate silent all-or-nothing Result.

### 7.3 Runtime semantics

After protocol error:

- No tool execution for the nameless tool.
- Tools that received `End` may already be scheduled by the stream consumer;
  Neo’s stream consumer must continue to treat Message-level protocol error as
  turn failure **without re-opening** ended tools. If today the consumer aborts
  the whole turn on first protocol error mid-stream, keep that. R3 only fixes
  **event completeness**, not tool execution policy.

### 7.4 Tests

- Multi-indexed tools: tool0 named+started, tool1 missing name → outcome.events
  contains End for tool0; outcome.error is MissingName for tool1.
- Existing single-tool missing-name Anthropic integration test still fails the
  stream with protocol error and no lifecycle for the nameless tool.

## 8. R4 — Escape Policy Split

### 8.1 Problem

Task 18 introduced `xml_escape::{escape_text, escape_attribute}` for model
envelopes. Session export and OAuth callback still ship private HTML escapers
(`escape_html_text` with `&#39;`). Risk is anonymous third copies and accidental
cross-use, not that HTML and XML must be identical.

### 8.2 Contract

| Medium | Owner | Escapes |
|---|---|---|
| Model / skill / shell message pseudo-XML | `crate::xml_escape` | text: `& < >`; attr: + `"` |
| Browser HTML export / OAuth HTML pages | `crate::html_escape` (new crate-private module) **or** keep one HTML helper in `session/export.rs` re-exported for OAuth | text: `& < > " '` (`&#39;`) |

Rules:

1. Zero third anonymous `fn escape_*` implementations in `neo-agent-core`.
2. Model envelope code must not call HTML helpers.
3. HTML code must not call `xml_escape` unless tests prove equivalence for that
   site (default: do not).
4. Skill instruction **bodies** that inject raw markdown into envelope tags are
   **out of scope** unless a separate security task opens; document only.

### 8.3 Repair

1. Add `html_escape::{escape_text}` (name exact) with current export semantics.
2. Migrate `session/export.rs` and `oauth/callback_server.rs` to it.
3. Delete local duplicates.
4. Module-level docs on both `xml_escape` and `html_escape` stating medium.

## 9. R5 — TaskList Metadata Enumeration Boundary

### 9.1 Problem

`TaskListTool` filters bash/question/workflow after
`background_tasks.list_metadata`. The manager method still returns
delegate/swarm metadata rows. Any future caller reopens dual discovery (F11).

### 9.2 Contract

```text
list_metadata (TaskList discovery owner path):
  returns only Bash | Question | Workflow
  never hydrates .log
  never synthesizes multi_agent runtime entries

ListDelegates:
  sole public discovery for delegate/swarm

BackgroundTaskManager may retain internal records for delegates if required
for TaskOutput/TaskStop adapters, but list_metadata must not expose them.
```

### 9.3 Repair

1. Move kind filtering into `list_metadata` (or rename current method to
   `list_task_list_metadata` and make it the only public discovery list).
2. If tests need “all kinds including delegate projections,” use a
   `#[cfg(test)]` or crate-private `list_metadata_all_kinds_for_test` — not
   production public API.
3. Remove redundant retain in `TaskListTool` **or** keep as defense-in-depth
   with comment that manager already filters (prefer single filter in manager
   + one assert test).
4. Retirement scan: production callers of list_metadata must not expect
   delegate kinds.

## 10. Compatibility Boundary

- No `AgentEvent` / JSONL schema change.
- No public MCP wire protocol change.
- MessageDelegate / live-steer fixes in `c6b748b8` remain; do not regress.
- External user data untouched.
- Windows / Linux / macOS: R2 is terminal-protocol (Kitty); non-Kitty paths
  must no-op safely. R1/R3/R4/R5 are OS-neutral.

## 11. Retirement Track

| Residual | Old path | Action |
|---|---|---|
| R1 | catch-all `NeedsAuth(err.to_string())` | delete |
| R2 | bare `reset()` forgetting ids | delete bare form; only reset-that-returns-deletes |
| R3 | all-or-nothing `finish_all -> Result` | replace with `FinishAllOutcome` (or equivalent); update all callers |
| R4 | anonymous HTML escape fns | delete after `html_escape` migration |
| R5 | full-kind `list_metadata` as TaskList discovery | filter at owner; no production full-kind public list |

## 12. Acceptance Criteria

1. Store-failure refresh cannot put managed MCP server into sticky `needs_auth`
   without reconnect eligibility; true re-auth cases still do.
2. After `LiveRenderer` reset following tracked kitty ids, delete sequences for
   those ids are produced and call sites write them.
3. Multi-tool finish with one nameless slot emits End for started named tools
   and surfaces MissingName; single-tool missing-name still protocol-errors.
4. `rg` finds no third anonymous HTML/XML escape implementation in
   `neo-agent-core/src` outside `xml_escape` and `html_escape`.
5. `list_metadata` never returns delegate/swarm kinds; ListDelegates still
   reports background delegates; TaskList tests still exclude them.
6. Exact package/target/filter tests green; CI green after implementation.
7. No Delegate-card or ShellRuntime admission edits.

## 13. Risks

| Risk | Mitigation |
|---|---|
| Over-narrowing NeedsAuth breaks real login prompts | Table-driven tests for missing refresh vs store error |
| Reset return value ignored at a call site | rg gate + compile if `#[must_use]` on reset return |
| FinishAllOutcome breaks many providers | Update all `finish_all` callers in one task; compile is the gate |
| HTML escape snapshot drift in export tests | Run export-related exact tests |
| Filtering list_metadata breaks an internal debug caller | rg production callers; only TaskList + tests |

## 14. ADR Signal

If R1 classification table or R3 FinishAllOutcome is treated as a durable
cross-module contract after approval, record a short ADR at completion:
“MCP NeedsAuth means re-authorization only” and/or “tool assembler finish
emits Ends for started tools before MissingName.” Not required before
implementation if the design is approved as the decision record for this
workstream.

## 15. Relationship To Parent Remediation

This workstream **extends** crates-audit remediation; it does not reopen F1–F16
product decisions. Overlaps:

- R1 completes the spirit of F10 (typed auth only).
- R3 hardens F3 multi-tool finish.
- R5 hardens F11 enumeration boundary.
- R2/R4 are residual quality from C1/C2 neighborhood, not new product features.
