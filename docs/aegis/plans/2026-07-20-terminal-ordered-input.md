# Terminal Ordered Input Implementation Plan

> **For agentic workers:** Execute with `aegis:executing-plans` or
> `aegis:subagent-driven-development`. Preserve unrelated dirty work, use the
> existing shared worktree unless the user separately authorizes `git worktree
> add`, and run only the exact verification named below.

**Goal:** Replace Terminal write's ambiguous scalar string with one canonical,
non-empty ordered sequence that can carry readable UTF-8 text and exact PTY
control bytes in a single tool call.

**Architecture:** Keep `TerminalTool` as the sole validation and byte-assembly
owner. It converts typed input parts to one `Vec<u8>`, preserving order and
normalizing newlines only inside text parts, then calls the unchanged
`GuardianClient::write_terminal(&[u8])` path once. Guardian framing, PTY
transport, session ownership, output collection, and platform backends remain
raw byte transports.

**Tech Stack:** Rust 2024, Serde, Schemars, Tokio, existing guardian IPC and
real-PTY integration harnesses, Markdown.

**Baseline/Authority Refs:**

- `docs/aegis/specs/2026-07-20-terminal-ordered-input-design.md`
- `docs/aegis/specs/2026-07-19-terminal-bounded-yield-design.md`
- `docs/aegis/plans/2026-07-19-terminal-bounded-yield.md`
- `AGENTS.md`

**Compatibility Boundary:** This is a hard public tool-contract replacement.
`mode=write` accepts only a non-empty ordered array of exactly-one-of `text` or
`control` parts. Scalar string input is rejected; there is no untagged enum,
alias, versioned field, escape parser, Base64 carrier, `interrupt`, or `signal`
operation. Admission waiting, command lifetime, bounded yield, shared output
offsets, cancellation cleanup, Guardian/Bash/runtime/TUI behavior, raw output,
and platform PTY ownership remain unchanged.

**TDD Route:**

- Mode: off
- Decision: skipped
- Strict authority: not applicable
- Test posture: post-change regression
- Reason: strict TDD was not requested; the defect and approved contract are
  already established, so focused contract and real-PTY regressions are the
  proportional evidence.
- Verification: exact commands in Tasks 1-4; no broad workspace test.

**Verification:** Prove exact byte assembly and invalid-input rejection in the
core unit target; prove schema retirement in the schema integration target;
prove one-call PTY control behavior and bounded-yield regression in the
guardian integration target; run the relevant exact tests on macOS, Fedora,
and Windows 11 ConPTY.

---

## Scope Check

**Aegis Visibility:** Planning is required because the public JSON schema is
being replaced, the old owner must be retired rather than retained as a
fallback, and completion requires platform-specific evidence without changing
the raw PTY transport boundary.

**Plan Basis:** The approved design records a requirements-level Design Defect:
JSON string escapes are not a reliable AI-facing representation of terminal
control input. The observed Fedora SSH failure delivered six printable bytes,
while the existing Unix real-PTY test proves actual byte `0x03` already works.

**BaselineUsageDraft:**

- Required baseline refs: ordered-input design, bounded-yield design/plan,
  repository `AGENTS.md`.
- Delivered context refs: current session evidence and approved design.
- Acknowledged before plan refs: all required refs.
- Cited in plan refs: all required refs.
- Missing refs: none.
- Decision: continue.

**Requirement Ready Check:**

- Requirement source refs: approved ordered-input design and user confirmation.
- Goals and scope refs: design `Decision`, `Product Contract`, and `Scope and
  Governance` sections.
- User / scenario refs: mixed ordinary text plus PTY control input, including
  nested interactive SSH, in one model tool call.
- Requirement item refs: typed ordered parts, exact control range, text newline
  normalization, hard scalar retirement, unchanged raw transport.
- Acceptance / verification criteria refs: design `Verification` section.
- Open blocker questions: none.
- Decision: ready.

**Change Necessity:**

- User-visible need: express exact ordered text and control bytes without a
  second tool call or ambiguous escaping.
- No-change / non-code option: documentation alone still permits the model and
  JSON producer to double-escape printable text.
- Why code change is necessary: the generated schema and deserializer must make
  the exact representation mechanically unambiguous before Guardian I/O.
- Minimum change boundary: `tools/terminal.rs`, existing Terminal call-site
  tests, schema regression, and synchronized zh/en tool docs.
- Decision: code-change.

**Existence Check:**

- Proposed new surface: typed parts within the existing `input` field.
- Existing owner / reuse candidate: `TerminalInput` and `write_terminal` in
  `TerminalTool`.
- Why existing surface is insufficient: `Option<String>` cannot distinguish
  literal escape-like text from requested control bytes.
- Creation proof: one local enum/struct and one encoder are the minimum typed
  trust-boundary representation; no module or runtime owner is added.
- Entropy / retirement impact: deletes the scalar path and avoids all
  compatibility carriers.
- Decision: reuse-existing.

**Architecture Integrity Lens:** Exact keyboard bytes belong at the PTY input
contract, not in process signaling or Guardian protocol interpretation.
`TerminalTool` remains canonical; Guardian remains a raw `Vec<u8>` transport.
There is no responsibility overlap or higher-level simplification beyond the
hard schema replacement. Verdict: edit in place.

**Ripple Signal Triage:** The input type is private to `terminal.rs`, but JSON
call sites in core/runtime and real-PTY integration tests must migrate. Tool
schema and docs are public consumers. Guardian, Bash, TUI, and persisted
sessions do not consume the typed Rust input and must not change.

**Complexity Budget:**

- Artifact class: Source Complexity and Test Complexity.
- Target files / artifacts: `terminal.rs` (~740 lines), existing guardian
  integration target, small schema target, two reference docs.
- Current pressure: `terminal.rs` is about 740 lines and approaching, but still
  below, the 800-line soft threshold; it is cohesive and already owns all
  Terminal input validation.
- Projected post-change pressure: one small local type and encoder, with the old
  normalization helper folded into that encoder; no new responsibility.
- Budget result: within-budget.
- Planned governance: edit in place, reuse existing tests, add no helper module
  or generic byte-input abstraction.

**Plan-Time Complexity Check:**

- Target files: `terminal.rs` and existing focused test owners.
- Existing size / shape signals: cohesive Terminal owner; large integration
  file already contains the unique cross-platform PTY harness.
- Owner fit: exact input validation and assembly are already Terminal concerns.
- Add-in-place risk: low if the encoder stays local and tests are not duplicated.
- Better file boundary: none; extraction would create a one-consumer owner.
- Recommendation: edit-in-place.

**Plan Pressure Test:**

- Owner / contract / retirement: one canonical array contract; scalar deleted.
- Architecture integrity / higher-level path: raw Guardian/PTTY path reused.
- Verification scope: local bytes/schema plus supported-host real PTY.
- Task executability: file paths, JSON shapes, negative cases, and exact tests
  are named below.
- Pressure result: proceed.

---

## File Map

**Modify**

- `crates/neo-agent-core/src/tools/terminal.rs`
- `crates/neo-agent-core/tests/tool_schema_descriptions.rs`
- `crates/neo-agent-core/tests/runtime_turn.rs` only for its existing Terminal
  write fixture.
- `crates/neo-agent/tests/tool_terminal_guardian.rs`
- `docs/en/reference/tools.md`
- `docs/zh/reference/tools.md`

**Create**

- `docs/aegis/work/2026-07-20-terminal-ordered-input/` execution records when
  plan execution starts.

**Do not modify**

- `crates/neo-agent-core/src/tools/shell_guard/**`
- Bash, admission scheduler, runtime dispatch, cancellation, or TUI owners.
- Unrelated dirty files, including the concurrent TUI tool-card work and its
  existing `docs/aegis/INDEX.md` entries.

---

### Task 1: Replace Scalar Input With One Ordered Encoder

**Files:** Modify `crates/neo-agent-core/src/tools/terminal.rs`.

**Why:** Make exact control bytes unambiguous while preserving readable text and
one tool call for mixed input.

**Change Necessity:** The current `Option<String>` deserializer is the root
cause. The minimum repair is a typed array plus byte encoder at the existing
Terminal trust boundary.

**Impact/Compatibility:** `mode=write` becomes intentionally incompatible with
the old scalar form. Other modes and all output/lifecycle contracts are stable.

**Repair Track:** Add a typed part, validate it, assemble one ordered byte
buffer, and call Guardian once.

**Retirement Track:** Remove `Option<String>` and
`normalize_terminal_input_newlines`; retain no scalar branch or escape parser.

- [ ] **Step 1: Define the canonical schema types**

  Replace the scalar field with:

  ```rust
  #[schemars(
      description = "Ordered terminal input parts. Required and non-empty for write. Each part is exactly one text string or one control byte. Text newlines become carriage returns; control accepts 0..=31 or 127."
  )]
  input: Option<Vec<TerminalInputPart>>,
  ```

  Use a Serde/Schemars representation that generates array items containing
  exactly one of `text` or `control`, denies unknown fields, and does not emit a
  string alternative. Keep the type private to `terminal.rs`.

- [ ] **Step 2: Validate field scope before Guardian I/O**

  Reject `input` on `start`, `read`, `resize`, and `stop`. For `write`, reject a
  missing or empty array. Reject a part with neither/both fields and reject any
  `control` value outside `0..=31` except `127`, returning
  `ToolError::InvalidInput` whose message names `input` or `control`.

- [ ] **Step 3: Assemble exact bytes in one local encoder**

  Add one helper with this effective signature:

  ```rust
  fn encode_terminal_input(
      tool: &str,
      parts: &[TerminalInputPart],
  ) -> Result<Vec<u8>, ToolError>
  ```

  Iterate once in array order. For text parts, append UTF-8 bytes while mapping
  LF and CRLF to CR and preserving all other characters literally. For control
  parts, append exactly one validated byte. Pass the completed slice to the
  existing `session.client.write_terminal(&bytes).await?` once.

- [ ] **Step 4: Replace the existing normalization test with one contract test**

  Add or rename one test to prove that parts equivalent to:

  ```json
  [
    {"text":"alpha\n"},
    {"control":3},
    {"text":"\\u0003\r\nomega"},
    {"control":27},
    {"control":127}
  ]
  ```

  encode exactly to
  `b"alpha\r\x03\\u0003\romega\x1b\x7f"`. The literal six-character
  `\\u0003` text must remain literal.

- [ ] **Step 5: Extend the existing mode-scoped validation test**

  Through `TerminalTool::execute`, assert empty input, invalid control `32`,
  scalar string input, and `input` on `read` fail as invalid input before a
  terminal session lookup or Guardian request.

- [ ] **Step 6: Run exact core verification**

  ```bash
  rtk cargo test --package neo-agent-core --lib -- tools::terminal::tests::terminal_input_encodes_ordered_text_and_control_bytes --exact --nocapture --include-ignored
  rtk cargo test --package neo-agent-core --lib -- tools::terminal::tests::terminal_input_is_non_empty_validated_and_write_scoped --exact --nocapture --include-ignored
  ```

  Expected: each exact test passes.

---

### Task 2: Retire Scalar Consumers and Prove the Generated Schema

**Files:** Modify `crates/neo-agent-core/tests/tool_schema_descriptions.rs`,
`crates/neo-agent-core/tests/runtime_turn.rs`, and existing Terminal JSON calls
in `crates/neo-agent/tests/tool_terminal_guardian.rs`.

**Why:** A hard replacement is incomplete if tests/examples still teach or
accept the ambiguous scalar input.

**Change Necessity:** These are current consumers of the public JSON contract;
leaving them scalar would either fail or pressure implementation toward a
compatibility branch.

**Impact/Compatibility:** Test fixtures migrate mechanically to
`"input":[{"text":"..."}]`; production owners outside Terminal do not change.

**Repair Track:** Update existing consumers and add one schema assertion that
checks the public contract rather than Serde internals.

**Retirement Track:** Search maintained Rust/docs for scalar Terminal write
examples and eliminate them; the negative scalar execution assertion remains
as retirement evidence.

- [ ] **Step 1: Migrate existing JSON write fixtures**

  Replace each Terminal write scalar with one text part, except control tests,
  which use `{"control":N}`. Preserve the original yield values, handles,
  payload text, and assertions.

- [ ] **Step 2: Add one public schema regression**

  Add `terminal_write_input_schema_is_ordered_parts_only` to
  `tool_schema_descriptions.rs`. Resolve the `input` property and assert it is
  an array whose item schema exposes `text` and `control`; serialize or walk the
  schema and assert no string alternative exists for `input`. Also assert the
  description names ordered parts and the `0..=31`/`127` control range.

- [ ] **Step 3: Run exact schema and runtime-fixture verification**

  ```bash
  rtk cargo test --package neo-agent-core --test tool_schema_descriptions -- terminal_write_input_schema_is_ordered_parts_only --exact --nocapture --include-ignored
  rtk cargo test --package neo-agent-core --test runtime_turn -- terminal_tool_emits_structured_runtime_events --exact --nocapture --include-ignored
  ```

  If the runtime test's exact qualified name differs, obtain it with
  `rtk cargo test --package neo-agent-core --test runtime_turn -- --list` and
  rerun exactly that one test; do not substitute a substring suite.

---

### Task 3: Prove One-Call Raw PTY Behavior Without Signal Claims

**Files:** Modify `crates/neo-agent/tests/tool_terminal_guardian.rs`.

**Why:** Unit bytes are necessary but do not prove that the public tool sends
the mixed sequence through the real PTY in one write or preserves the existing
post-write observation contract.

**Change Necessity:** The existing cross-platform harness is the only current
owner of real Guardian/PTTY lifecycle evidence.

**Impact/Compatibility:** Reuse existing platform helpers and hold protocols.
Do not add shell assumptions outside existing `cfg(unix)`/`cfg(windows)` paths.

**Repair Track:** Convert the Unix Ctrl+C test to `{"control":3}` and add only
one mixed text/control test if it uniquely proves ordering in one write.

**Retirement Track:** Delete any test construction of actual control characters
inside scalar JSON strings. Windows continues to prove byte delivery/session
usability, not portable SIGINT generation.

- [ ] **Step 1: Update the existing Ctrl+C regression**

  On Unix, send `"input":[{"control":3}]` after starting the foreground sleep,
  then prove the command is interrupted and the same session accepts a later
  command. Preserve its current exact test name when practical.

- [ ] **Step 2: Add one unique mixed ordering regression**

  In one Terminal write call, send text followed by `{"control":4}` to a
  foreground reader that completes on EOF, then assert the received text and
  the post-write output/session state. Guard platform-specific program setup
  with existing helpers; on Windows use the existing PowerShell hold protocol
  to prove ordered raw byte handling/session usability without asserting signal
  semantics. If the current Ctrl+C/lifecycle tests already prove the same
  one-call ordering on a platform, do not add a redundant second test there.

- [ ] **Step 3: Re-run bounded-yield and cap regressions**

  ```bash
  rtk cargo test --package neo-agent --test tool_terminal_guardian -- terminal_start_write_and_read_share_incremental_bounded_output --exact --nocapture --include-ignored
  rtk cargo test --package neo-agent --test tool_terminal_guardian -- terminal_ctrl_c_interrupts_command_and_keeps_session_usable --exact --nocapture --include-ignored
  rtk cargo test --package neo-agent --test tool_terminal_guardian -- terminal_write_sends_ordered_text_and_control_in_one_call --exact --nocapture --include-ignored
  rtk cargo test --package neo-agent --test tool_terminal_guardian -- terminal_read_details_do_not_leak_output_past_max_output_bytes --exact --nocapture --include-ignored
  ```

  Expected: each applicable exact test passes. A `cfg`-excluded test is reported
  as uncovered on that host, not relabeled as a pass.

---

### Task 4: Synchronize the AI-Facing Contract

**Files:** Modify `docs/en/reference/tools.md` and
`docs/zh/reference/tools.md`.

**Why:** Models and users must see the canonical one-call representation and
the receiver-controlled PTY semantics.

**Change Necessity:** The current docs explicitly instruct models to rely on
decoded scalar control characters, which is the retired design.

**Impact/Compatibility:** Documentation only describes the new schema; no raw
output filtering or signal promise is introduced.

**Repair Track:** Show a mixed text/control array and list common control byte
values.

**Retirement Track:** Remove all claims that `input` is a scalar string or that
JSON escape decoding is the supported control-key mechanism.

- [ ] **Step 1: Update English and Chinese references in lockstep**

  Document:

  ```json
  "input": [
    {"text": "command text"},
    {"control": 3}
  ]
  ```

  State that text LF/CRLF becomes CR, control accepts `0..=31` or `127`, array
  order is preserved in one write, literal `\\u0003` inside a text part remains
  literal, and interactive remote control should use `ssh -tt` when remote PTY
  allocation is uncertain. Keep the Windows ConPTY and portable-signal caveat.

- [ ] **Step 2: Run documentation/retirement checks**

  ```bash
  rg -n 'input.*(U\+0003|decode|解码|Option<String>)|"input"\s*:\s*"' crates/neo-agent-core/src/tools/terminal.rs crates/neo-agent-core/tests crates/neo-agent/tests/tool_terminal_guardian.rs docs/en/reference/tools.md docs/zh/reference/tools.md
  git diff --check
  ```

  Expected: only intentional scalar-negative-test evidence remains; no scalar
  production schema, fixture, or documentation example remains.

---

### Task 5: Verify Supported Hosts and Commit the Logical Change

**Files:** No new production files. Update execution evidence records only.

**Why:** Exact PTY keyboard semantics depend on the receiving terminal stack,
so byte-contract evidence and supported-host lifecycle evidence must be kept
distinct.

**Impact/Compatibility:** Verification must not change production behavior or
weaken platform assertions merely to pass a host.

- [ ] **Step 1: Verify macOS locally**

  Run the exact Task 1-4 tests. Also run:

  ```bash
  rustfmt --check --edition 2024 crates/neo-agent-core/src/tools/terminal.rs crates/neo-agent-core/tests/tool_schema_descriptions.rs crates/neo-agent-core/tests/runtime_turn.rs crates/neo-agent/tests/tool_terminal_guardian.rs
  git diff --check
  ```

- [ ] **Step 2: Verify Fedora over the configured host**

  In `/root/neo`, pull only after the change is available there, then run the
  exact core encoder/validation tests and the applicable Unix real-PTY tests
  from Task 3. Use the existing Rust 1.97.1 toolchain. Do not use `ssh -t` for
  non-interactive Cargo commands; the test itself creates its PTY.

- [ ] **Step 3: Verify Windows 11 ConPTY**

  In `C:/Users/10592/Desktop/neo`, use PowerShell plus `vcvars64.bat` and
  `stable-x86_64-pc-windows-msvc` 1.97.1. Run the exact core/schema tests and
  applicable real-PTY tests. Report Ctrl+C as byte/session evidence only; do
  not claim portable Windows signal semantics.

- [ ] **Step 4: Commit only ordered-input files**

  Inspect `git diff` and `git status`, stage only the files listed in this plan
  plus this workstream's Aegis records, and commit:

  ```bash
  git commit -m "fix(terminal): add ordered control input"
  ```

  Do not stage concurrent TUI files or their unrelated index hunks. Do not push
  without explicit user authorization.

---

## Execution Readiness View

- Intent Lock: exact ordered text/control PTY input in one Terminal write call.
- Scope Fence: Terminal schema/encoder, current JSON consumers, focused tests,
  and zh/en docs only.
- Baseline Lock: approved ordered-input design plus unchanged bounded-yield and
  raw Guardian boundaries.
- Approved Behavior: typed non-empty array; text newline normalization; exact
  C0/DEL byte insertion; one write and one bounded observation.
- Owner / Contract Constraints: `TerminalTool` validates/assembles; Guardian is
  an uninterpreted byte transport; receiver terminal modes decide effects.
- Compatibility Boundary: scalar input is invalid; no compatibility path.
- Retirement Boundary: delete scalar schema, fixtures, and docs; retain only a
  negative regression proving rejection.
- Task Batches: core contract, consumers/schema, real PTY, docs, three-host
  verification.
- Test Obligations: exact byte/negative/schema tests; existing yield/cap tests;
  Unix interrupt/session test; cross-platform session usability.
- Review Gates: stop if implementation needs Guardian/Bash/runtime/TUI changes,
  a signal API, an escape parser, or a second input owner.
- Drift / Rewind Rules: classify any failed platform test before editing; do
  not weaken a semantic assertion solely for a host.
- Evidence Required Before Completion: fresh exact commands with exit status,
  lingering scalar search, formatting/diff checks, and explicit uncovered host
  scope.
- Advisory Boundary: method-pack execution guidance only; not GateDecision,
  PolicySnapshot, or completion authority.

## Risks and Retirement

- Receiver behavior remains terminal-mode-dependent; exact bytes do not equal a
  cross-platform OS signal guarantee.
- Windows ConPTY may treat Ctrl+C specially depending on console mode; tests
  must separate byte/session evidence from signal semantics.
- SSH without a remote PTY may deliver control bytes to stdin without terminal
  line-discipline effects; docs name `ssh -tt` for that scenario.
- Large text parts still use existing Guardian chunking, which must preserve
  byte order; no wire change is planned.

**Anti-Entropy Declaration:**

- Deletion Class: contract-carrying code retirement.
- Old Path/Object: scalar `TerminalInput.input: Option<String>` and its docs /
  fixtures.
- New Canonical Owner: ordered typed parts encoded by `TerminalTool`.
- Expected Preserved Behavior: UTF-8 text input, newline normalization, one
  Guardian write, bounded post-write output, raw PTY semantics.
- Expected Retired Behavior: scalar strings as Terminal write input and
  control-key guidance based on JSON escape decoding.
- External Boundary Touched: yes, the model-facing tool schema.
- Source-of-Truth Data Risk: none; historical transcript data is not replayed.
- User Confirmation Required: no; the user explicitly approved the hard
  replacement and no persistent state is deleted.

**Retirement Decision:**

- Path: delete-first.
- Why: the scalar path is an internal/public-schema ambiguity with no proven
  external compatibility dependency and the approved design forbids dual mode.
- Non-edits: no historical JSONL migration, no Guardian protocol versioning,
  and no input alias.

**Verification Plan:**

- Main-path check: mixed ordered parts reach the PTY in one write.
- Lingering-reference check: no scalar production schema/docs/fixtures remain.
- Negative check: scalar JSON and invalid/empty controls are rejected.
- Boundary check: bounded-yield, offset, cap, cancellation, and session
  usability regressions remain green on supported hosts.
