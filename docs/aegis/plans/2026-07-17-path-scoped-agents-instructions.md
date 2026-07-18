# Path-Scoped AGENTS.md Instructions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `aegis:subagent-driven-development` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Neo's startup-only project context loading with one durable, session-scoped instruction runtime that discovers and applies trusted path-scoped `AGENTS.md` bundles before any applicable tool side effect.

**Architecture:** A session-shared `InstructionRegistry` resolves canonical scope chains, strict standalone `@path` imports, stable revisions, failures, and atomic budget admission. Each agent keeps independent model-visible instruction state; the runtime appends one durable `InstructionEpoch` event for baseline and later changes, defers the complete tool batch, and asks the model to replan inside the same turn. Compaction excludes instruction bodies from summaries and rehydrates exact current rules, while the TUI projects the same epoch into a metadata-only card at the deferred tools' earliest position.

**Tech Stack:** Rust 2024, Tokio, Serde/serde_json, Schemars, SHA-256 via the existing `sha2` workspace dependency, Neo's `AgentRuntime`/JSONL/compaction/transcript infrastructure, no new dependencies.

**Design specification:** `docs/aegis/specs/2026-07-17-path-scoped-agents-instructions-design.md`

## Global Constraints

- `AGENTS.md` is the only project instruction filename. Matching is case-insensitive; multiple case-folded variants in one directory are a blocking ambiguity.
- Delete the `CLAUDE.md` fallback, candidate-order behavior, `resources::load_context_files`, and all duplicate loaded-source state. Do not add a feature flag, compatibility branch, or second runtime.
- `$NEO_HOME/AGENTS.md` is always trusted. Project instructions and workspace-local imports are loaded only for a trusted primary workspace.
- Downward discovery is limited to `AppConfig.project_dir`; imported files must canonicalize inside the primary workspace or `$NEO_HOME`.
- Discover only the primary-workspace-to-target directory chains derived from typed tool arguments. Never recursively preload descendants, parse shell command text, guess MCP paths, or inspect additional workspace roots.
- The batch order is exactly `parse all -> instruction preflight -> authorize all -> fingerprint recheck -> execute all`. A deferred or blocked parallel batch never partially executes.
- A new, updated, removed, reactivated, partially admitted, or blocked instruction state appends an `InstructionEpoch`; it never rewrites an earlier system prompt/message or silently replays original tool calls.
- Every deferred assistant tool call receives one provider-valid non-error tool result. The model must issue a fresh batch after the epoch, inside the same user workflow.
- Imports are standalone single-`@` lines outside fenced code. Relative imports resolve from the importer; `~` uses the platform home. URLs, environment variables, inline mentions, `@@`, and fenced examples stay literal.
- Structural limits are fixed: maximum import depth `5`, maximum sources per graph `32`, maximum source size `1 MiB`, maximum complete graph size `8 MiB`. Structural or integrity failures block the whole bundle; never truncate or partially inject it.
- The nominal instruction budget is `max(65_536, effective_max_tokens / 8)`, clamped to safely available request capacity including existing output headroom and ordinary context.
- Over-budget admission reads and validates complete bundles, selects deterministic whole bundles, discards ignored bodies after measurement, continues after one model replan, and emits a `⚠` warning naming ignored bundles and token estimates.
- Admission priority is global, workspace root, deepest nested targets to shallowest, then trusted ancestors nearest first. Rendering order is global, ancestors outermost-to-nearest, workspace root, then nested scopes shallowest-to-deepest.
- Instruction content is exact pinned context. Full compaction excludes it from summary input and rehydrates it byte-for-byte. Micro projection never summarizes, truncates, removes, or rewrites instruction epochs.
- Source/revision cache is session-shared; model visibility is agent-local. Child agents receive their own baseline before their first model request and use the same preflight state machine.
- `InstructionEpoch` is the single persisted source for model content and transcript metadata. Do not also persist a `MessageAppended` copy.
- Transcript cards display metadata only, redact absolute home paths, absorb deferred placeholders at the earliest canonical position, and remain finalized in place.
- Preserve Windows/Linux/macOS behavior with `Path`/`PathBuf`; do not use string-prefix containment, Unix-only locks, or shell parsing.
- Update the paired English and Chinese docs and repository `AGENTS.md` in the same feature. `/init` creates or refreshes only workspace-root `AGENTS.md`.
- Follow TDD with exact tests. Completion evidence names one package, one target selector, and one exact test path/filter; broad workspace/package test runs are not evidence.
- Preserve unrelated worktree changes. Subagents must never run `git add`, `git commit`, `git checkout`, `git restore`, `git reset`, `git stash`, `git clean`, `git rebase`, or any other Git mutation. Only the coordinator stages exact reviewed files and commits.

## Frozen Interfaces

The task briefs must use these names and field types verbatim. Any necessary interface change discovered during implementation is a plan conflict and must be escalated before dependent tasks continue.

```rust
// crates/neo-agent-core/src/instructions/types.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionEpochOutcome {
    Ready,
    Activated,
    Updated,
    Removed,
    Reactivated,
    PartiallyLoaded,
    Blocked,
}

#[derive(Debug, Clone)]
pub struct InstructionRegistryConfig {
    pub primary_workspace: PathBuf,
    pub neo_home: Option<PathBuf>,
    pub project_trusted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionEpochData {
    pub agent_id: String,
    pub generation: u64,
    pub outcome: InstructionEpochOutcome,
    pub scopes: Vec<InstructionScopeData>,
    pub selected_bundles: Vec<InstructionBundleMetadata>,
    pub ignored_bundles: Vec<IgnoredInstructionBundle>,
    pub replacements: Vec<InstructionReplacement>,
    pub failure: Option<InstructionFailure>,
    pub deferred_tool_ids: Vec<String>,
    // Persisted once in this event and consumed only by model-context projection.
    pub model_content: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AgentInstructionState {
    pub visible_generation: u64,
    pub visible_revisions: BTreeMap<PathBuf, String>,
    pub active_scopes: Vec<PathBuf>,
    pub most_recent_scope: Option<PathBuf>,
    pub last_epoch_fingerprint: Option<String>,
}

pub enum InstructionPreflightDecision {
    Proceed { fingerprint: InstructionFingerprint },
    Defer { epoch: InstructionEpochData, fingerprint: InstructionFingerprint },
    Block { epoch: InstructionEpochData, fingerprint: InstructionFingerprint },
}

pub struct InstructionReconcileRequest {
    pub agent_id: String,
    pub kind: InstructionReconcileKind,
    pub target_directories: Vec<PathBuf>,
    pub budget: InstructionBudget,
    pub deferred_tool_ids: Vec<String>,
}

impl InstructionRegistry {
    pub fn new(config: InstructionRegistryConfig) -> Result<Self, InstructionError>;
    pub async fn reconcile(
        &self,
        request: InstructionReconcileRequest,
        state: &AgentInstructionState,
    ) -> InstructionPreflightDecision;
    pub async fn recheck(
        &self,
        fingerprint: &InstructionFingerprint,
        state: &AgentInstructionState,
    ) -> InstructionPreflightDecision;
    pub fn restore_epoch(&self, epoch: &InstructionEpochData);
    pub fn child_baseline_request(
        &self,
        parent: &AgentInstructionState,
        child_agent_id: String,
        inheritance: InstructionInheritance,
        budget: InstructionBudget,
    ) -> InstructionReconcileRequest;
}

pub fn find_agents_file(directory: &Path) -> Result<Option<PathBuf>, InstructionError>;
```

`InstructionScopeData`, `InstructionBundleMetadata`, `IgnoredInstructionBundle`, `InstructionReplacement`, and `InstructionFailure` contain only display-safe paths, revision/fingerprint strings, token/byte/import counts, and typed reasons. They never expose source bodies. `model_content` is the only expanded-content field.

The internal message model gains one explicit variant so compaction and projection can identify pinned rules without parsing marker text:

```rust
AgentMessage::Instruction {
    generation: u64,
    content: Vec<Content>,
}

AgentEvent::InstructionEpoch {
    epoch: InstructionEpochData,
}
```

`AgentMessage::Instruction` converts to a provider `ChatMessage::System`, is never emitted as `MessageAppended`, and is never exported as user/assistant prose.

## File And Ownership Map

| Area | Owned files | Responsibility |
|---|---|---|
| Instruction engine | `crates/neo-agent-core/src/instructions/{mod.rs,types.rs,resolver.rs,registry.rs}` | Canonical paths, stable reads, imports, revisions, failures, admission, shared cache |
| Durable semantics | `messages.rs`, `events.rs`, `runtime/context.rs`, `runtime/tokens.rs`, `runtime/image_blobs.rs`, `session/{mod.rs,event_persistence.rs}` | Epoch event, pinned internal message, replay and persistence |
| Runtime lifetime | `runtime/config.rs`, `multi_agent/runtime.rs` | Session registry seed and agent-local visibility |
| Context bridge | `runtime/instruction_context.rs`, `runtime/context_budget.rs`, `runtime/chat_request.rs`, `compaction/{mod.rs,projection.rs}` | Budget, pre-compaction admission, exact rehydration |
| Tool enforcement | `runtime/{agent.rs,turn_loop.rs,tool_dispatch.rs,permission.rs,tool_arguments.rs}`, `tools/terminal.rs`, tool schema descriptions | Typed probes, full-batch defer/block, two-phase authorization, fingerprint recheck |
| TUI component | `neo-tui/src/transcript/instruction_card.rs`, `transcript/entry/mod.rs`, `transcript/mod.rs` | Compact/expanded metadata-only card |
| TUI placement | `transcript/{store.rs,pane.rs,event_handler.rs}`, `shell/event_router.rs` | Earliest-position insertion, placeholder absorption, event routing |
| Neo startup/resume | `neo-agent/src/{resources.rs,trust.rs}`, `modes/run/runtime/agent.rs`, run/interactive session paths | Construct one registry, restore events, establish legacy-session baseline |
| Documentation | paired `docs/{en,zh}` files and repository `AGENTS.md` | Canonical user contract |

## Dependency DAG And Commit Ownership

```text
Task 1: engine + DTOs
   |
   +----------------+----------------+----------------+
   v                v                v                v
Task 2 events    Task 3 lifetime   Task 4 card     Task 5 migration
   |                |                |                |
   +--------+-------+                +-------+--------+
            |                                |
            v                                v
     Task 6 budget/compaction         Task 7 TUI placement
            |                                |
            +----------------+---------------+
                             v
                  Task 8 tool-batch enforcement
                             |
                     +-------+-------+
                     v               v
             Task 9 startup/e2e  Task 10 docs/residue
```

Parallel dispatch is allowed only within the displayed waves and only after the prerequisite review is clean. The coordinator records a base commit before each wave, waits for every implementer, reviews each task independently, stages the exact task write set, and creates these commits:

| Task | Coordinator commit |
|---|---|
| 1 | `feat: add scoped instruction registry` |
| 2 | `feat: persist instruction epochs` |
| 3 | `feat: share instructions across agent runtimes` |
| 4 | `feat: render instruction transcript cards` |
| 5 | `refactor: remove legacy project context loading` |
| 6 | `feat: preserve instructions across compaction` |
| 7 | `feat: position instruction transcript epochs` |
| 8 | `feat: enforce instructions before tool batches` |
| 9 | `feat: wire scoped instructions into sessions` |
| 10 | `docs: document scoped agents instructions` |

---

### Task 1: Build The Pure Instruction Engine And Shared DTOs

**Files:**
- Create: `crates/neo-agent-core/src/instructions/mod.rs`
- Create: `crates/neo-agent-core/src/instructions/types.rs`
- Create: `crates/neo-agent-core/src/instructions/resolver.rs`
- Create: `crates/neo-agent-core/src/instructions/registry.rs`
- Modify: `crates/neo-agent-core/src/lib.rs`
- Create: `crates/neo-agent-core/tests/instruction_registry.rs`

**Interfaces:**
- Produces every type and method in **Frozen Interfaces**.
- Produces the shared AGENTS-only `find_agents_file` detector used by Neo trust discovery, including case-insensitive matching and collision errors.
- Produces `InstructionBudget::from_context(effective_max_tokens, safely_available_tokens)` and deterministic `InstructionAdmission::select`.
- Produces typed `InstructionFailureKind` values for missing import, unreadable source, invalid encoding, include cycle, instruction limit exceeded, untrusted import, ambiguous `AGENTS.md`, and unstable source.
- Consumes no runtime, session, TUI, or Neo binary types beyond `Path`/`PathBuf` and the existing token estimator utility.

- [ ] **Step 1: Add resolver tests that fail before any production module exists.**

Create fixture helpers that write temporary workspace and `$NEO_HOME` trees, then add exact tests with these assertions:

```rust
#[test]
fn resolver_merges_target_chains_general_to_specific_without_siblings() {
    // workspace/AGENTS.md, workspace/crates/AGENTS.md,
    // workspace/crates/ui/AGENTS.md, workspace/docs/AGENTS.md
    // Probe crates/ui/src/lib.rs; selected scopes are workspace, crates, crates/ui.
    // docs never appears and rendering keeps the same general-to-specific order.
}

#[test]
fn resolver_expands_only_standalone_imports_outside_fences_in_place() {
    // Expand @./rules.md and @~/.neo/CX.md with escaped provenance wrappers.
    // Keep @@./x.md, inline @x, fenced @x, URL, and $ENV forms byte-identical.
}

#[test]
fn resolver_rejects_casefold_collision_and_canonical_escape() {
    // AGENTS.md + agents.MD is ambiguous; a symlink/.. outside both roots is untrusted.
}
```

- [ ] **Step 2: Run the three exact tests and confirm RED.**

```bash
cargo test --package neo-agent-core --test instruction_registry resolver_merges_target_chains_general_to_specific_without_siblings -- --exact --nocapture
cargo test --package neo-agent-core --test instruction_registry resolver_expands_only_standalone_imports_outside_fences_in_place -- --exact --nocapture
cargo test --package neo-agent-core --test instruction_registry resolver_rejects_casefold_collision_and_canonical_escape -- --exact --nocapture
```

Expected: compilation fails because `neo_agent_core::instructions` and its types do not exist.

- [ ] **Step 3: Implement canonical scope discovery, strict import parsing, and stable reads.**

Use a line-state parser that toggles fenced blocks only for Markdown backtick/tilde fences, accepts exactly one leading `@` on an otherwise standalone line, and replaces a directive at its byte position with:

```text
<included_instructions path="DISPLAY_PATH">
EXACT_UTF8_SOURCE
</included_instructions>
```

Canonicalize roots and every existing source; compare paths through `Path::starts_with` only after canonicalization. Read each source with `metadata A -> bytes -> metadata B`, retry once on change, then hash accepted bytes with SHA-256. Do not cache a missing directory result across calls. Enforce `5`, `32`, `1 MiB`, and `8 MiB` before returning a bundle.

- [ ] **Step 4: Add failing admission, revision, and failure-dedup tests.**

```rust
#[test]
fn admission_uses_dynamic_cap_and_keeps_atomic_bundles_in_priority_order() {
    assert_eq!(InstructionBudget::from_context(Some(1_048_576), 200_000).nominal, 131_072);
    assert_eq!(InstructionBudget::from_context(Some(131_072), 40_000).actual, 40_000);
    // Assert global -> root -> deepest nested -> shallow nested -> nearest ancestor admission.
    // Assert model rendering is global -> outer ancestors -> root -> shallowest nested -> deepest.
}

#[test]
fn identical_content_and_failure_fingerprints_do_not_create_new_epochs() {
    // mtime-only rewrite returns Proceed; same source/failure/fingerprint returns Proceed.
    // Changed bytes create Updated with a replacement revision.
}

#[test]
fn missing_results_are_not_cached_across_reconcile_calls() {
    // First reconcile has no nested AGENTS.md; create it; second reconcile returns Defer.
}

#[test]
fn resolver_reports_every_atomic_structural_and_integrity_failure() {
    // Table cases: missing/unreadable/invalid UTF-8/special import, cycle,
    // depth 6, source 33, source >1 MiB, graph >8 MiB, untrusted path,
    // ambiguous AGENTS.md, and twice-changing unstable source.
    // Each returns one blocked bundle and injects none of its readable subset.
}
```

- [ ] **Step 5: Implement registry reconciliation and atomic budget admission.**

Keep shared source/bundle caches and in-flight keyed reads behind `Arc` + Tokio synchronization. `reconcile` freezes one generation, resolves the union of all target chains, selects complete bundles, compares them with the supplied agent state, and returns exactly one `Proceed`, `Defer`, or `Block`. Ignored bundle state stores only display path, hash, token estimate, and omission reason; drop ignored source bodies before returning. Repeated identical selection/failure fingerprints return `Proceed`.

- [ ] **Step 6: Run focused engine verification.**

```bash
cargo test --package neo-agent-core --test instruction_registry admission_uses_dynamic_cap_and_keeps_atomic_bundles_in_priority_order -- --exact --nocapture
cargo test --package neo-agent-core --test instruction_registry identical_content_and_failure_fingerprints_do_not_create_new_epochs -- --exact --nocapture
cargo test --package neo-agent-core --test instruction_registry missing_results_are_not_cached_across_reconcile_calls -- --exact --nocapture
cargo test --package neo-agent-core --test instruction_registry resolver_reports_every_atomic_structural_and_integrity_failure -- --exact --nocapture
```

Expected: all seven named Task 1 tests pass. Subagent reports changed files and test output; coordinator performs spec/quality review and commits only this task's files.

---

### Task 2: Add The Durable Epoch, Pinned Message, Persistence, And Replay State

**Files:**
- Modify: `crates/neo-agent-core/src/messages.rs`
- Modify: `crates/neo-agent-core/src/events.rs`
- Modify: `crates/neo-agent-core/src/runtime/context.rs`
- Modify: `crates/neo-agent-core/src/runtime/tokens.rs`
- Modify: `crates/neo-agent-core/src/runtime/image_blobs.rs`
- Modify: `crates/neo-agent-core/src/session/mod.rs`
- Modify: `crates/neo-agent-core/src/session/event_persistence.rs`
- Modify: `crates/neo-agent-core/tests/session_jsonl.rs`

**Interfaces:**
- Consumes `InstructionEpochData`, `AgentInstructionState`, and `InstructionRegistry` from Task 1.
- Produces `AgentMessage::Instruction { generation, content }` and `AgentEvent::InstructionEpoch { epoch }`.
- Produces `AgentContext::apply_instruction_epoch`, `instruction_state`, `instruction_state_mut`, `attach_instruction_registry`, and `instruction_registry`.
- `AgentContext` serializes `AgentInstructionState`, skips the host-only `Option<Arc<InstructionRegistry>>`, and rebuilds messages from replayed epoch events.

- [ ] **Step 1: Write failing JSONL and replay tests.**

```rust
#[tokio::test]
async fn instruction_epoch_persists_once_and_replays_model_context() {
    // Append one InstructionEpoch with model_content.
    // Assert wire JSON has one instruction_epoch and no MessageAppended copy.
    // Replay and assert one AgentMessage::Instruction plus matching state generation.
}

#[test]
fn instruction_message_converts_to_provider_system_message_and_counts_tokens() {
    // Assert exact content survives AgentMessage -> ChatMessage conversion.
    // Assert token estimation includes it and image-blob resolution leaves it unchanged.
}
```

- [ ] **Step 2: Run exact tests and confirm RED.**

```bash
cargo test --package neo-agent-core --test session_jsonl instruction_epoch_persists_once_and_replays_model_context -- --exact --nocapture
cargo test --package neo-agent-core --lib -- messages::tests::instruction_message_converts_to_provider_system_message_and_counts_tokens --exact --nocapture
```

Expected: compilation fails for missing event/message variants.

- [ ] **Step 3: Add the canonical event and internal message projection.**

Add the frozen variants. `AgentContext::apply_instruction_epoch` must append an `AgentMessage::Instruction` only when `epoch.model_content` is `Some`, update `AgentInstructionState`, and never synthesize `MessageAppended`. Update every exhaustive `AgentMessage` match in the owned files: provider conversion maps it to `ChatMessage::System`; token counting includes the body; blob resolution passes it through; session role/export helpers label or omit it without exposing content as conversation prose.

- [ ] **Step 4: Make persistence and replay event-driven.**

`SessionEventPersistence::persisted_events` returns the epoch itself exactly once. `AgentContext::from_replay` applies epoch events in wire order, reconstructing replacements/removals and agent-local visibility before any live disk reconciliation. Old sessions with no epoch leave `visible_generation == 0`, which Task 8 uses to establish a fresh baseline.

- [ ] **Step 5: Verify single-source persistence and replay ordering.**

```bash
cargo test --package neo-agent-core --test session_jsonl instruction_epoch_persists_once_and_replays_model_context -- --exact --nocapture
cargo test --package neo-agent-core --lib -- messages::tests::instruction_message_converts_to_provider_system_message_and_counts_tokens --exact --nocapture
cargo test --package neo-agent-core --lib -- runtime::context::tests::replay_instruction_replacement_preserves_historical_messages_and_updates_authority --exact --nocapture
```

Expected: all three tests pass; the replacement appends a second pinned message and marks the earlier revision replaced without rewriting it.

---

### Task 3: Preserve Registry Lifetime And Seed Child-Agent Visibility

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/config.rs`
- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

**Interfaces:**
- Consumes Task 1 registry types and Task 2 `AgentContext` attachment/state APIs.
- Adds skipped fields `AgentConfig::instruction_registry: Option<Arc<InstructionRegistry>>` and `AgentConfig::instruction_inheritance: InstructionInheritance`.
- Produces child runtime setup that shares source caches but always creates child-owned `AgentInstructionState` and child baseline request.

- [ ] **Step 1: Add failing registry-sharing and visibility-isolation tests.**

```rust
#[tokio::test]
async fn child_runtime_shares_registry_but_not_parent_visibility() {
    // Seed parent with a visible nested revision.
    // Build an inherit child and a summary child.
    // Assert Arc::ptr_eq registry for both.
    // Assert inherit may seed explicit visible revisions, summary starts agent-local baseline.
}

#[tokio::test]
async fn concurrent_children_singleflight_the_same_source_read() {
    // Instrument the registry file reader; two child baselines read one source once.
}
```

- [ ] **Step 2: Run both exact tests and confirm RED.**

```bash
cargo test --package neo-agent-core --test multi_agent_runtime child_runtime_shares_registry_but_not_parent_visibility -- --exact --nocapture
cargo test --package neo-agent-core --test multi_agent_runtime concurrent_children_singleflight_the_same_source_read -- --exact --nocapture
```

- [ ] **Step 3: Wire the session seed through `AgentConfig` and child creation.**

Clone only the registry handle and immutable configuration into `child_config`. At the `AgentContext::new()` site in `run_agent_snapshot`, attach the shared registry, create child state keyed by the actual child `agent_id`, and materialize a child-owned baseline before the child's first prompt. `inherit` may explicitly copy revisions already represented in inherited full messages; `summary` and `none` must not infer visibility from prose. Child epochs remain in the child's wire JSONL.

- [ ] **Step 4: Verify lifetime and per-agent behavior.**

```bash
cargo test --package neo-agent-core --test multi_agent_runtime child_runtime_shares_registry_but_not_parent_visibility -- --exact --nocapture
cargo test --package neo-agent-core --test multi_agent_runtime concurrent_children_singleflight_the_same_source_read -- --exact --nocapture
```

Expected: both tests pass and no process-global registry is introduced.

---

### Task 4: Build The Metadata-Only Instruction Card Component

**Files:**
- Create: `crates/neo-tui/src/transcript/instruction_card.rs`
- Modify: `crates/neo-tui/src/transcript/entry/mod.rs`
- Modify: `crates/neo-tui/src/transcript/mod.rs`
- Modify: `crates/neo-tui/tests/transcript.rs`

**Interfaces:**
- Consumes `InstructionEpochData` and `InstructionEpochOutcome`.
- Produces `InstructionCardComponent::new`, `id`, `set_expanded`, `render_with_theme`, and `copy_text`.
- Adds `TranscriptEntry::InstructionEpoch { component }`; Task 7 owns insertion and routing.

- [ ] **Step 1: Add failing compact, expanded, and redaction tests.**

```rust
#[test]
fn instruction_card_renders_outcome_metadata_without_model_content() {
    // Ready: brand styling; Updated/PartiallyLoaded: status_warn; Blocked: status_error.
    // Assert compact labels and token/import counts.
    // Assert secret sentinel in model_content never renders or copies.
}

#[test]
fn expanded_instruction_card_lists_loaded_ignored_imports_and_redacted_paths() {
    // Assert workspace-relative and ~/ paths, ignored reasons, token estimates, revision.
    // Assert no absolute home prefix and no source body.
}
```

- [ ] **Step 2: Run the exact tests and confirm RED.**

```bash
cargo test --package neo-tui --test transcript instruction_card_renders_outcome_metadata_without_model_content -- --exact --nocapture
cargo test --package neo-tui --test transcript expanded_instruction_card_lists_loaded_ignored_imports_and_redacted_paths -- --exact --nocapture
```

- [ ] **Step 3: Implement a finalized semantic card.**

Map `Ready`, `Activated`, `Reactivated` to brand/muted styles; `Updated` and `PartiallyLoaded` to `status_warn`; `Blocked` to `status_error`; `Removed` to muted removal styling. Compact labels follow the spec exactly. Expanded rows contain only scope, loaded bundle metadata, ignored bundle metadata/reasons, imports, and revision. Build all display text from metadata fields; never inspect `model_content`.

- [ ] **Step 4: Verify rendering and expansion.**

```bash
cargo test --package neo-tui --test transcript instruction_card_renders_outcome_metadata_without_model_content -- --exact --nocapture
cargo test --package neo-tui --test transcript expanded_instruction_card_lists_loaded_ignored_imports_and_redacted_paths -- --exact --nocapture
```

Expected: both tests pass and the entry reports finalized semantics rather than a live spinner.

---

### Task 5: Remove The Legacy Loader And `CLAUDE.md` Trust Fallback

**Files:**
- Modify: `crates/neo-agent/src/resources.rs`
- Modify: `crates/neo-agent/src/trust.rs`
- Modify: `crates/neo-agent/src/modes/run/runtime/agent.rs`

**Interfaces:**
- Produces AGENTS-only trust discovery with case-fold collision reporting.
- Removes `ContextFile`, `load_context_files`, `load_context_file_from_dir`, `format_project_context`, and project-context concatenation from `load_system_prompt`.
- Removes the obsolete `project_dir`/`project_trusted` parameters from `load_system_prompt` and updates its runtime caller; Task 9 later revisits that caller to create the registry.
- Leaves `SYSTEM.md`, `APPEND_SYSTEM.md`, built-in tool guidance, and skill catalog loading unchanged.

- [ ] **Step 1: Replace old fallback tests with failing canonical tests.**

```rust
#[test]
fn trust_inputs_accept_only_agents_md_case_insensitively() {
    // AGENTS.md / agents.md is detected; lone CLAUDE.md is ignored.
}

#[test]
fn trust_inputs_report_ambiguous_casefolded_agents_files() {
    // Two variants return the same typed ambiguity used by the registry.
}

#[test]
fn system_prompt_does_not_append_project_instruction_files() {
    // SYSTEM + APPEND + skills remain; AGENTS body sentinel is absent.
}
```

- [ ] **Step 2: Run exact tests and confirm RED against current fallback behavior.**

```bash
cargo test --package neo-agent --bin neo -- trust::tests::trust_inputs_accept_only_agents_md_case_insensitively --exact --nocapture
cargo test --package neo-agent --bin neo -- trust::tests::trust_inputs_report_ambiguous_casefolded_agents_files --exact --nocapture
cargo test --package neo-agent --bin neo -- resources::tests::system_prompt_does_not_append_project_instruction_files --exact --nocapture
```

- [ ] **Step 3: Delete the old path and keep trust semantics narrow.**

Make trust collection search only case-insensitive `AGENTS.md`, return/propagate ambiguity rather than selecting a first candidate, and ignore `CLAUDE.md`. Remove all project instruction reads from `load_system_prompt`; do not leave wrappers, dead candidates, or compatibility comments.

- [ ] **Step 4: Verify migration behavior.**

```bash
cargo test --package neo-agent --bin neo -- trust::tests::trust_inputs_accept_only_agents_md_case_insensitively --exact --nocapture
cargo test --package neo-agent --bin neo -- trust::tests::trust_inputs_report_ambiguous_casefolded_agents_files --exact --nocapture
cargo test --package neo-agent --bin neo -- resources::tests::system_prompt_does_not_append_project_instruction_files --exact --nocapture
```

Expected: all three tests pass and `rg -n "load_context_files|format_project_context|CLAUDE.md" crates/neo-agent/src/{resources.rs,trust.rs}` has no matches.

---

### Task 6: Integrate Dynamic Budget, Prefix Stability, And Exact Compaction Rehydration

**Files:**
- Create: `crates/neo-agent-core/src/runtime/instruction_context.rs`
- Modify: `crates/neo-agent-core/src/runtime/mod.rs`
- Modify: `crates/neo-agent-core/src/runtime/context_budget.rs`
- Modify: `crates/neo-agent-core/src/runtime/chat_request.rs`
- Modify: `crates/neo-agent-core/src/compaction/mod.rs`
- Modify: `crates/neo-agent-core/src/compaction/projection.rs`
- Modify: `crates/neo-agent-core/tests/runtime_turn.rs`

**Interfaces:**
- Consumes Task 1 admission APIs and Task 2 pinned message/state.
- Produces `InstructionContextBridge::budget`, `prepare_pending_epoch`, `apply_epoch`, and `rehydrate_after_compaction`.
- Produces `ContextBudgetSnapshot::safely_available_instruction_tokens` without granting capacity beyond effective/absolute maximum or reserved output headroom.
- Task 8 calls the bridge before each provider request and after preflight.

- [ ] **Step 1: Add failing budget and prefix tests.**

```rust
#[test]
fn instruction_budget_is_max_64k_or_one_eighth_then_safely_clamped() {
    // 32K -> nominal 65_536 but actual <= safe capacity.
    // 128K -> 65_536; 1M -> 131_072; observed provider cap replaces advertised cap.
}

#[tokio::test]
async fn adjacent_requests_keep_the_complete_previous_message_prefix() {
    // Capture request N, activate nested epoch, capture N+1.
    // Assert N.messages == N+1.messages[..N.messages.len()].
    // Assert stable system prompt, tool ordering, reasoning settings, cache key.
}
```

- [ ] **Step 2: Add failing compaction and projection tests.**

```rust
#[tokio::test]
async fn compaction_excludes_instruction_bodies_and_rehydrates_exact_bytes() {
    // Put a unique sentinel in Instruction message and ordinary history.
    // Assert summary request excludes sentinel.
    // Assert post-compaction request contains byte-identical sentinel content once.
}

#[test]
fn micro_projection_never_changes_instruction_messages() {
    // Project large tool results around an instruction epoch; instruction variant is untouched.
}

#[tokio::test]
async fn compacted_sibling_scope_reactivates_when_reentered() {
    // Activate scopes A then B, compact while B is current, then probe A.
    // A remains cached but unpinned and emits one Reactivated epoch on re-entry.
}
```

- [ ] **Step 3: Run exact tests and confirm RED.**

```bash
cargo test --package neo-agent-core --lib -- runtime::context_budget::tests::instruction_budget_is_max_64k_or_one_eighth_then_safely_clamped --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn adjacent_requests_keep_the_complete_previous_message_prefix -- --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn compaction_excludes_instruction_bodies_and_rehydrates_exact_bytes -- --exact --nocapture
cargo test --package neo-agent-core --lib -- compaction::projection::tests::micro_projection_never_changes_instruction_messages --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn compacted_sibling_scope_reactivates_when_reentered -- --exact --nocapture
```

- [ ] **Step 4: Implement the context bridge and safe capacity calculation.**

Compute `effective_max_tokens` with the existing observed-overflow correction. Compute safe capacity after fixed request overhead, tool schemas, reserved output headroom, and current ordinary context. Before applying a pending epoch, request full compaction when it would cross the existing threshold; never append it and immediately summarize it. If compaction cannot create enough safe capacity, apply deterministic whole-bundle omission or return the existing typed context error.

- [ ] **Step 5: Exclude and rehydrate pinned instruction messages.**

Full compaction summary input filters `AgentMessage::Instruction`; after applying the summary, `rehydrate_after_compaction` appends exact current global, initial workspace, and current/most-recent nested scope content from registry state. Drop visited sibling bodies from pinned context while retaining metadata. Re-entry produces `Reactivated`; invisible rehydration of the already-current scope produces no card. Projection returns every instruction variant byte-for-byte.

- [ ] **Step 6: Run focused context verification.**

```bash
cargo test --package neo-agent-core --lib -- runtime::context_budget::tests::instruction_budget_is_max_64k_or_one_eighth_then_safely_clamped --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn adjacent_requests_keep_the_complete_previous_message_prefix -- --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn compaction_excludes_instruction_bodies_and_rehydrates_exact_bytes -- --exact --nocapture
cargo test --package neo-agent-core --lib -- compaction::projection::tests::micro_projection_never_changes_instruction_messages --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn compacted_sibling_scope_reactivates_when_reentered -- --exact --nocapture
```

Expected: all five tests pass; no test relies on string markers to recognize instruction content.

---

### Task 7: Insert Instruction Cards And Absorb Deferred Tool Placeholders

**Files:**
- Modify: `crates/neo-tui/src/transcript/store.rs`
- Modify: `crates/neo-tui/src/transcript/pane.rs`
- Modify: `crates/neo-tui/src/transcript/event_handler.rs`
- Modify: `crates/neo-tui/src/shell/event_router.rs`
- Modify: `crates/neo-tui/tests/transcript_store.rs`
- Modify: `crates/neo-tui/tests/transcript_pane.rs`

**Interfaces:**
- Consumes Task 4 `TranscriptEntry::InstructionEpoch` and component.
- Produces `TranscriptStore::insert_instruction_epoch(epoch) -> TranscriptEntryId`.
- Reuses existing `suppress_tool_run`; does not redesign `presentation.rs`.

- [ ] **Step 1: Add failing live placement and replay-idempotence tests.**

```rust
#[test]
fn instruction_epoch_replaces_deferred_placeholders_at_earliest_position() {
    // Add pending Read, Grep, Bash entries in order, then epoch with all three ids.
    // Assert one card occupies Read's canonical position and all placeholders are suppressed.
    // Add actual retried tools; assert they appear after the fixed card.
}

#[test]
fn replayed_instruction_epoch_has_identical_order_and_no_duplicate_card() {
    // Replay tool calls/results/epoch twice through normal event routing.
    // Assert stable order and one fingerprinted card.
}
```

- [ ] **Step 2: Run exact tests and confirm RED.**

```bash
cargo test --package neo-tui --test transcript_store instruction_epoch_replaces_deferred_placeholders_at_earliest_position -- --exact --nocapture
cargo test --package neo-tui --test transcript_pane replayed_instruction_epoch_has_identical_order_and_no_duplicate_card -- --exact --nocapture
```

- [ ] **Step 3: Route epochs and perform canonical insertion.**

On `AgentEvent::InstructionEpoch`, find the minimum current index among `deferred_tool_ids`, suppress every matching unexecuted tool run, and insert the finalized card at that index. If no placeholder exists, append normally. Deduplicate by `(agent_id, generation, scope/revision/selection/failure fingerprint)`. Mark dirtiness from the insertion index so later updates do not move the card. Never delete provider history or JSONL events.

- [ ] **Step 4: Verify placement and card stability.**

```bash
cargo test --package neo-tui --test transcript_store instruction_epoch_replaces_deferred_placeholders_at_earliest_position -- --exact --nocapture
cargo test --package neo-tui --test transcript_pane replayed_instruction_epoch_has_identical_order_and_no_duplicate_card -- --exact --nocapture
cargo test --package neo-tui --test transcript_pane finalized_instruction_card_does_not_drift_after_later_updates -- --exact --nocapture
```

Expected: all three tests pass without modifying `crates/neo-tui/src/transcript/presentation.rs`.

---

### Task 8: Enforce Instruction Preflight Before Permission And Tool Execution

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/agent.rs`
- Modify: `crates/neo-agent-core/src/runtime/turn_loop.rs`
- Modify: `crates/neo-agent-core/src/runtime/tool_dispatch.rs`
- Modify: `crates/neo-agent-core/src/runtime/permission.rs`
- Modify: `crates/neo-agent-core/src/runtime/tool_arguments.rs`
- Modify: `crates/neo-agent-core/src/tools/terminal.rs`
- Modify: `crates/neo-agent-core/src/tools/bash.rs`
- Modify: `crates/neo-agent-core/tests/runtime_turn.rs`
- Modify: `crates/neo-agent-core/tests/tool_schema_descriptions.rs`

**Interfaces:**
- Consumes Tasks 1-3 and 6 registry/context APIs.
- Produces typed `InstructionScopeProbe::from_prepared_tool(name, arguments, primary_workspace)`.
- Produces one batch pipeline: parse all calls; preflight all typed paths; authorize all; recheck one frozen fingerprint; execute all.
- Adds optional `cwd` to `TerminalInput`, used only for `mode = start`; Bash/Terminal guidance requires explicit nested `cwd` because shell command text is never parsed.

- [ ] **Step 1: Add failing typed-probe and complete-batch deferral tests.**

```rust
#[tokio::test]
async fn first_nested_edit_defers_before_side_effect_and_retried_batch_executes_once() {
    // Nested AGENTS exists. Fake model first emits Edit, then after epoch emits Edit again.
    // Assert file unchanged at epoch boundary, one deferred result, then one final edit.
}

#[tokio::test]
async fn one_new_scope_defers_every_call_in_a_parallel_mixed_batch() {
    // Batch contains root Read, nested Write, root Grep.
    // Assert zero tools execute and every call receives a matching non-error deferred result.
}

#[tokio::test]
async fn first_read_write_and_nested_cwd_shell_each_defer_before_execution() {
    // Run separate Read, Write, Bash(cwd), and Terminal(start,cwd) cases.
    // Assert the first attempt for each emits deferred result + epoch and no side effect.
}

#[test]
fn typed_scope_probes_cover_files_roots_and_explicit_shell_cwds_only() {
    // Read/Write/Edit -> parent; List/Grep/Find/Glob -> explicit root;
    // Bash/Terminal start -> explicit cwd or workspace; Terminal write/read/stop -> no new probe;
    // external Read/additional roots/MCP -> none; shell command strings are ignored.
}
```

- [ ] **Step 2: Run exact tests and confirm RED.**

```bash
cargo test --package neo-agent-core --test runtime_turn first_nested_edit_defers_before_side_effect_and_retried_batch_executes_once -- --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn one_new_scope_defers_every_call_in_a_parallel_mixed_batch -- --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn first_read_write_and_nested_cwd_shell_each_defer_before_execution -- --exact --nocapture
cargo test --package neo-agent-core --lib -- runtime::tool_arguments::tests::typed_scope_probes_cover_files_roots_and_explicit_shell_cwds_only --exact --nocapture
```

- [ ] **Step 3: Split preparation, authorization, and execution into explicit phases.**

Refactor `execute_tool_calls` so no `permission_preparation_for_mode`, approval handler, `before_tool_call`, scheduling, `ToolExecutionStarted`, or tool body runs until every argument is parsed and instruction preflight returns `Proceed`. Invalid tool arguments still receive valid error results, but they do not let another valid call bypass preflight. Build the permission decisions for the full batch, await dialogs sequentially where required, then fingerprint-recheck once before scheduling any call.

- [ ] **Step 4: Emit provider-valid deferred/blocked results and one epoch.**

For `Defer`, return one `ToolResult::success` per original call with a compact machine-readable payload such as:

```json
{"status":"deferred","reason":"instruction_epoch","side_effect_occurred":false,"generation":7}
```

Append all tool results in original call order, then emit/apply exactly one `InstructionEpoch`; continue `run_agent_turn` to the next model request without `TurnFinished`, a new user message, or automatic tool replay. For `Block`, use structured results and the epoch: after its fingerprint is visible, allow only an all-read-only diagnostic batch; block mutation/execution or any mixed batch as a whole.

- [ ] **Step 5: Establish baseline-before-user ordering and post-tool reconciliation.**

In `AgentRuntime::run_turn_with_cancel`, establish a missing baseline epoch before line 206's user `MessageAppended` equivalent. This applies to new sessions and pre-feature resumes. After any actual tool result and before the next provider request, reconcile active files: a tool that edits/removes an instruction source is governed by the old revision for that tool, then produces `Updated`/`Removed`. A newly created nested `AGENTS.md` is picked up before any later batch in that scope.

- [ ] **Step 6: Add permission-race and failure-policy tests.**

```rust
#[tokio::test]
async fn approval_wait_rechecks_instruction_fingerprint_before_execution() {
    // Change AGENTS while approval is pending; approval completion must defer, not execute.
}

#[tokio::test]
async fn blocked_scope_allows_read_only_diagnosis_but_blocks_mixed_mutation_batch() {
    // Missing import -> visible Blocked epoch; Read proceeds next; Read+Edit batch both block.
}

#[tokio::test]
async fn baseline_epoch_precedes_first_user_message_for_new_and_legacy_sessions() {
    // Assert event/message ordering for empty context and replay with no prior epoch.
}
```

- [ ] **Step 7: Add Terminal cwd and model guidance.**

Add `cwd: Option<String>` to `TerminalInput`; reject it for non-`start` modes, resolve it through existing workspace path policy for `start`, and pass the resolved directory into the PTY launch. Update Bash and Terminal schema descriptions plus base tool-use guidance to say nested commands must supply `cwd`; do not inspect the command string.

- [ ] **Step 8: Run the enforcement boundary tests.**

```bash
cargo test --package neo-agent-core --test runtime_turn first_nested_edit_defers_before_side_effect_and_retried_batch_executes_once -- --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn one_new_scope_defers_every_call_in_a_parallel_mixed_batch -- --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn first_read_write_and_nested_cwd_shell_each_defer_before_execution -- --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn approval_wait_rechecks_instruction_fingerprint_before_execution -- --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn blocked_scope_allows_read_only_diagnosis_but_blocks_mixed_mutation_batch -- --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn baseline_epoch_precedes_first_user_message_for_new_and_legacy_sessions -- --exact --nocapture
cargo test --package neo-agent-core --test tool_schema_descriptions terminal_start_exposes_cwd_and_requires_it_for_nested_scope -- --exact --nocapture
```

Expected: all seven tests pass; captured event order proves no permission prompt or tool-start event precedes instruction preflight.

---

### Task 9: Wire One Registry Through Neo Startup, Resume, TUI, And Child Sessions

**Files:**
- Modify: `crates/neo-agent/src/modes/run/runtime/agent.rs`
- Modify: `crates/neo-agent/src/modes/run/mod.rs`
- Modify: `crates/neo-agent/src/modes/run/output/json.rs`
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`
- Modify: `crates/neo-agent/src/modes/interactive/sessions.rs`
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`
- Modify: `crates/neo-agent/tests/mock_provider_e2e.rs`

**Interfaces:**
- Consumes canonical trust decision from Task 5 and all core/TUI behavior through Task 8.
- Constructs exactly one `InstructionRegistryConfig { primary_workspace, neo_home, project_trusted }` per session and attaches its registry to `AgentConfig`/`AgentContext`.
- Restores historical epochs into registry and agent state before first live disk reconcile.

- [ ] **Step 1: Add failing startup and unchanged-resume tests.**

```rust
#[tokio::test]
async fn startup_builds_one_registry_and_baseline_before_first_provider_request() {
    // Global + ancestor + root AGENTS fixtures; capture request and events.
    // Assert one Ready epoch and semantic render order before user content.
}

#[tokio::test]
async fn unchanged_resume_replays_epoch_without_duplicate_message_or_card() {
    // Persist and resume; assert registry restored first and no new Ready/Updated epoch.
}

#[tokio::test]
async fn changed_source_after_resume_appends_replacement_before_provider_call() {
    // Mutate active source after writing session; resume; assert one Updated replacement.
}
```

- [ ] **Step 2: Run exact tests and confirm RED.**

```bash
cargo test --package neo-agent --bin neo -- modes::run::tests::startup_builds_one_registry_and_baseline_before_first_provider_request --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::run::tests::unchanged_resume_replays_epoch_without_duplicate_message_or_card --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::run::tests::changed_source_after_resume_appends_replacement_before_provider_call --exact --nocapture --include-ignored
```

- [ ] **Step 3: Build and restore the session registry.**

In `agent_config_for_app`, canonicalize `config.project_dir`, pass `neo_home()` and the resolved trust boolean, create the registry once, and store its `Arc` seed in `AgentConfig`. For print/run/TUI resume, read JSONL events first, call `restore_epoch` in wire order, attach the same registry to replayed `AgentContext`, then allow the runtime's first provider boundary to reconcile disk state. A session with no historical epoch receives current baseline; unchanged modern sessions receive none.

- [ ] **Step 4: Route epoch events through all live and replay surfaces.**

Persist the event through the existing event writer, map it in JSON output as metadata only, feed it to the TUI during live operation and `replay_session_into_transcript`, and keep child events in child JSONL/transcripts. Do not mirror child cards into the main transcript in v1.

- [ ] **Step 5: Add end-to-end path-scope and budget-warning coverage.**

```rust
#[tokio::test]
async fn nested_scope_import_and_over_budget_warning_replan_without_breaking_turn() {
    // Mock provider issues a nested Read; fixture has an imported rule and one oversized bundle.
    // Assert defer -> PartiallyLoaded epoch -> fresh call -> success in same turn.
    // Assert warning names loaded/ignored bundles and contains no ignored body sentinel.
}
```

- [ ] **Step 6: Run focused integration verification.**

```bash
cargo test --package neo-agent --bin neo -- modes::run::tests::startup_builds_one_registry_and_baseline_before_first_provider_request --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::run::tests::unchanged_resume_replays_epoch_without_duplicate_message_or_card --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::run::tests::changed_source_after_resume_appends_replacement_before_provider_call --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::run::tests::nested_scope_import_and_over_budget_warning_replan_without_breaking_turn --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::run::tests::stable_json_redacts_instruction_metadata_paths_and_failure_detail --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::instruction_registry_cache_is_scoped_by_session_id --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::event_loop_keeps_new_session_active_for_followup_prompt --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::rebuild_transcript_sets_workspace_root_before_replaying_instruction_cards --exact --nocapture --include-ignored
```

Expected: all eight tests pass and one continuous run contains both preflight replan and final answer.

---

### Task 10: Update English/Chinese Documentation And Prove Canonical Migration

**Files:**
- Modify: `docs/en/customization/agents.md`
- Modify: `docs/zh/customization/agents.md`
- Modify: `docs/en/configuration/config-files.md`
- Modify: `docs/zh/configuration/config-files.md`
- Modify: `docs/en/configuration/data-locations.md`
- Modify: `docs/zh/configuration/data-locations.md`
- Modify: `docs/en/reference/slash-commands.md`
- Modify: `docs/zh/reference/slash-commands.md`
- Modify: `AGENTS.md`

**Interfaces:**
- Documents the exact approved behavior; introduces no config knobs or alternate semantics.
- Keeps English and Chinese headings/examples semantically paired.

- [ ] **Step 1: Rewrite the paired `AGENTS.md` customization contract.**

Document global + trusted ancestor + workspace baseline; typed-path nested discovery; directory-chain ordering; standalone recursive imports and allowed roots; all structural limits; preflight defer/replan; read-only diagnosis versus mutation blocking; dynamic budget formula and whole-bundle warning; prefix-cache behavior; exact compaction; resume; and shared-cache/agent-local visibility.

Include this portable scope example in both languages:

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

- [ ] **Step 2: Update configuration, locations, slash commands, and repository guide.**

Remove every `AGENTS.md / CLAUDE.md` compatibility statement. Explain that project instructions no longer mutate `system_prompt`; instruction epochs are durable JSONL context. State that `/init` creates/refreshes only workspace-root `AGENTS.md`, while nested files remain user-authored. Update the repository runtime quick reference to describe trust-gated canonical `AGENTS.md`, imports, path-scoped preflight, and transcript warning behavior.

- [ ] **Step 3: Run canonical residue scans.**

```bash
status=0
rg -n "CLAUDE\.md|load_context_files|format_project_context" crates README.md AGENTS.md \
  --glob '!crates/neo-agent/src/modes/interactive/tests.rs' || status=$?
test "$status" -eq 1
status=0
rg -n "AGENTS\.md / CLAUDE\.md|CLAUDE\.md as a compatibility fallback|CLAUDE\.md 作为兼容候选|startup-only loader remains|仍保留仅启动时加载器|project root.*auto-reads|project-root.*auto-reads" docs/en docs/zh || status=$?
test "$status" -eq 1
rg -n "max\(65_536, effective_max_tokens / 8\)|maximum recursive import depth.*5|最大递归导入深度.*5|PartiallyLoaded|partially loaded|部分加载" docs/en docs/zh AGENTS.md crates/neo-agent-core/src
```

Expected: the first two commands have zero stale source references or positive fallback/startup-only claims; the third finds the paired formula/limit/warning contract and implementation symbols. Documentation may explicitly state that the old fallback was removed.

- [ ] **Step 4: Verify paired documentation coverage manually.**

Compare the nine required topics in `docs/en/customization/agents.md` and `docs/zh/customization/agents.md`: baseline, nested scopes, imports, trust, failure, budget, cache/compaction, resume/multi-agent, and `/init`/no fallback. Record the matching headings in the task report. This is a documentation-only task; no Cargo test is required.

Expected: both locales describe one canonical behavior and repository `AGENTS.md` no longer says trust gates `CLAUDE.md`.

---

## Subagent-Driven Execution Protocol

The executing coordinator must use `aegis:subagent-driven-development` with these repository-specific adaptations:

1. Read this plan and the approved spec completely, then run the SDD pre-flight conflict scan before dispatching Task 1.
2. Maintain `docs/aegis/work/2026-07-17-path-scoped-agents-instructions/20-checkpoint.md`; trust the ledger and Git history after compaction. Never redispatch a completed task.
3. Generate each task brief with the SDD `scripts/task-brief` helper. A fresh implementer reads only the brief, spec path, required earlier interfaces, and a report-file path.
4. Explicitly select a model for every implementer and reviewer. Use the most capable model for Tasks 1, 6, 8, 9 and the final whole-branch review; use at least a standard model for all reviewers.
5. Dispatch parallel waves only as shown in the DAG. Never parallelize tasks with overlapping write sets.
6. Every subagent prompt must repeat: no Git mutation, no reverting worktree files, exact tests only, and report `DONE`, `DONE_WITH_CONCERNS`, `NEEDS_CONTEXT`, or `BLOCKED` into its report file.
7. The coordinator, not the implementer, waits for a complete wave, inspects all reports/diffs, stages only exact task files, and commits using the table above.
8. Generate a review package from the recorded task base through the task head. Give each task a spec-compliance review followed by code-quality review; fix and re-review every Critical/Important finding before marking it complete.
9. Resolve every reviewer `Cannot verify from diff` item using the spec and cross-task state. A real gap reopens the task.
10. Do not stop between tasks or waves unless genuinely blocked or a plan/spec conflict requires the user's decision.
11. After Task 10, run a final review package from branch merge-base to HEAD and dispatch the most capable whole-branch reviewer. One fixer receives the complete final findings list, runs focused covering tests, and is re-reviewed.
12. Use `aegis:verification-before-completion`, then `aegis:finishing-a-development-branch`. Do not push, merge, tag, switch branches, or remove worktrees without explicit user authorization.

## Final Acceptance Review

Before claiming completion, the coordinator maps each approved acceptance criterion to evidence:

```text
1 nested first-side-effect gate       -> Task 8 exact Edit/Write/cwd tests
2 full-batch defer + same-turn replan -> Tasks 8 and 9 event/request traces
3 imports + visible failures          -> Tasks 1, 4, 8
4 append-only request prefix          -> Task 6 adjacent-request assertion
5 exact compaction rehydration        -> Task 6 sentinel assertion
6 atomic budget omission + warning    -> Tasks 1, 4, 9
7 replay + agent-local visibility     -> Tasks 2, 3, 9
8 durable positioned metadata card   -> Tasks 4 and 7
9 old loader/fallback deleted         -> Tasks 5 and 10 residue scan
10 English/Chinese canonical docs     -> Task 10 paired-heading audit
```

The final report must state the exact commands run, their results, any skipped checks caused by unrelated worktree changes, and the final commit range. Broad `cargo test`, package-wide `nextest`, or CI status cannot substitute for the named boundary evidence above.
