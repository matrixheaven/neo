# Neo Skill Package Completion Implementation Plan

> Execution owner: use `aegis:executing-plans` for inline execution or
> `aegis:subagent-driven-development` when the active host permits independent
> agents. Maintain the existing long-task checkpoint throughout execution.

**Goal:** Implement the approved local skill-package contract: path-aware
activation, bounded fail-soft discovery, narrow Neo host metadata, typed
authoring, and retirement of manifest fields without real independent behavior.

**Architecture:** `SkillStore` remains the load/catalog owner and `LoadedSkill`
remains the immutable package snapshot. New `skills/metadata.rs` owns
`agents/neo.yaml`; new `skills/context.rs` owns the single activation envelope.
Discovery returns successful packages plus diagnostics. TUI and management
callers consume the snapshot without becoming alternate parsers or policy
owners.

**Tech Stack:** Rust 2024, serde/serde_yaml, standard-library filesystem APIs,
existing Neo tool/runtime/TUI abstractions, Cargo nextest, rustfmt, Clippy.

**Baseline/Authority Refs:**

- `docs/aegis/specs/2026-07-22-skill-package-completion-design.md`
- `docs/aegis/specs/2026-07-09-skill-resources-design.md`
- `docs/aegis/specs/2026-07-13-skill-invocation-transcript-design.md`
- `docs/aegis/specs/2026-07-20-append-only-skill-catalog-brief.md`
- `docs/aegis/adr/ADR-0001-aegis-dual-host-skill-discovery.md`
- `docs/aegis/baseline/2026-07-18-aegis-dual-host-install.md`
- repository `AGENTS.md`, `CX.md`, and `RTK.md`

**Compatibility Boundary:** Preserve `references/scripts/assets`,
`${NEO_SKILL_DIR}`, `/skill:<name>`, model `Skill` calls, arguments,
`whenToUse`, `disableModelInvocation`, user > extra > built-in precedence,
`$NEO_HOME/skills`, `extra_skill_dirs`, `skill_path`, symlinked Aegis views,
append-only catalog snapshots, transcript `SkillInvocation`, and replay.

**TDD Route:**

- Mode: `off`
- Decision: `skipped`
- Strict authority: `not applicable`
- Test posture: minimum implementation followed by focused post-change
  regression tests.
- Reason: neither the user nor project requires strict test-first TDD; the repo
  requires proportional exact tests.
- Verification: every command names one package, one target selector, and a
  test-name filter.

**Verification:** Run only the exact commands listed per task, then the final
format/lint/diff checks in Task 8. Broad `cargo test` or package-wide nextest is
not completion evidence.

## 1. Scope Check

### Plan Basis

The user approved the recommended P0/P1 direction and requested a complete
spec, plan, long-task boundary, committed documents, and detailed handoff. The
design is the requirement owner for implementation.

### Requirement Ready Check

- Requirement source: approved design spec above.
- Scenarios: filesystem package discovery, automatic activation, manual
  activation, TUI completion, package authoring, and malformed-package failure.
- Acceptance: design Section 16.
- Missing decisions: none.
- Decision: `ready`.

### Change Necessity

- User-visible need: imported/resource-backed skills must locate their own
  files and remain available beside malformed packages.
- No-change option: documentation-only guidance would preserve the current raw
  automatic body and fail-closed loader.
- Why code is necessary: those failures are owned by runtime behavior, not
  author education.
- Minimum boundary: core skill snapshot/discovery/context, current management
  tools, and existing completion caller.
- Decision: `code-change`.

### Existence Check

| Surface | Existing owner candidate | Decision |
| --- | --- | --- |
| `skills/metadata.rs` | `SkillManifest` | `add-with-proof`: host-only fields must not enter model policy/frontmatter. |
| `skills/context.rs` | automatic and manual caller renderers | `add-with-proof`: one envelope must serve both callers. |
| `SkillDiagnostic` | fatal `SkillLoadError` | `reuse-existing concept`: evolve load errors into retained diagnostics. |
| generic package/plugin framework | none | `reject`: no consumer or requirement. |

### Architecture Integrity Lens

- Invariant: one owner for model policy, host metadata, discovery, and context
  rendering.
- Canonical owners: `SKILL.md`, `agents/neo.yaml`, `SkillStore`, and shared
  context renderer respectively.
- Responsibility overlap to remove: manual markup renderer, raw automatic
  output, and `type` as duplicate manual-only policy.
- Higher-level simplification: fix the package snapshot/render contract once;
  do not add guards in every caller.
- Retirement/falsifier: any required consumer that still needs `SkillType` or
  `slashCommands` invalidates deletion and must be proven before code is kept.
- Verdict: `proceed`.

### Plan Pressure Test

- Owner/contract/retirement: explicit in Tasks 1 through 6.
- Higher-level path: shared snapshot and renderer, no caller fallback.
- Verification: exact owner tests plus one manual integration boundary.
- Executability: file paths, code shapes, test names, commands, and commits are
  named below.
- Pressure result: `proceed`.

## 2. Execution Readiness View

- Intent Lock: complete the approved local package runtime, not generic plugin
  or remote skill infrastructure.
- Scope Fence: only files named in the file map unless a direct compile error
  proves one additional caller; record that caller before editing it.
- Baseline Lock: resource, catalog, invocation transcript, and dual-host
  discovery records above remain authoritative constraints.
- Approved Behavior: design Sections 6 through 14.
- Owner/Contract Constraints: `SKILL.md` owns model behavior;
  `agents/neo.yaml` owns host display/dependency metadata; `SkillStore` owns
  load/catalog; one core renderer owns activation context.
- Compatibility Boundary: header contract above.
- Retirement Boundary: delete `SkillType`, manifest `type`,
  `CreateSkill.skill_type`, `slashCommands`, the manual-only renderer, raw
  automatic skill body output, and fail-closed recursion. Add no fallback.
- Task Batches: manifest retirement; metadata; discovery; context; consumers
  and authoring; docs; verification/ADR review.
- Test Obligations: each acceptance row in design Section 16 has an exact test
  or explicit lingering-reference check.
- Review Gates: after Tasks 3, 4, and 6 inspect `git diff`; after Task 8 request
  an independent code review and run Aegis completion verification.
- Drift/Rewind Rules: a new project skill root, compatibility parser,
  marketplace/provider, automatic dependency installer, dynamic selector,
  catalog omission, transcript redesign, or permission change stops execution
  and returns to the spec.
- Evidence Required Before Completion: exact test output, format/lint status,
  lingering-reference result, docs parity, independent review disposition, and
  scoped commit log.
- Advisory Boundary: method-pack execution guidance only; not `GateDecision`,
  `PolicySnapshot`, or completion authority.

## 3. File Map

| File | Planned role |
| --- | --- |
| `crates/neo-agent-core/src/skills/mod.rs` | Canonical manifest and package snapshot; module wiring; catalog rendering. |
| `crates/neo-agent-core/src/skills/metadata.rs` | New sidecar model, validation, parse, and serialization owner. |
| `crates/neo-agent-core/src/skills/context.rs` | New shared activation envelope renderer. |
| `crates/neo-agent-core/src/skills/discovery.rs` | Bounded traversal, symlink-cycle handling, per-skill diagnostics. |
| `crates/neo-agent-core/src/skills/arguments.rs` | Preserve argument and `${NEO_SKILL_DIR}` expansion. |
| `crates/neo-agent-core/src/runtime/skill_dispatch.rs` | Automatic invocation calls shared renderer. |
| `crates/neo-agent-core/src/tools/skills_manager.rs` | List/Create wiring and typed host metadata input; no parser logic. |
| `crates/neo-agent-core/tests/skills.rs` | Package, metadata, discovery, catalog regression coverage. |
| `crates/neo-agent/src/resources.rs` | Load diagnostics reporting only if required by the compiled owner boundary. |
| `crates/neo-agent/src/modes/interactive/slash_commands.rs` | Manual invocation calls shared renderer; delete local markup renderer. |
| `crates/neo-agent/src/modes/interactive/prompt_completion.rs` | Host display metadata with canonical insertion. |
| `crates/neo-agent/src/modes/interactive/tests.rs` | One manual-context and one completion integration check. |
| `crates/neo-agent-core/src/skills/builtin/mod.rs` | One consolidated regression test for both shipped authoring contracts. |
| `crates/neo-agent-core/src/skills/builtin/create-skill.md` | Requirement-driven single-skill authoring and verification contract. |
| `crates/neo-agent-core/src/skills/builtin/self-evo.md` | Evidence-driven, zero-or-more sequential skill distillation contract. |
| `docs/en/customization/skills.md` | English package/manifest/migration docs. |
| `docs/zh/customization/skills.md` | Chinese package/manifest/migration docs. |

Do not edit transcript card/render files, session persistence, instruction
epochs, MCP connection management, config discovery roots, or multi-agent code.

## 4. Complexity Budget

- Artifact class: Source Complexity.
- Pressure: `skills_manager.rs` is over 2,500 lines;
  `interactive/tests.rs` is also large.
- Governance: add only argument/wiring code to `skills_manager.rs`; put sidecar
  logic in `metadata.rs`. Consolidate existing built-in prompt assertions into
  one contract test rather than adding parallel substring tests.
- Recommendation: new cohesive owner files for metadata/context; edit discovery
  in place; no unrelated extraction.
- Budget result: `at-risk but governed`.

## Task 0: Resume-Safe Start and Dirty-Tree Fence

**Files:**

- Read `docs/aegis/work/2026-07-22-skill-package-completion/10-intent.md`
- Read `docs/aegis/work/2026-07-22-skill-package-completion/20-checkpoint.md`
- Read the approved spec and this plan

**Why:** The implementation will span several commits and may resume after
context resets. Work must not drift into unrelated dirty files.

**Change Necessity:** No source change in this task. It establishes the
execution boundary required by the long-task protocol.

**Impact/Compatibility:** None.

**Steps:**

1. Run `rtk icm recall-context "Neo skill package completion execution" --limit 5`.
2. Run `rtk git status --short` and record pre-existing unrelated paths in the
   checkpoint. At this amendment's closeout this included `.gitignore`; treat
   the live status as authoritative because the shared worktree may change
   after handoff.
3. Read only the baseline and owner files named above. Use CodeGraph/cx before
   broader source search. Do not re-explore `.references/codex` unless a spec
   statement cannot be mapped to Neo code.
4. Update the Aegis checkpoint: current task Task 1, no blockers, next exact
   command.
5. Do not commit in this task.

## Task 1: Retire Inactive Manifest Surfaces

**Files:**

- Modify `crates/neo-agent-core/src/skills/mod.rs`
- Modify `crates/neo-agent-core/src/tools/skills_manager.rs`
- Modify affected constructors/tests in the two files and
  `crates/neo-agent-core/tests/skills.rs`

**Why:** `type` and `slashCommands` currently claim semantics Neo does not own.
Keeping them would create duplicate manual-only and invocation-policy paths.

**Change Necessity:** Source edits are required to make the runtime schema and
tool schema match the approved manifest contract. Minimum boundary is the
manifest, CreateSkill serialization, and direct tests.

**Repair Track:** Keep `name`, `description`, `when_to_use`,
`disable_model_invocation`, and `arguments`. Preserve `auto_invokable()` as
`!disable_model_invocation`.

**Retirement Track:** Delete `SkillType`, `skill_type`, `type` serialization,
`slash_commands`, parsing helpers, enum matches, and test fixtures. Do not map
old values to new values at runtime.

**Target shape:**

```rust
pub struct SkillManifest {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub disable_model_invocation: bool,
    pub arguments: Vec<SkillArgument>,
}

impl SkillManifest {
    #[must_use]
    pub const fn auto_invokable(&self) -> bool {
        !self.disable_model_invocation
    }
}
```

`CreateSkillArgs` must contain `name`, `description`, `body`, optional
`host_metadata`, and `resources`; `host_metadata` is added in Task 6, so keep
the compile boundary explicit rather than adding a temporary compatibility
field.

**Verification:**

1. Add/update one integration test named
   `loads_canonical_manifest_without_retired_execution_types`.
2. Run:

   ```bash
   rtk cargo nextest run -p neo-agent-core --test skills loads_canonical_manifest_without_retired_execution_types
   ```

3. Run the retirement search:

   ```bash
   rtk rg -n "SkillType|skill_type|slash_commands|slashCommands" crates/neo-agent-core/src crates/neo-agent/src docs/en/customization/skills.md docs/zh/customization/skills.md
   ```

   At this stage the two built-in authoring files and user docs may still carry
   the old authoring examples scheduled for Tasks 6 and 7; production Rust
   matches must be zero.

4. Commit only this slice:

   ```bash
   rtk git add crates/neo-agent-core/src/skills/mod.rs crates/neo-agent-core/src/tools/skills_manager.rs crates/neo-agent-core/tests/skills.rs
   rtk git commit -m "refactor(skills): remove inactive manifest fields"
   ```

## Task 2: Add the Neo Host-Metadata Owner

**Files:**

- Create `crates/neo-agent-core/src/skills/metadata.rs`
- Modify `crates/neo-agent-core/src/skills/mod.rs`
- Modify `crates/neo-agent-core/tests/skills.rs`

**Why:** Model selection prose and human-facing picker prose have different
consumers. A sidecar prevents one field from serving both and gives declared
MCP dependencies a local package home.

**Change Necessity:** No existing owner can hold host-only fields without
polluting model-facing `SKILL.md`. The new file is the minimum cohesive owner.

**Impact/Compatibility:** Sidecar absence preserves manifest fallbacks.
Malformed optional metadata never hides a valid skill.

**Implement these public package types:**

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillHostMetadata {
    pub interface: Option<SkillInterface>,
    pub dependencies: Vec<SkillToolDependency>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInterface {
    pub display_name: Option<String>,
    pub short_description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillToolDependency {
    pub value: String,
    pub description: Option<String>,
}
```

Only MCP dependencies survive validation, so the runtime type does not need a
one-variant enum. The YAML input struct may deserialize its `type` string and
reject values other than `mcp` with a diagnostic.

Add `host_metadata: SkillHostMetadata` to `LoadedSkill`. Add methods that return
display-name and short-description fallbacks without duplicating that logic in
TUI callers.

Implement `load_host_metadata(skill_root: &Path)` and
`serialize_host_metadata(...)` in `metadata.rs`. The loader returns metadata
plus diagnostics; missing file returns empty metadata and no diagnostic.

**Verification:**

1. Add `neo_host_metadata_loads_and_invalid_optional_metadata_falls_back` to
   `crates/neo-agent-core/tests/skills.rs`. One test should cover both the valid
   consumer fields and malformed-sidecar fail-open behavior because they are
   the same contract boundary.
2. Run:

   ```bash
   rtk cargo nextest run -p neo-agent-core --test skills neo_host_metadata_loads_and_invalid_optional_metadata_falls_back
   ```

3. Commit:

   ```bash
   rtk git add crates/neo-agent-core/src/skills/metadata.rs crates/neo-agent-core/src/skills/mod.rs crates/neo-agent-core/tests/skills.rs
   rtk git commit -m "feat(skills): load Neo host metadata"
   ```

## Task 3: Make Discovery Bounded and Fail-Soft

**Files:**

- Modify `crates/neo-agent-core/src/skills/discovery.rs`
- Modify `crates/neo-agent-core/src/skills/mod.rs`
- Modify `crates/neo-agent-core/tests/skills.rs`
- Modify direct `SkillStore::load` callers only as required by the new outcome

**Why:** One malformed package must not disable unrelated skills, and symlinked
skill trees must not permit recursion cycles or unbounded scans.

**Change Necessity:** Documentation cannot repair an unbounded fail-closed
filesystem traversal. The correct owner is discovery, not each caller.

**Repair Track:** Return successful skills and structured diagnostics. Preserve
sorted traversal, tier precedence, nested `skills/`, and discovered symlink
paths.

**Retirement Track:** Delete recursive `?` propagation that aborts the entire
root/store. Do not add a caller retry or fallback scanner.

**Implementation contract:**

- constants: depth 6, directories 2,000, entries 20,000;
- use an iterative stack or bounded recursive helper with explicit counters;
- canonicalize directories only for the visited set;
- preserve the discovered absolute path in `LoadedSkill.root`;
- skip `agents`, `references`, `scripts`, and `assets` only below a directory
  containing `SKILL.md`;
- continue after malformed required files and collect a diagnostic;
- continue with manifest fallbacks after optional metadata diagnostics;
- expose diagnostics through `SkillStore::diagnostics()`;
- `ListSkills` later renders concise diagnostics; the model catalog never does.

**Verification:**

1. Add one table-driven integration test named
   `discovery_is_bounded_cycle_safe_and_keeps_valid_siblings`. It must cover a
   malformed sibling and, under the platform-appropriate symlink/reparse cfg, a
   cycle. Keep assertions on Neo behavior rather than stdlib behavior.
2. Run:

   ```bash
   rtk cargo nextest run -p neo-agent-core --test skills discovery_is_bounded_cycle_safe_and_keeps_valid_siblings
   ```

3. Run the existing Aegis symlink-view discovery test identified through
   CodeGraph/cx with its exact final name. Do not substitute a broad filter.
4. Inspect `git diff` to confirm no root source or tier precedence changed.
5. Commit:

   ```bash
   rtk git add crates/neo-agent-core/src/skills/discovery.rs crates/neo-agent-core/src/skills/mod.rs crates/neo-agent-core/tests/skills.rs crates/neo-agent/src/resources.rs
   rtk git commit -m "fix(skills): isolate and bound package discovery"
   ```

`crates/neo-agent/src/resources.rs` is the only planned external load caller;
the staging command is harmless when it is unchanged. If compilation identifies
another direct caller, record it in the checkpoint before editing and add its
exact path to this command. Never use `git add -A`.

## Task 4: Unify Path-Aware Activation Context

**Files:**

- Create `crates/neo-agent-core/src/skills/context.rs`
- Modify `crates/neo-agent-core/src/skills/mod.rs`
- Modify `crates/neo-agent-core/src/runtime/skill_dispatch.rs`
- Modify `crates/neo-agent/src/modes/interactive/slash_commands.rs`
- Modify focused tests in the same modules and
  `crates/neo-agent/src/modes/interactive/tests.rs`

**Why:** Automatic calls currently return raw instructions while manual calls
use a path-bearing private envelope. Resource resolution must not depend on
which activation route was used.

**Change Necessity:** One core renderer is the smallest stable fix for both
callers. Caller-side path preambles would duplicate the contract.

**Repair Track:** Expand arguments first, then render one envelope with escaped
name/source/root, optional dependency block, and instructions.

**Retirement Track:** Delete `render_loaded_skill_block` from interactive slash
handling and stop returning a raw body from `execute_invoke_skill`.

**Core API shape:**

```rust
pub fn render_skill_context(skill: &LoadedSkill, instructions: &str) -> String;
```

The API derives canonical name, source, root, and dependencies from the
snapshot. Do not pass duplicate caller-owned fields.

Update `available_skills_prompt()` usage prose to state the relative-resource
resolution and progressive-disclosure rules. Keep deterministic ordering and
the complete replacement snapshot marker unchanged.

**Verification:**

1. Update/add core test
   `execute_invoke_skill_returns_path_aware_context`.
2. Run:

   ```bash
   rtk cargo nextest run -p neo-agent-core --lib execute_invoke_skill_returns_path_aware_context
   ```

3. Add binary test
   `modes::interactive::tests::manual_skill_context_uses_shared_path_aware_envelope`.
4. Run:

   ```bash
   rtk cargo test --package neo-agent --bin neo -- modes::interactive::tests::manual_skill_context_uses_shared_path_aware_envelope --exact --nocapture --include-ignored
   ```

5. Re-run the existing exact SkillInvocation transcript test whose behavior
   covers one semantic card and no generic `ToolRun`; locate its full path with
   `cx symbols` rather than using a broad filter.
6. Inspect `git diff` to confirm no transcript renderer/card file changed.
7. Commit:

   ```bash
   rtk git add crates/neo-agent-core/src/skills/context.rs crates/neo-agent-core/src/skills/mod.rs crates/neo-agent-core/src/runtime/skill_dispatch.rs crates/neo-agent/src/modes/interactive/slash_commands.rs crates/neo-agent/src/modes/interactive/tests.rs
   rtk git commit -m "feat(skills): provide path-aware activation context"
   ```

## Task 5: Consume Host Metadata Without Changing the Model Catalog

**Files:**

- Modify `crates/neo-agent-core/src/skills/mod.rs`
- Modify `crates/neo-agent-core/src/tools/skills_manager.rs`
- Modify `crates/neo-agent/src/modes/interactive/prompt_completion.rs`
- Modify focused tests in those owners

**Why:** A sidecar is justified only if current product surfaces consume it.
Human display metadata must not alter model selection or canonical insertion.

**Change Necessity:** Existing picker/list code directly reads the manifest
description. Small wiring changes route it through snapshot fallback methods.

**Impact/Compatibility:** Canonical names remain in `/skill:*`, transcripts,
lookups, and model catalog. Sidecar changes only human-facing text and declared
dependency summaries.

**Steps:**

1. Make completion labels use `skill.display_name()` while insertion remains
   `format!("/skill:{}", skill.name)`.
2. Make completion descriptions use `skill.short_description()` with manifest
   fallback.
3. Keep `write_available_skill` on canonical name, manifest description, and
   `whenToUse`; add no display metadata.
4. Extend `ListSkills` lines with optional display and dependency summaries,
   then append concise diagnostics after tier sections.
5. Add `completion_uses_host_label_but_inserts_canonical_skill_name`.
6. Run:

   ```bash
   rtk cargo test --package neo-agent --bin neo -- modes::interactive::tests::completion_uses_host_label_but_inserts_canonical_skill_name --exact --nocapture --include-ignored
   ```

7. Add/update core catalog test
   `host_metadata_does_not_change_model_visible_catalog` and run:

   ```bash
   rtk cargo nextest run -p neo-agent-core --test skills host_metadata_does_not_change_model_visible_catalog
   ```

8. Commit:

   ```bash
   rtk git add crates/neo-agent-core/src/skills/mod.rs crates/neo-agent-core/src/tools/skills_manager.rs crates/neo-agent/src/modes/interactive/prompt_completion.rs crates/neo-agent/src/modes/interactive/tests.rs crates/neo-agent-core/tests/skills.rs
   rtk git commit -m "feat(skills): surface Neo package metadata"
   ```

## Task 6: Add Typed Sidecar Authoring and Upgrade Both Built-in Authors

**Files:**

- Modify `crates/neo-agent-core/src/tools/skills_manager.rs`
- Modify `crates/neo-agent-core/src/skills/metadata.rs`
- Modify `crates/neo-agent-core/src/skills/builtin/mod.rs`
- Modify `crates/neo-agent-core/src/skills/builtin/create-skill.md`
- Modify `crates/neo-agent-core/src/skills/builtin/self-evo.md`

**Why:** A first-class package field that only hand editing can create leaves
Neo's built-in authoring workflow incomplete. Both shipped authors currently
teach retired `type` / `skill_type` fields, so updating only the Rust tool would
immediately regenerate obsolete packages.

**Change Necessity:** `CreateSkill` already owns safe package writes and whole
package backup. Reuse it; do not create another metadata-writing tool.

**Impact/Compatibility:** Calls without `host_metadata` preserve an existing
sidecar and behave as before. A supplied object is a complete atomic sidecar
replacement. Existing resource writes and backups remain unchanged.

**Input types:**

```rust
pub struct CreateSkillHostMetadata {
    pub interface: Option<CreateSkillInterface>,
    #[serde(default)]
    pub dependencies: Vec<CreateSkillDependency>,
}
```

Use typed nested structs for display name, short description, MCP value, and
optional dependency description. Reject an empty supplied object. Validate all
metadata before any backup or write. Write `agents/neo.yaml` atomically through
the same safe-directory/reparse policy used for package resources.

**Built-in author boundaries:**

- `create-skill` is requirement-driven and creates one focused package. It asks
  for a concrete capability when invoked without one, selects resources and
  host metadata only for real consumers, calls `CreateSkill` once, then
  verifies through `ListSkills` and a representative behavior check.
- `self-evo` is evidence-driven and requires an explicit history scope. It may
  create zero skills, rejects session narrative/project-only facts/secrets,
  deduplicates against `ListSkills`, and creates/verifies candidates one at a
  time. A failure stops the next candidate.
- both remain `disableModelInvocation: true`, remove their own `type: prompt`,
  call `CreateSkill` without `skill_type`, and never write files directly;
- both omit `host_metadata` when it would only restate name/description. They
  declare an MCP dependency only when the body requires an existing configured
  server identifier, without inventing connection or installer data.

**Verification:**

1. Before editing either built-in skill, use `aegis:writing-skills` to run and
   record one baseline behavior scenario per author:
   - `create-skill`: request a resource-backed skill with a distinct display
     label and a known MCP dependency; record any retired fields, phantom
     resources, fabricated metadata, direct writes, or missing verification.
   - `self-evo`: provide a noisy scope with one strong repeatable workflow and
     one weak candidate; record any narrative copying, vague/batch creation,
     retired fields, missing deduplication, or skipped per-skill verification.
2. Add one manager test
   `create_skill_writes_and_preserves_typed_host_metadata` covering initial
   creation, omitted-on-update preservation, and active-store reload.
3. Update both built-in files to the approved shared and author-specific
   contracts. Preserve their distinct input sources; do not merge them or add a
   shared authoring abstraction.
4. In `skills/builtin/mod.rs`, consolidate the existing author prompt checks
   into `builtin_skill_authors_use_canonical_package_contract`. Inspect the raw
   built-in frontmatter and loaded bodies for both authors. Assert common
   canonical fields/manual-only behavior and the distinct requirement-driven
   versus evidence-driven rules. Replace
   `builtin_skills_include_create_skill`,
   `self_evo_builtin_requires_scope_and_verify_section`, and
   `create_skill_builtin_requires_verify_and_create_skill_tool` rather than
   retaining parallel substring-only tests. Update the stale extraction fixture
   to use canonical frontmatter while preserving its refresh assertion.
5. Run the tool behavior test:

   ```bash
   rtk cargo nextest run -p neo-agent-core --lib create_skill_writes_and_preserves_typed_host_metadata
   ```

6. Run the built-in author contract test:

   ```bash
   rtk cargo nextest run -p neo-agent-core --lib builtin_skill_authors_use_canonical_package_contract
   ```

7. Re-run one existing exact symlink/reparse rejection test from
   `skills_manager` against the new `agents` path behavior where platform
   applicable.
8. Re-run the two baseline scenarios with fresh agents and the revised built-in
   skill loaded. Record whether each author now emits canonical `CreateSkill`
   input, observes its stop conditions, and performs post-create verification.
   Do not claim prompt completion from Rust substring assertions alone.
9. Inspect `skills_manager.rs`: sidecar parse/serialization logic must remain in
   `metadata.rs`; this file gets input and safe-write orchestration only.
10. Run the final authoring retirement search:

   ```bash
   rtk rg -n "SkillType|skill_type|slash_commands|slashCommands|type: (prompt|inline|flow)" crates/neo-agent-core/src/skills/builtin crates/neo-agent-core/src/tools/skills_manager.rs
   ```

   It must return no matches.
11. Commit:

   ```bash
   rtk git add crates/neo-agent-core/src/tools/skills_manager.rs crates/neo-agent-core/src/skills/metadata.rs crates/neo-agent-core/src/skills/builtin/mod.rs crates/neo-agent-core/src/skills/builtin/create-skill.md crates/neo-agent-core/src/skills/builtin/self-evo.md
   rtk git commit -m "feat(skills): author Neo package metadata"
   ```

## Task 7: Update User Documentation and Migration

**Files:**

- Modify `docs/en/customization/skills.md`
- Modify `docs/zh/customization/skills.md`

**Why:** Published skill authors need one accurate package layout and migration
path. Existing docs currently overstate `inline`, `flow`, and slash metadata.

**Change Necessity:** Documentation is the external contract for hand-authored
skill files and must change with the parser/tool schema.

**Impact/Compatibility:** Explain the breaking manifest cleanup without adding
a runtime compatibility path.

**Steps:**

1. Document the canonical package tree including `agents/neo.yaml`.
2. Document the exact sidecar schema and concrete consumers.
3. State that `SKILL.md` remains the only automatic context entry point.
4. Replace the old `${NEO_SKILL_DIR}` requirement with the root-relative rule
   while retaining the placeholder as supported explicit syntax.
5. Remove `type`, `skill_type`, and `slashCommands` from current examples and
   field tables.
6. Add migration mapping: prompt/inline remove type; flow add
   `disableModelInvocation: true`; aliases use canonical `/skill:<name>`.
7. Keep binary assets, icons, automatic dependencies, project-local roots, and
   remote skill sources explicitly unsupported.
8. Compare English/Chinese heading and example coverage manually.
9. Run:

   ```bash
   rtk git diff --check -- docs/en/customization/skills.md docs/zh/customization/skills.md
   ```

10. Commit:

   ```bash
   rtk git add docs/en/customization/skills.md docs/zh/customization/skills.md
   rtk git commit -m "docs(skills): document completed package contract"
   ```

## Task 8: Focused Closure, Review, and ADR Backfill Check

**Files:** All files touched by Tasks 1 through 7; Aegis checkpoint/evidence;
an ADR only if `aegis:recording-architecture-decisions` confirms the signal
after implementation evidence exists.

**Why:** This cross-module contract is not complete until old owners are gone,
targeted behavior is verified, unrelated dirty files are excluded, and durable
architecture recording is assessed from actual implementation.

**Change Necessity:** Verification and architecture recording are required by
the approved spec; no new product code should appear here.

**Steps:**

1. Run each exact test from Tasks 1 through 6 once more only when its owning
   files changed after the first green result. Do not run package-wide nextest.
2. Run:

   ```bash
   rtk cargo fmt --all --check
   rtk cargo clippy -p neo-agent-core --lib -- -D clippy::all
   rtk cargo clippy -p neo-agent --bin neo -- -D clippy::all
   ```

3. Run lingering-reference checks:

   ```bash
   rtk rg -n "SkillType|skill_type|slash_commands|slashCommands" crates/neo-agent-core/src crates/neo-agent/src
   rtk rg -n "type: (prompt|inline|flow)" crates/neo-agent-core/src/skills/builtin
   rtk rg -n "render_loaded_skill_block" crates/neo-agent-core/src crates/neo-agent/src
   ```

   All three commands must return no production or shipped built-in matches.
4. Run scoped whitespace validation by listing every touched path explicitly:

   ```bash
   rtk git diff --check -- crates/neo-agent-core/src/skills crates/neo-agent-core/src/runtime/skill_dispatch.rs crates/neo-agent-core/src/tools/skills_manager.rs crates/neo-agent-core/tests/skills.rs crates/neo-agent/src/modes/interactive docs/en/customization/skills.md docs/zh/customization/skills.md
   ```

5. Use `aegis:requesting-code-review` for an independent defect-first review
   focused on discovery limits/symlinks, model-vs-host metadata ownership,
   context injection, retired paths, and test adequacy. Fix only verified
   in-scope findings and rerun their exact owner tests.
6. Use `aegis:verification-before-completion`. Update checkpoint, evidence,
   drift, and proof bundle. Decision must remain `needs-verification` until all
   acceptance rows have evidence.
7. Invoke `aegis:recording-architecture-decisions` for the design's ADR signal.
   Create or amend only the record it selects; do not invent an ADR before this
   evidence review.
8. Run the Aegis workspace helper `check` and `bundle` for
   `2026-07-22-skill-package-completion`.
9. Run `rtk git status --short` and verify unrelated pre-existing files were
   never staged or committed.
10. If review creates or amends an ADR/baseline record, run
    `rtk git status --short docs/aegis/adr docs/aegis/baseline`, verify every
    returned path belongs to this task, stage those exact paths individually,
    and run `rtk git commit -m "docs(skills): record package architecture"`.
    Skip this commit when no record was created or amended.

11. Do not push. Report commit hashes, exact verification evidence, skipped
   checks, residual risk, and the unchanged unrelated dirty paths.

## 5. Drift and Rewind Rules

Stop implementation and return to the spec when any of these occurs:

- project-local or `.agents/skills` discovery appears necessary;
- a caller asks to parse `agents/openai.yaml` as fallback;
- a field has no concrete current consumer;
- MCP dependency handling begins mutating config or network state;
- discovery needs a second scanner or retry path;
- the catalog starts ranking, truncating, or omitting skills;
- transcript cards or session history must change;
- sidecar and manifest both claim invocation policy;
- tests require broad workspace execution to establish owner behavior;
- unrelated dirty files block a targeted compile or test.

For a genuine new requirement, record `scope-exceeded`, preserve current
evidence, and request a design amendment. Do not improvise a compatibility
branch.

## 6. Retirement Verification

Anti-Entropy Declaration:

- Deletion class: contract-carrying internal code.
- Old paths: `SkillType`, `type`, `skill_type`, `slashCommands`, manual context
  renderer, raw auto result, fail-closed recursion.
- New canonical owners: `disableModelInvocation`, shared context renderer, and
  bounded discovery outcome.
- Preserved behavior: compatibility header above.
- Retired behavior: nominal type/alias metadata and whole-store failure.
- External boundary touched: yes, documented hand-authored manifest.
- Source-of-truth data risk: none; no user files are mutated.
- Confirmation required: no.

Retirement decision: `delete-first`. The external migration is documentation,
not a retained parser branch.

Verification plan:

- main path: exact automatic and manual activation tests;
- lingering reference: Task 8 searches;
- negative: canonical authoring output contains no retired fields;
- boundary: existing root, resource, Aegis symlink, catalog, and transcript
  tests remain green.

## 7. Handoff Prompt

Use the following prompt verbatim when handing execution to another AI:

```text
You are implementing the approved Neo Skill Package Completion long-running task in
/Users/chenyuanhao/Workspace/neo.

AUTHORITATIVE INPUTS, IN ORDER:
1. Repository AGENTS.md, CX.md, and RTK.md.
2. docs/aegis/specs/2026-07-22-skill-package-completion-design.md
3. docs/aegis/plans/2026-07-22-skill-package-completion.md
4. docs/aegis/work/2026-07-22-skill-package-completion/10-intent.md
5. docs/aegis/work/2026-07-22-skill-package-completion/20-checkpoint.md
6. The baseline refs listed in the plan header.

The design direction is approved. Do not restart product discovery, repeat a full-repo
survey, or redesign the feature from scratch. Use CodeGraph/cx before grep or broad file
reads. Read only the owner files named by the active task plus direct callers proven by
CodeGraph or compiler errors.

INTENT LOCK:
- Complete Neo's local filesystem skill package runtime.
- Make automatic and manual skill activation use one path-aware context envelope.
- Make discovery deterministic, bounded, symlink-cycle safe, and fail-soft per skill.
- Add a narrow agents/neo.yaml for current Neo host consumers.
- Make create-skill and self-evo emit and verify that canonical package shape while
  preserving their distinct requirement-driven and evidence-driven roles.
- Retire manifest surfaces that do not have independent runtime semantics.

CANONICAL OWNERS:
- SKILL.md: canonical name, model-facing description/whenToUse, invocation policy via
  disableModelInvocation, and arguments.
- agents/neo.yaml: display_name, short_description, and declared MCP dependencies only.
- SkillStore/discovery: package loading, precedence, limits, and diagnostics.
- skills/context.rs: the only model-visible activated skill envelope.
- Existing append-only catalog owner: complete deterministic snapshots; no selector.
- create-skill: one requirement-driven package through CreateSkill, then verification.
- self-evo: explicit-scope evidence distillation; zero is valid; candidates are written
  and verified sequentially through CreateSkill.

MANDATORY PRESERVATION:
- SKILL.md remains the only package file automatically injected.
- references/, scripts/, assets/, ${NEO_SKILL_DIR}, /skill:<name>, Skill tool,
  arguments, whenToUse, disableModelInvocation, user > extra > builtin precedence,
  $NEO_HOME/skills as the sole implicit user root, extra_skill_dirs, skill_path,
  symlinked Aegis views, SkillInvocation transcript events, and session replay.
- Preserve discovered symlink-view paths for resource resolution; canonical paths are
  only for cycle detection.
- Skill catalogs remain complete. Never auto-truncate, auto-omit, rank, or dynamically
  select them based on token/cost predictions.

MANDATORY RETIREMENT, NO FALLBACKS:
- Delete SkillType, manifest type, CreateSkill.skill_type, and slashCommands.
- Replace type: flow guidance with disableModelInvocation: true in docs; do not retain a
  runtime compatibility parser/branch.
- Delete the manual-only context renderer and raw automatic Skill body output after the
  shared renderer owns both paths.
- Delete fail-closed recursive discovery; do not add caller retries or a second scanner.
- Never parse agents/openai.yaml as a fallback and never put invocation policy in both
  SKILL.md and agents/neo.yaml.
- Both built-in author files must remove type: prompt and every skill_type instruction;
  do not leave shipped prompts capable of regenerating retired fields.

HARD NON-GOALS:
- No project-local/.agents skill roots, marketplace, hosted sync, plugin runtime,
  remote/orchestrator provider, dynamic selector, silent catalog truncation, icon/brand
  UI, default prompt, binary CreateSkill payload, or automatic MCP install/enable/auth.
- Do not change transcript card design, session persistence, permission modes,
  instruction epochs, shell behavior, MCP connection ownership, or multi-agent code.

EXECUTION PROTOCOL:
1. Run rtk icm recall-context before work and obey mandatory icm store triggers.
2. Read the latest long-task checkpoint; never resume from memory alone.
3. Execute plan Tasks 0 through 8 in order. Update checkpoint/evidence/drift after each
   slice. A new owner, fallback, compatibility path, or non-goal is a stop-and-return-to-
   spec condition.
4. Use the exact narrow tests in each task. Do not use broad cargo test or package-wide
   nextest as evidence. Do not add redundant cosmetic tests.
5. Commit each verified logical slice with the specified conventional message. Stage
   explicit paths only. Never use git add -A, stash, reset, checkout/restore, clean,
   rebase, amend, force push, or any worktree-discarding command. Do not push.
6. The worktree is shared and dirty. At this amendment's closeout an unrelated change
   existed in .gitignore. This snapshot is not exhaustive: re-read live status and do not
   touch, stage, revert, or include unrelated changes. If they break broad checks, skip
   those checks and report it; never revert them.
7. Keep skills_manager.rs wiring-only for the new feature. Put sidecar logic in
   skills/metadata.rs and activation rendering in skills/context.rs.
8. Task 6 is incomplete until create-skill and self-evo each have a recorded baseline
   behavior scenario, revised contract, exact Rust contract test, and fresh-agent
   post-change scenario. Substring tests alone are not skill-behavior evidence.
9. Before completion, request an independent defect-first review, run Aegis verification-
   before-completion, perform the ADR backfill check, run lingering-reference searches,
   and report exact evidence plus residual risk.

START NOW with Task 0. Your first update should state the active slice, intended edits,
explicit non-edits, and exact verification command. Do not ask the user to reapprove the
already-approved design unless a listed drift/rewind condition actually occurs.
```

## 8. Plan Self-Review

- Spec coverage: every acceptance row maps to Tasks 1 through 8, including
  separate `create-skill` and `self-evo` authoring evidence in Task 6.
- Placeholder scan: no unresolved implementation placeholders; staging commands
  explicitly tell the executor to resolve and enumerate direct paths.
- Type consistency: manifest, sidecar, snapshot, and renderer ownership are
  consistent across tasks.
- Compatibility: preserved and retired behavior is explicit.
- Change necessity: every source-edit task names why docs/config cannot solve it.
- Existence: two new source files have concrete single responsibilities and
  consumers; generic abstractions were rejected.
- Complexity: oversized owners receive wiring/tests only; cohesive behavior is
  extracted by responsibility.
- Architecture integrity: no policy or renderer has two owners.
- Verification: commands are package/target/filter scoped.
- ADR/baseline sync: deferred until implementation evidence exists and carried
  into Task 8.
