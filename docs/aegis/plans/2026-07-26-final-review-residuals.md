# Neo 2026-07-26 Final-Review Residuals Implementation Plan

> Executor note: implement
> `docs/aegis/specs/2026-07-26-final-review-residuals-design.md` after user
> approval. Do not reopen crates-audit Tasks 1–18 product decisions. Do not
> absorb unrelated dirty worktree files (including local `.gitignore`).

## Goal

Close residuals R1–R5 (OAuth NeedsAuth false settlement, Kitty reset deletes,
assembler finish completeness, HTML/XML escape ownership, TaskList metadata
kind boundary) at their canonical owners, with exact verification and one
commit per logical task.

## Architecture

- R1: OAuth service typed error propagation; manager auth settlement unchanged
  except false positives disappear.
- R2: `LiveRenderer::reset` returns delete bytes; InlineTerminal writes them.
- R3: `StreamingToolCallAssembler::finish_all` → `FinishAllOutcome`; providers
  push events then surface optional error.
- R4: new crate-private `html_escape`; delete anonymous HTML helpers.
- R5: `list_metadata` filters to bash/question/workflow at the manager.

## Baseline / Authority Refs

- Spec: `docs/aegis/specs/2026-07-26-final-review-residuals-design.md`
- Parent: `docs/aegis/specs/2026-07-25-crates-audit-remediation-design.md` (F3/F10/F11/C2)
- `AGENTS.md`
- Already-landed follow-up (do not regress): `c6b748b8`

```text
BaselineUsageDraft:
- Required baseline refs: AGENTS.md; 2026-07-26 residuals design; parent remediation design F3/F10/F11
- Delivered context refs: final review reports; user expansion of five deferred items
- Acknowledged before plan refs: same
- Cited in plan refs: same
- Missing refs: none
- Decision: continue
```

```text
Requirement Ready Check:
- Requirement source refs: design §2
- Goals and scope refs: design §1 §3
- Acceptance / verification criteria refs: design §12
- Open blocker questions: none (pending user approval of design)
- Decision: ready after design approval
```

```text
Change Necessity:
- User-visible need: false re-auth, ghost images, incomplete tool ends, escape dual-owner, discovery leak
- No-change option: insufficient
- Why code change is necessary: runtime owners only
- Minimum change boundary: files listed per task
- Decision: code-change
```

## Compatibility Boundary

- No AgentEvent / session JSONL schema change.
- No Delegate-card presentation change.
- No ShellRuntime admission / unbounded timeout change.
- No new third-party crates; no unsafe.
- No `.references/` edits.
- `#[must_use]` on reset return preferred.

## TDD Route

```text
TDD Route:
- Mode: off
- Decision: skipped
- Strict authority: not applicable
- Test posture: post-change regression (one valuable test per residual)
- Reason: project AGENTS.md + remediation posture; no explicit strict TDD request
- Verification: exact package/target/filter commands only
```

## Subagent Execution Model

Root uses ≥3 implementation subagents. Initial disjoint wave:

- Subagent A: Task 1 (R1 OAuth)
- Subagent B: Task 2 (R2 Kitty)
- Subagent C: Task 4 (R4 html_escape) — file-disjoint from A/B

Then:

- Task 3 (R3 assembler) serial after any parallel wave (touches neo-ai shared)
- Task 5 (R5 list_metadata) after Task 1 if desired, else parallel with Task 3
  (different crates: core tools vs neo-ai)

Suggested:

| Wave | Tasks |
|---|---|
| A | 1, 2, 4 parallel |
| B | 3 then 5 (or 3 ∥ 5: neo-ai vs neo-agent-core tools) |

Task 3 and Task 5 are file-disjoint → may run parallel in wave B.

Subagents: no git mutation. Root reviews, exact verifies, commits.

## Files Map

| Residual | Primary files |
|---|---|
| R1 | `crates/neo-agent-core/src/tools/mcp/oauth/service.rs`, `http.rs` tests, `mcp_manager.rs` tests |
| R2 | `crates/neo-tui/src/screen_output/live_renderer.rs`, `inline_terminal.rs` |
| R3 | `crates/neo-ai/src/tool_assembly.rs`, `providers/anthropic.rs`, other `finish_all` callers, tests |
| R4 | `crates/neo-agent-core/src/html_escape.rs` (new), `session/export.rs`, `oauth/callback_server.rs`, `lib.rs` |
| R5 | `crates/neo-agent-core/src/tools/background_tasks.rs`, multi_agent_background / unit tests |

---

## Task 1: Typed OAuth NeedsAuth Only

**Residual:** R1  
**Commit:** `fix(core): stop rewriting non-auth OAuth failures as NeedsAuth`

**Files:**

- `crates/neo-agent-core/src/tools/mcp/oauth/service.rs`
- Local unit tests in oauth service and/or `mcp/http.rs` / `mcp_manager.rs`

**Steps:**

1. Locate every `Err(err) => Err(McpOAuthError::NeedsAuth(err.to_string()))`
   catch-all in the access-token / refresh path (at minimum the match in
   `service.rs` around the `refresh` call after freshness check).
2. Propagate typed errors unchanged. Do not convert `Store` into `NeedsAuth`.
3. Keep true NeedsAuth sources: missing tokens, expired without refresh,
   missing client registration when product already uses NeedsAuth for that
   case, authorization/refresh invalid_grant style failures already mapped to
   NeedsAuth.
4. Confirm `oauth_error_to_http` + `streamable_http_needs_auth` only treat
   NeedsAuth as auth; no Display matching.
5. Add regression:
   - inject/store failure (or construct `McpOAuthError::Store(...)`) through
     the refresh path → result is **not** NeedsAuth at McpErrorKind boundary.
   - missing refresh token still NeedsAuth.
6. Manager: store-failure connect/refresh must not call `set_needs_auth` sticky
   without reconnect; reuse existing non-auth failure handling.

```bash
rtk cargo nextest run -p neo-agent-core --lib store_failure_during_oauth_refresh_is_not_needs_auth
rtk cargo nextest run -p neo-agent-core --lib missing_refresh_token_still_needs_auth
# or equivalent exact names chosen in the task; one package --lib one filter each
rtk rg -n 'NeedsAuth\(err\.to_string\(\)\)|NeedsAuth\(format!' crates/neo-agent-core/src/tools/mcp/oauth/service.rs
```

Manually confirm remaining NeedsAuth constructions are true re-auth cases.

**Stop if:** true re-auth can no longer surface NeedsAuth, or repair needs a
new public error enum redesign beyond existing variants.

---

## Task 2: Kitty Delete On LiveRenderer Reset

**Residual:** R2  
**Commit:** `fix(tui): emit kitty deletes when live renderer resets`

**Files:**

- `crates/neo-tui/src/screen_output/live_renderer.rs`
- `crates/neo-tui/src/screen_output/inline_terminal.rs`
- unit tests co-located or existing image/live tests

**Steps:**

1. Change `LiveRenderer::reset` to `#[must_use] pub(crate) fn reset(&mut self) -> String`.
2. Implementation: `let out = delete_kitty_images(&self.previous_kitty_image_ids);`
   then clear `previous_lines`, cursor, ids, `full_redraw_pending`; return `out`.
3. Update every call site in `inline_terminal.rs` (and any other rg hit) to
   append the returned string into the same transition/transaction buffer that
   already receives `clear_at_origin` bytes.
4. Add unit test: set/track ids (via public/crate test seam or by rendering
   lines containing kitty ids if that is how `collect_kitty_image_ids` works),
   call `reset()`, assert return contains delete markers for those ids and
   internal id set empty.
5. `rtk rg -n '\.reset\(\)' crates/neo-tui/src` — every live-renderer reset
   must use the return value (no bare `self.live.reset();` without write).

```bash
rtk cargo test --package neo-tui --lib -- live_renderer::tests::reset_emits_kitty_deletes_for_previous_ids --exact --nocapture
rtk rg -n 'live\.reset\(\)|next_live\.reset\(\)|\.live\.reset\(\)' crates/neo-tui/src
```

**Stop if:** a call site cannot write delete bytes before terminal teardown
and the surface is still live.

---

## Task 3: Assembler FinishAllOutcome

**Residual:** R3  
**Commit:** `fix(ai): finish started tool calls before MissingName on finish_all`

**Files:**

- `crates/neo-ai/src/tool_assembly.rs`
- All callers of `finish_all` (rg): `providers/anthropic.rs`, openai-compatible
  paths, tests
- `crates/neo-ai/tests/real_provider_adapters.rs` as needed

**Steps:**

1. Introduce:

```rust
pub struct FinishAllOutcome {
    pub events: Vec<ToolCallAssemblyEvent>,
    pub error: Option<ToolCallAssemblyError>,
}

pub fn finish_all(&mut self) -> FinishAllOutcome { ... }
```

2. Algorithm:
   - For each unfinished slot **with** `name`: emit Start if needed, ArgsDelta
     if needed, End; mark finished (preserve existing start-if-not-started
     behavior for named slots).
   - Then if any unfinished slot lacks name: set
     `error = Some(MissingName { id })` (first such id is enough).
   - Return `{ events, error }` — events always include Ends for named started
     tools.
3. Update every caller:

```rust
let outcome = assembler.finish_all();
// push outcome.events into stream
if let Some(err) = outcome.error {
    return protocol / Err(...);
}
```

4. Tests:
   - Unit: two indexed slots; slot0 named+partial args; slot1 no name → events
     contain End for slot0; error is MissingName.
   - Existing Anthropic single-tool missing-name integration still protocol-
     errors without emitting lifecycle for that tool.
5. Retirement: no production `finish_all` returning bare `Result` without
   partial events.

```bash
rtk cargo nextest run -p neo-ai --lib finish_all_emits_end_for_started_named_tools_before_missing_name
rtk cargo nextest run -p neo-ai --test real_provider_adapters anthropic_missing_tool_name_is_protocol_error_without_tool_lifecycle_events
rtk rg -n 'finish_all\(' crates/neo-ai
```

**Stop if:** a caller requires all-or-nothing Result for external API stability
(should not — crate-internal assembler).

---

## Task 4: Named HTML Escape Owner

**Residual:** R4  
**Commit:** `refactor(core): centralize HTML escape beside xml_escape`

**Files:**

- create `crates/neo-agent-core/src/html_escape.rs`
- `crates/neo-agent-core/src/lib.rs` (`mod html_escape;`)
- `crates/neo-agent-core/src/session/export.rs`
- `crates/neo-agent-core/src/oauth/callback_server.rs` (or actual path of HTML
  escape)
- `crates/neo-agent-core/src/xml_escape.rs` (module doc only)

**Steps:**

1. Add `html_escape::escape_text` matching current export semantics
   (`& < > " '` → entities including `&#39;`).
2. Module docs: “browser HTML only; model envelopes use `xml_escape`.”
3. Document `xml_escape` as “model/skill/shell pseudo-XML only.”
4. Migrate export + OAuth callback helpers; delete local `fn escape_html_*`.
5. Do **not** migrate skill body raw injection; out of scope per design.
6. Scans:

```bash
rtk rg -n 'fn escape_html|&#39;|escape_html_text' crates/neo-agent-core/src
rtk cargo nextest run -p neo-agent-core --lib export
# use the narrowest existing export/html test filter available; if none,
# add one assertion that apostrophe becomes &#39;
```

**Stop if:** OAuth HTML is not plain text escape but a different encoder
(verify before forcing `html_escape`).

---

## Task 5: list_metadata Kind Boundary

**Residual:** R5  
**Commit:** `refactor(core): exclude delegates from task-list metadata enumeration`

**Files:**

- `crates/neo-agent-core/src/tools/background_tasks.rs`
- tests in same module and/or `tests/multi_agent_background.rs`

**Steps:**

1. In `list_metadata`, after building snapshots (or inside snapshot push),
   retain only `Bash | Question | Workflow`.
2. Never hydrate `.log` (already true — keep).
3. Optionally keep a crate-private / `#[cfg(test)]` full-kind enumerator for
   tests that assert manager still tracks delegate projections.
4. TaskListTool may keep retain as defense-in-depth with a one-line comment,
   or drop duplicate filter after manager filter + test.
5. Tests:
   - TaskList still excludes delegates (existing).
   - New/adjusted: `list_metadata` alone returns no delegate kinds even when
     manager has delegate records.
   - ListDelegates still reports background delegate.

```bash
rtk cargo nextest run -p neo-agent-core --lib list_metadata_excludes_delegate_and_swarm_kinds
rtk cargo nextest run -p neo-agent-core --lib task_list_uses_metadata_only_enumeration_and_excludes_delegates
rtk cargo nextest run -p neo-agent-core --test multi_agent_background list_delegates_reports_background_delegate
rtk rg -n 'list_metadata\(' crates/neo-agent-core
```

**Stop if:** a production non-TaskList caller requires full-kind metadata and
is not test-only — escalate rather than re-open dual discovery.

---

## Per-Task Gate

1. `git status --short` — ignore unrelated dirt.
2. Exact tests + scans for the task.
3. File-scoped rustfmt + `git diff --check` on task paths.
4. Root stages only task files; one conventional commit.
5. No push without authorization.

## Final Integration Gate

1. Re-run all R1–R5 exact tests and retirement scans at HEAD.
2. Confirm no Delegate-card files and no ShellRuntime admission edits.
3. `cargo fmt --all --check` as formatting evidence only.
4. CI green after push (when authorized).
5. Completion matrix: residual → commit SHA → commands.

## Execution Readiness View

```text
Execution Readiness View:
- Intent Lock: close five final-review residuals only
- Scope Fence: R1–R5 owners; no remediation 1–18 reopen; no OAuth UX redesign
- Baseline Lock: 2026-07-26 residuals design + AGENTS.md + parent F3/F10/F11
- Approved Behavior: design §2.2 decisions
- Owner / Contract Constraints: design §4–§9
- Compatibility Boundary: no schema/card/shell admission change
- Retirement Boundary: design §11
- Task Batches: Wave A {1,2,4}; Wave B {3∥5}
- Test Obligations: exact filters per task
- Review Gates: root review+commit after each task
- Drift / Rewind Rules: stop on contract conflict; no destructive git
- Evidence Required Before Completion: final integration gate + CI if pushed
- Advisory Boundary: method-pack guidance only
```

## Risks

| Risk | Handling |
|---|---|
| NeedsAuth over-narrowing | dual tests true vs false auth |
| reset return dropped | must_use + rg |
| FinishAllOutcome call-site miss | rg finish_all + compile |
| export snapshot apostrophe | exact export test |
| list_metadata consumer | rg + stop/escalate |

## Plan Pressure Test

```text
Plan Pressure Test:
- Owner / contract / retirement: each residual has one owner and delete list
- Architecture integrity: no new dual owner; FinishAllOutcome is one assembler API
- Verification scope: exact filters named
- Task executability: file lists + steps + commands
- Pressure result: proceed after design approval
```

---

## Self-Review

1. Spec coverage: R1→T1, R2→T2, R3→T3, R4→T4, R5→T5.
2. No TBD placeholders for control flow.
3. Compatibility and non-goals carried.
4. Change necessity: code-change justified.
5. Retirement per residual.
6. TDD skipped recorded.
7. Subagent file leases disjoint for wave A.

---

Plan complete once design is approved. Execution options after approval:

1. **Subagent-driven (recommended)** — one subagent per task, root review/commit  
2. **Inline** — execute in-session with checkpoints  

Which approach after you approve the design?
