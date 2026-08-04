# Neo Theme Manager and AI-Assisted Custom Theme Implementation Plan

Date: `2026-08-04`

Status: `ready for implementation after approved spec review`

Spec: [`docs/aegis/specs/2026-08-04-theme-manager-design.md`](../specs/2026-08-04-theme-manager-design.md)

## Implementer Directive

Execute this plan against the approved design. Do not reopen the product decisions
about a full manager, two-column layout, temporary application versus explicit
persistence, import-and-archive behavior, busy-turn handling, the separate
`custom-theme` skill, or the `ThemeDraft` handoff.

The shared worktree may contain unrelated user changes, including the untracked
`CONTEXT.md`. Preserve them. Work only in the files listed by each task, plus
focused test files and the documentation files explicitly named below. Do not
introduce a second durable theme store, a second `TuiTheme`, a project-local
`.neo/themes` location, a generic `$NEO_HOME` writer, or an AI auto-apply path.

This document plans implementation only. It does not authorize deleting user
managed theme files, changing live configuration, switching branches, or starting
implementation in the current planning request.

## Architecture

The implementation keeps one canonical owner per concern:

- `crates/neo-agent/src/themes.rs` owns the logical `ThemeId`, catalog, strict
  parser/materializer, managed-directory safety, and atomic theme mutations.
- `[tui].theme` owns the persisted startup selection; `AppConfig` exposes the
  resolved startup value without becoming a second file store.
- `neo_tui::primitive::TuiTheme` and `NeoChromeState` remain the only current
  render-color owner and propagation path.
- `neo-tui` owns transient `ThemeManagerState` and pure preview rendering. It
  receives snapshots and emits actions; it never reads the filesystem or config.
- `neo-agent` owns the controller side effects and the repository-backed,
  bounded `ThemeDraft` adapter. `neo-agent-core` supplies only generic Tool and
  permission contracts; it does not depend on `neo-agent`.
- The existing append-only tool/session event pipeline remains the event boundary;
  theme state is never placed in prompt context or session metadata.

The main flow is:

```text
$NEO_HOME/themes/
    -> neo-agent theme repository/catalog
    -> controller snapshot -> neo-tui ThemeManagerState
    -> typed action -> controller -> repository/config/NeoChromeState

custom-theme interview
    -> ThemeDraft.preview -> shared preview renderer/details
    -> explicit confirmation -> ThemeDraft.save(draft_id)
    -> repository only; applied=false
```

## Tech Stack

- Rust 2024 Cargo workspace, minimum Rust version `1.96.1`.
- `neo-agent` for CLI/config/controller/repository and the host Tool adapter.
- `neo-tui` for shell overlays, visible-width-safe rendering, transcript cards,
  and the existing `TuiTheme` model.
- `neo-agent-core` for `Tool`, `ToolRegistry`, typed approval actions,
  `PermissionMode`, plan-mode guards, skills, and append-only runtime events.
- `serde`, `serde_json`, and `toml` for strict tool/theme/config contracts;
  `anyhow`/existing error types for local diagnostics.
- Standard-library `Path`/`PathBuf`, platform-safe filesystem metadata, the
  existing Neo Home/theme lock, and the existing atomic config replacement path.
- `cargo nextest` with focused package/target/name filters, `cargo fmt`, `git
  diff --check`, and repository text scans for verification.

## Goal And Stop Condition

Make the documented `/theme` command real as a complete manual theme manager and
make `custom-theme` a separate, explicit-only AI-assisted creation workflow. The
feature must use one repository for theme files, the existing `TuiTheme` and
`NeoChromeState` for current rendering, the existing atomic config writer for the
startup default, and a host-owned `ThemeDraft` adapter for AI preview/save.

Stop when all of the following have focused evidence:

- Theme files have one canonical repository under `$NEO_HOME/themes/` and a
  validated logical `ThemeId` boundary.
- Explicit `[tui].theme` selection, bounded no-field startup compatibility, and
  invalid-explicit fallback are distinct and tested.
- The manager can list, filter, preview, apply for session, set startup default,
  import, copy, delete with safeguards, refresh, and close.
- Direct `/theme <name-or-id>` works while a model turn is running; bare
  `/theme` is rejected while busy and opens the manager only while idle.
- Completion and the command palette use the same catalog/resolution service.
- Session theme overrides survive unrelated `refresh_config()` calls and are
  cleared only by an explicit reload-startup-default action.
- `ThemeDraft.preview` is non-mutating, `ThemeDraft.save` persists only an
  existing previewed draft, conflicts are explicit, and success reports
  `applied: false`.
- ThemeDraft is available only in the intended root runtime and never in child or
  delegate registries.
- Ask/auto/yolo/plan permission behavior is covered without granting generic
  arbitrary file writes.
- The preview card and manager reuse one pure preview renderer and remain safe at
  wide, medium, narrow, short, CJK, and long-value dimensions.
- The skill, English and Chinese user guides, targeted tests, and architecture
  records describe the shipped behavior without the retired stub semantics.
- Context records, cache prefixes, transcript history, and default-off micro
  compaction/snipping behavior are unchanged.

## Scope Check

The approved scope is cross-module and includes `neo-agent`, `neo-tui`,
`neo-agent-core`, built-in skill content, documentation, and focused regression
tests. It is intentionally limited to the local theme domain.

Included:

- `ThemeId`, catalog descriptors, strict theme repository operations, and safe
  persistence under the single Neo Home.
- Optional `[tui].theme` configuration and startup resolution.
- Responsive `ThemeManagerState`, shared preview rendering, overlay routing, and
  controller-owned side effects.
- Slash routing, completion, command palette, live application, and refresh
  override handling.
- Root-runtime `ThemeDraft` preview/save, bounded in-memory drafts, permission
  classification, and structured tool-card presentation.
- Explicit-only `custom-theme` guidance and synchronized documentation.

Excluded:

- Hosted synchronization, marketplace, profiles, history, favorites, analytics,
  or cross-machine transfer.
- Project-local theme directories or another theme source of truth.
- Theme descriptions, inheritance/`extends`, or a new persisted metadata schema.
- Session/context persistence for a temporary theme.
- Automatic AI invocation or automatic AI application.
- General-purpose writes outside the existing ordinary tool boundaries.

## Requirement Ready Check

- **Status:** ready.
- **Reason:** the user approved the four design sections and explicitly moved the
  work to implementation planning.
- **Product decisions reopened by this plan:** none.
- **Implementation questions that remain:** local API names and exact helper
  placement may be chosen within the files and ownership boundaries below; they
  must not change the approved behavior.
- **Written-spec metadata note:** the spec's opening status line still says
  `design approved; written spec pending user review`, while its approval
  transition and the user's latest instruction establish that review is complete.
  Correct this administrative status during documentation synchronization; it is
  not a product or architecture gap.

## Facts, Assumptions, And Unknowns

### Facts

- `crates/neo-agent/src/themes.rs` currently owns strict semantic-token parsing,
  startup discovery, and `ResolvedTheme`.
- `neo_tui::primitive::TuiTheme` is the only runtime color model.
- `NeoChromeState::set_theme` already propagates theme changes through transcript
  cache invalidation and dirty state.
- `update_file_config` already provides config locking, temporary-file writing,
  and atomic replacement.
- `handle_slash_command`, `handle_simple_slash_command`, prompt completion, and
  command-palette dispatch are established extension points.
- `ToolRegistry::register` accepts runtime tools; `tool_registry_for_config` is
  used by both root runtime and the Btw sidecar.
- Core permission currently classifies unknown registered tools as ordinary
  `tool` operations and has a typed `PermissionOperation`/approval pipeline.
- Built-in skills are loaded from `SkillStore`; `auto_invokable()` controls model
  visibility and `/skill:<name>` remains the explicit activation path.
- The repository has no `docs/current/AEGIS_MINIMALITY_REFERENCE.md`. This is an
  acknowledged authority gap and must not be silently treated as satisfied.

### Assumptions

- Existing temporary-directory and environment-isolation helpers can be reused
  for repository/config tests. Tests must not rely on the user's real `$NEO_HOME`.
- The existing transcript/tool-card details channel is sufficient for a typed
  `theme_draft_preview` branch; no new event stream is required.
- A logical id can cross the `neo-agent`/`neo-tui` boundary as an opaque validated
  string snapshot. The repository remains the validator and source of truth.
- Root-runtime registration can be performed at the existing top-level registry
  construction call sites while leaving the generic registry helper safe for the
  Btw sidecar and child filtering.

### Unknowns constrained to implementation

- The exact existing helper used to inject environment variables in each test
  target.
- The exact overlay result polling point in `interactive/input.rs` after the
  manager action API is added.
- The next available ADR number at implementation closeout.

These unknowns do not justify adding a second owner, fallback parser, or generic
write path. Resolve them by following the existing local pattern in the named
files.

## Baseline And Authority Refs

### Product and architecture authority

- [`docs/aegis/specs/2026-08-04-theme-manager-design.md`](../specs/2026-08-04-theme-manager-design.md)
  Sections 1-21. This is the approved product and architecture baseline for this
  plan.
- [`docs/aegis/BASELINE-GOVERNANCE.md`](../BASELINE-GOVERNANCE.md). This governs
  ownership, module boundaries, compatibility, retirement, and authority gaps.
- [`docs/aegis/README.md`](../README.md). `specs/` and `plans/` are the canonical
  tracked homes for this work.
- [`CONTEXT.md`](../../../CONTEXT.md). Passive domain terminology only; it does not
  replace the approved spec or repository authority.

### Code baseline

- `crates/neo-agent/src/themes.rs`
- `crates/neo-tui/src/primitive/theme.rs`
- `crates/neo-tui/src/shell/state.rs`
- `crates/neo-tui/src/transcript/pane.rs`
- `crates/neo-agent/src/config/types.rs`
- `crates/neo-agent/src/config/loader.rs`
- `crates/neo-agent/src/config/mod.rs`
- `crates/neo-agent/src/config/mutations.rs`
- `crates/neo-agent/src/modes/interactive/slash_commands.rs`
- `crates/neo-agent/src/modes/interactive/prompt_completion.rs`
- `crates/neo-agent/src/modes/interactive/command_palette.rs`
- `crates/neo-agent/src/modes/interactive/input.rs`
- `crates/neo-agent/src/modes/interactive/mod.rs`
- `crates/neo-agent/src/modes/run/runtime/agent.rs`
- `crates/neo-agent/src/modes/run/mod.rs`
- `crates/neo-agent/src/modes/run/runtime/mod.rs`
- `crates/neo-agent/src/modes/btw.rs`
- `crates/neo-agent-core/src/tools/mod.rs`
- `crates/neo-agent-core/src/permissions.rs`
- `crates/neo-agent-core/src/runtime/permission.rs`
- `crates/neo-agent-core/src/skills/mod.rs`
- `crates/neo-agent-core/src/skills/builtin/custom-theme.md`
- `crates/neo-tui/src/shell/overlay.rs`
- `crates/neo-tui/src/shell/dialog_factory.rs`
- `crates/neo-tui/src/shell/input_dispatch.rs`
- `crates/neo-tui/src/shell/mod.rs`
- `crates/neo-tui/src/transcript/tool_call.rs`

### Existing tests and guide paths

- `crates/neo-agent/src/themes.rs` unit tests.
- `crates/neo-agent/src/config/mod.rs` and `config/loader.rs` unit tests.
- `crates/neo-agent/src/modes/interactive/tests.rs`.
- `crates/neo-agent/tests/cli_commands.rs`.
- `crates/neo-tui/tests/app_shell.rs`, `task_browser.rs`, `transcript_pane.rs`,
  and `tool_cards.rs`.
- `crates/neo-agent-core/tests/tool_permissions.rs`, `runtime_turn.rs`,
  `multi_agent_roles.rs`, and `skills.rs`.
- English and Chinese guides under
  `docs/user_guide/{en,zh}/customization/`, `reference/`, and `configuration/`.

### Missing authority

`docs/current/AEGIS_MINIMALITY_REFERENCE.md` is absent. Continue with the
observed owner evidence above, record the gap in the implementation evidence,
and do not invent new surfaces to compensate for it.

### BaselineUsageDraft

- **Required baseline refs:** existing theme parser/model/propagation,
  config loader and atomic mutation, slash/completion/palette routing, overlay
  input patterns, ToolRegistry/permission, SkillStore, transcript card details,
  and user guides.
- **Delivered context refs:** CodeGraph symbol exploration, codebase-memory
  call-path exploration, the approved design spec, and the current Aegis
  governance documents.
- **Missing ref:** `docs/current/AEGIS_MINIMALITY_REFERENCE.md`.
- **Decision:** continue with the gap explicitly recorded; no implementation
  boundary depends on the missing file.

## Change Necessity

- **User-visible need:** the documented `/theme` command has no route or UI, and
  the existing custom-theme stub describes an unsafe and incorrect workflow.
- **No-change option:** documentation-only changes would leave the command
  unusable; ordinary `Write` cannot provide preview fingerprints, canonical
  schema validation, or the `$NEO_HOME/themes/` write boundary.
- **Minimum code boundary:** the existing theme loader/repository owner, config
  conversion and refresh path, one TUI manager state, controller dispatch,
  one host adapter tool, the core permission classification needed for that
  tool, the existing tool-card details branch, and focused tests/docs.
- **Decision:** `code-change`, with no new durable owner.

## Existence Check

### ThemeManagerState

- **Existing candidates checked:** generic picker/select-list states,
  command-palette state, workflow picker, task browser, and dialog states.
- **Gap:** none can simultaneously represent the two-pane preview, filter/focus
  model, action confirmation state, invalid catalog entries, stable selection
  after mutation, and narrow-terminal degradation.
- **Decision:** add one transient `ThemeManagerState` under the existing TUI
  overlay system. It owns no filesystem, repository, config writer, or durable
  catalog.

### ThemeDraft

- **Existing candidates checked:** ordinary `Write`, `Edit`, generic Tool, and
  skill text alone.
- **Gap:** none can bind a previewed canonical payload to a stable draft id and
  fingerprint while enforcing theme-directory containment and conflict rules.
- **Decision:** add one `ThemeDraft` adapter in `neo-agent`, registered through
  the existing Tool trait/registry. It reuses the canonical theme repository and
  is root-runtime-only; it is not a second store or a core-to-agent dependency.

### Non-additions

- Do not add a second `ThemeStore`, `TuiTheme`, theme parser, project-local
  directory, synchronization service, or general `$NEO_HOME` write tool.
- Do not make `neo-agent-core` depend on `neo-agent`.

## Architecture Integrity Lens

- **Ownership integrity:** repository owns theme files and canonical materialized
  payloads; config owns startup id; `NeoChromeState/TuiTheme` owns current colors;
  manager owns transient UI state; `ThemeDraft` owns only bounded runtime drafts.
- **Module boundaries:** `neo-tui` receives snapshots and emits actions; it never
  reads disk or TOML. `neo-agent-core` exposes only generic Tool/permission
  contracts. `neo-agent` owns Neo Home and adapts ThemeDraft.
- **Contract changes:** `/theme`, `[tui].theme`, `ThemeManagerAction`, the
  `ThemeDraft` input/details schema, and `ThemeSave` permission semantics are
  documented and tested as explicit contracts.
- **Cascade control:** manager mutations return refreshed snapshots through one
  controller path; completion and direct apply reuse repository resolution rather
  than introducing parallel scans.
- **Dependency direction:** `neo-agent` may depend on `neo-agent-core` and
  `neo-tui`; neither core nor TUI depends on `neo-agent` theme repository code.
- **Retirement completeness:** explicit-config sorted fallback, project-local
  custom-theme wording, ordinary-Write handoff, and silent AI apply are removed
  from active behavior; only the bounded no-field startup compatibility remains.
- **Entropy flow:** the only new durable behavior is the requested repository
  capability; all other new entities are transient state, pure rendering, or a
  typed adapter with an existing owner.

## Compatibility Boundary

### Retained compatibility exception

Users with no `[tui].theme` field retain the existing sorted-first JSON startup
selection. This is an external persisted-config boundary with active existing-user
dependencies. It is bounded to the absent-field path and must be observable in
resolution diagnostics/tests as `legacy sorted fallback`; it must never run when an
explicit field exists. A future config migration or major-version decision is the
retirement trigger. No second fallback is allowed.

Existing valid semantic-token JSON remains loadable. Retired token names such as
`accent` remain rejected.

### Retired internal paths

Use delete-first retirement for active internal behavior that the new repository
owns:

- explicit configuration silently falling through to sorted discovery;
- project-local `.neo/themes` assumptions;
- custom-theme instructions that edit files through ordinary `Write`;
- custom-theme instructions that auto-apply a saved theme;
- duplicate parser/store/runtime-owner proposals.

Unknown dependency is not evidence for retaining these paths. Verify their absence
with negative scans and skill/documentation assertions.

### Persistent-state safeguards

The manager's delete, overwrite, and config-default operations touch persistent
user state. Runtime implementation is confirmation-first:

- manager deletion requires an explicit confirmation and blocks active/startup
  default dependencies;
- the repository rechecks the target under the theme-directory lock;
- overwrite and save-as-new are explicit conflict choices;
- config writes use `update_file_config` and leave prior state unchanged on error;
- no destructive operation is executed during this planning task.

## Anti-Entropy Declaration

- **Deletion class:** internal code/contract retirement plus guarded persistent
  state mutation surfaces.
- **Old path:** startup-only loader semantics, explicit-config sorted fallback,
  stale custom-theme instructions, and ordinary-Write AI handoff.
- **New canonical owner:** repository for files; config for startup id;
  `NeoChromeState/TuiTheme` for current colors; `ThemeManagerState` for UI;
  repository-backed ThemeDraft for AI save intent.
- **Preserved behavior:** valid semantic themes and no-field sorted startup
  compatibility remain available.
- **Retired behavior:** explicit-config fallback, project-local path claims,
  ordinary-Write theme saving, and silent AI application.
- **External boundary touched:** yes, the persisted config and managed theme
  directory; compatibility is explicitly bounded.
- **Source-of-truth data risk:** possible at runtime mutations; mitigated by
  confirmation, locking, revalidation, and atomic replacement.
- **User confirmation required:** for actual delete/overwrite operations at
  runtime, yes; for writing this plan, no.

## Retirement Decision

- **Path:** `delete-first` for internal stale behavior; `compat-exception` only
  for no-field startup behavior; `confirmation-first` for live persistent-state
  deletion/overwrite.
- **Why:** existing users depend on the no-field startup behavior, while the
  other paths are internal stale owners or incorrect instructions. Theme files
  and config are user-owned persistent state and require explicit runtime guards.
- **Non-edits:** no live theme/config deletion, no project-local migration, no
  broad compatibility alias, and no ordinary-Write fallback.

## TDD Route

- **Mode:** `off`.
- **Decision:** `skipped`.
- **Strict authority:** `not applicable`.
- **Test posture:** post-change focused regression at each boundary, with small
  pure-state tests added before or alongside the implementation when that is the
  shortest route to isolate layout/parser behavior.
- **Reason:** strict test-first work was not requested and the approved design
  already defines the regression matrix. This plan still requires targeted tests;
  it does not use RED/GREEN as a mandatory process.
- **Command discipline:** every test command below names one package, one target
  selector (`--lib`, `--test`, or `--bin`), and a focused test-name filter. Do not
  use broad workspace tests as evidence.

## Plan-Time Complexity Check

- Repository work is high-risk because it combines strict parsing, path safety,
  locking, atomic writes, and startup compatibility. Keep it in one owner module
  and test it independently before controller/UI work.
- Manager rendering is medium-risk but width-sensitive. Keep layout math and
  visible-width helpers pure; keep side effects in the controller.
- ThemeDraft is high-risk at the permission and persistent-write boundary. Keep
  preview/save typed and make save accept only a stored id.
- Controller integration is high-risk for busy-turn behavior and refresh
  overrides. Use a single action execution path and explicit state markers.
- Do not split work into tiny abstractions that duplicate the repository or
  runtime theme model. Prefer six coherent slices with a focused commit after
  each verified slice when implementation begins.

## Dependency Order

1. Repository/schema/config/startup owner and compatibility tests.
2. Pure preview renderer and TUI manager state/overlay plumbing.
3. Controller actions, slash grammar, completion, palette, runtime override.
4. ThemeDraft adapter, root registration, permission semantics, preview card.
5. Explicit-only custom-theme skill and skill tests.
6. Documentation, architecture status, cross-boundary negative scans, and final
   focused regression.

Task 2 can build pure UI fixtures against snapshot DTOs after Task 1 defines the
logical id and catalog contract. Task 3 depends on both repository resolution and
manager actions. Task 4 depends on repository materialization and the shared
preview renderer. Tasks 5 and 6 depend on the finalized tool/skill contract.

## Tasks

### Task 1: Establish the canonical theme repository and startup configuration

**Files:**

- `crates/neo-agent/src/themes.rs`
- `crates/neo-agent/src/config/types.rs`
- `crates/neo-agent/src/config/loader.rs`
- `crates/neo-agent/src/config/mod.rs`
- `crates/neo-agent/src/config/mutations.rs`
- focused unit tests in `crates/neo-agent/src/themes.rs`,
  `crates/neo-agent/src/config/loader.rs`, and
  `crates/neo-agent/src/config/mod.rs`
- focused startup/config coverage in `crates/neo-agent/tests/cli_commands.rs`

**Why:**

All later paths need one exact repository and one startup-selection contract.
The current module only discovers the first JSON file and accepts path-like
startup behavior; it cannot list invalid entries, resolve exact ids/names, or
perform safe mutations.

**Change Necessity:**

`code-change`. Extending the existing loader is the minimum coherent change. A
new store or parser would violate the approved ownership boundary.

**Changes:**

1. Add a validated `ThemeId` contract representing a logical relative path under
   `$NEO_HOME/themes/`, persisted with `/` separators and never as an absolute
   path. Centralize component, traversal, control-character, symlink/reparse,
   and platform-reserved-name validation at the repository boundary.
2. Extend `themes.rs` into the canonical repository/catalog owner. Add catalog
   entries that distinguish valid and invalid files without hiding valid files
   when one entry is malformed. Preserve the strict semantic-token schema and
   existing default-value materialization into the single `TuiTheme` model.
3. Implement exact id and unique exact display-name resolution. Return stable
   not-found and ambiguity errors. Direct resolution must not fuzzy-match.
4. Add canonical serialization/materialization for complete independent themes,
   including base-theme plus semantic-token overrides for the future ThemeDraft
   adapter. Do not add `description`, `extends`, or a second persisted schema.
5. Add repository operations for import/copy/delete/overwrite/save-as-new using
   the managed directory lock, re-scan/revalidation, temp-file-in-root, and
   atomic replacement sequence. Import reads an outside source but never stores
   its original path.
6. Add an explicit startup-id input to the existing `FileTuiConfig`/`TuiConfig`
   conversion. The persisted field is a logical id, not an absolute path.
7. Change startup resolution to: explicit id first; built-in default plus a
   visible diagnostic for missing/invalid explicit id; sorted-first discovery
   only when the field is absent. Never select another JSON file for an explicit
   invalid id and never auto-rewrite config.
8. Keep `ResolvedTheme` useful for runtime propagation while adding enough
   resolution provenance/diagnostic information for tests and logs to identify
   the bounded legacy fallback.
9. Provide a repository method for setting the startup id through the existing
   `update_file_config` path. A failed config write must leave both the current
   runtime and previous config unchanged.

**Impact / Compatibility:**

- Existing no-field users retain sorted-first startup behavior, bounded to the
  absent-field branch.
- Existing valid semantic JSON remains loadable.
- Absolute/traversal/symlink/reparse theme ids are rejected at the logical-id
  boundary; an external import source is read-only input.
- The repository becomes the only owner of theme file mutation and is reused by
  the manager and ThemeDraft.
- No current session theme changes are introduced in this task.

**Implementation steps (2-5 minutes each):**

1. Add `ThemeId`, catalog/result/error types beside the existing theme parser and
   map the existing `discover_themes`/`load_theme_file` behavior into repository
   methods without changing semantic token names.
2. Add safe logical-id-to-platform-path conversion and canonical materialization;
   write unit fixtures for CJK, long names, traversal, separators, reserved names,
   symlink/reparse targets, and invalid JSON entries.
3. Add the optional config field and thread it through `tui_from_file`, validation,
   `AppConfig::load`, and startup resolution; expose the legacy-fallback marker in
   a testable result/diagnostic without adding telemetry infrastructure.
4. Add lock/atomic repository mutations and the startup-default mutation helper;
   test overwrite, save-as-new, concurrent re-scan guards, and failed writes.
5. Run the focused repository/config checks, then leave all UI/controller behavior
   for the following tasks.

**Verification:**

```bash
rtk cargo nextest run -p neo-agent --lib theme_repository
rtk cargo nextest run -p neo-agent --lib config_theme_selection
rtk cargo nextest run -p neo-agent --test cli_commands theme
```

Expected evidence includes explicit-versus-absent config resolution, built-in
fallback with diagnostic, exact id/name behavior, path safety, invalid-entry
catalog behavior, atomic failure preservation, import/copy/delete conflict rules,
and `[tui].theme` lock/helper usage.

**Commit boundary:**

One scoped commit after the repository/config tests pass, for example
`feat: add canonical theme repository and startup selection`.

### Task 2: Add shared preview rendering and the responsive TUI manager state

**Files:**

- `crates/neo-tui/src/theme_preview.rs` (new pure renderer module)
- `crates/neo-tui/src/shell/theme_manager.rs` (new transient state/action module)
- `crates/neo-tui/src/lib.rs`
- `crates/neo-tui/src/shell/mod.rs`
- `crates/neo-tui/src/shell/overlay.rs`
- `crates/neo-tui/src/shell/dialog_factory.rs`
- `crates/neo-tui/src/shell/input_dispatch.rs`
- `crates/neo-tui/src/shell/state.rs` only where existing chrome overlay access
  needs a small accessor
- `crates/neo-tui/tests/theme_manager.rs` (new focused integration target)
- `crates/neo-tui/tests/app_shell.rs` only for overlay blocking regression
- `crates/neo-tui/tests/tool_cards.rs` only for the shared preview fixture if
  the existing card helper is the least-duplication path

**Why:**

The existing picker states cannot express the approved two-pane preview,
filter/focus/action contract, invalid entries, stable post-mutation selection,
or narrow-terminal degradation. The preview card and manager also need one pure
renderer so their sample content cannot drift.

**Change Necessity:**

`code-change`. A new transient state is justified by the existence check; it is
not a second catalog owner. A pure renderer is justified because both manager
and tool-card previews must display the same representative surface.

**Changes:**

1. Add `ThemePreviewRenderer` that accepts a `TuiTheme`, width/height, and a
   sample model/value. Render welcome/banner, user and assistant messages, tool
   status/footer, diff roles, approval/selection, and permission/context footer
   states. It must not read the transcript, mutate chrome, or append events.
2. Add UI snapshot DTOs for catalog entries and status markers. Carry logical ids
   as opaque strings across the crate boundary; do not carry absolute paths or
   make `neo-tui` depend on `neo-agent`.
3. Add `ThemeManagerState` with catalog snapshot, filter text/indices, selected
   id, focus (`List`, `Preview`, `Actions`, `Filter`), preview value, pending
   confirmation, and status/error message.
4. Add `ThemeManagerAction` with the approved action contract: apply session, set
   startup default, import, duplicate, delete, refresh, and close. The state
   only emits actions; it never writes files or config.
5. Register the state in the existing `OverlayKind` and list-selection/input
   routing. Add open/take-result accessors through the existing dialog factory
   style rather than a parallel overlay stack.
6. Implement wide (`>=100x18`), medium (`68..99`), narrow (`<68`), and very-short
   layouts. Use `visible_width`, safe truncation, and existing style helpers for
   every row. Preserve title/focus/status/essential action on short terminals.
7. Implement navigation, filter editing, focus cycling, action shortcuts, delete
   confirmation, and action-bar equivalents. Selection changes only preview.
8. Ensure overlay input blocks the composer while open and close/clear-filter
   behavior follows the approved `Esc` contract.

**Impact / Compatibility:**

- Existing overlays and command palette remain unchanged except for one new
  `OverlayKind` branch and shared routing cases.
- The manager can render invalid entries but cannot mark them applicable; the
  controller/repository remains the final validator.
- The renderer is pure and has no session, filesystem, config, or model-context
  side effect.

**Implementation steps (2-5 minutes each):**

1. Add the pure preview renderer and snapshot/action types; render fixed sample
   data using the existing `TuiTheme`, `Line`, `Style`, `visible_width`, and
   truncation helpers.
2. Add manager state transitions and unit-level render helpers for filtering,
   focus, navigation, selection, confirmation, and stable selection ids.
3. Add the overlay enum/factory/input branches and wire render dimensions through
   the existing shell rendering path.
4. Add wide/medium/narrow/short/CJK/long-value tests, then run the focused TUI
   target before any controller integration.

**Verification:**

```bash
rtk cargo nextest run -p neo-tui --test theme_manager theme_manager
rtk cargo nextest run -p neo-tui --test app_shell overlay
rtk cargo nextest run -p neo-tui --test task_browser visible_width
```

Expected evidence includes all responsive layouts, bounded visible widths,
selection-only preview behavior, focus/navigation/action mapping, blocking
overlay input, and the shared renderer's representative sample coverage.

**Commit boundary:**

One scoped commit after the TUI target passes, for example
`feat: add responsive theme manager overlay and preview renderer`.

### Task 3: Connect controller actions, slash commands, completion, palette, and runtime overrides

**Files:**

- `crates/neo-agent/src/modes/interactive/theme_manager.rs` (new controller
  adapter/action executor)
- `crates/neo-agent/src/modes/interactive/mod.rs`
- `crates/neo-agent/src/modes/interactive/slash_commands.rs`
- `crates/neo-agent/src/modes/interactive/prompt_completion.rs`
- `crates/neo-agent/src/modes/interactive/command_palette.rs`
- `crates/neo-agent/src/modes/interactive/input.rs`
- `crates/neo-agent/src/config/mod.rs` only if the session override marker belongs
  on `AppConfig::inherit_live_state`; keep controller metadata out of serialized
  config
- `crates/neo-agent/src/modes/interactive/tests.rs`
- `crates/neo-agent/tests/cli_commands.rs` for user-visible command parsing if
  the existing target owns that boundary

**Why:**

Task 1 owns repository behavior and Task 2 owns transient UI state, but neither
can decide when an action is safe, apply a theme to the current chrome, persist a
startup id, or preserve a temporary override across refreshes. This task is the
single controller boundary for those side effects.

**Change Necessity:**

`code-change`. The public slash command and runtime behavior do not exist. A
controller adapter is required to keep filesystem/config effects out of TUI
state and to share exact catalog resolution across slash, completion, and UI
actions.

**Changes:**

1. Add one controller-side catalog snapshot adapter that maps repository entries
   to the TUI DTOs and one action executor that re-scans after import/copy/delete
   and restores stable selection according to the spec.
2. Add `/theme` grammar before generic prompt handling. Trim boundary whitespace,
   preserve the argument as one exact value, recognize only lowercase `/theme`,
   and keep `/themeish`/embedded prose as normal prompts.
3. Make bare `/theme` clear the submitted prompt and open the manager only when
   `active_turn` is idle. While busy, leave the turn intact and show a precise
   manager-idle status.
4. Make `/theme <name-or-id>` resolve exact logical id first, then unique exact
   display name, and apply directly through `NeoChromeState::set_theme` while
   busy or idle. No fuzzy direct application.
5. Apply-for-session must update only current render state and a controller-owned
   ephemeral session-override marker. It must not write `config.toml`, append
   transcript/session events, alter the prompt, or modify model context.
6. Set-startup-default must persist only the logical id through the repository
   config mutation and leave the current `TuiTheme` unchanged. If writing fails,
   retain the old runtime and config state.
7. Add explicit Reload startup default behavior that clears the override marker and
   applies the resolved config theme. Update `refresh_config()` so unrelated
   refreshes preserve the current override and do not overwrite chrome.
8. Add import/copy/delete/refresh action handling, including path-dialog input,
   explicit conflict choice, active/default deletion safeguards, and retryable
   status while keeping the manager open after errors.
9. Add live catalog completion for `/theme` ids and display names without any
   mutation. Reuse the repository service; do not scan files in completion code.
10. Add `theme.manager` to the command palette and route it to the same manager
    entry as bare `/theme`.
11. Keep direct application allowed during a model turn while blocking only the
    manager overlay entry. Verify the normal prompt/queue behavior is untouched.

**Impact / Compatibility:**

- Adds a public slash command and one palette command.
- Retains the documented direct-apply behavior while making the bare manager
  explicitly idle-only.
- Temporary theme state remains controller metadata and is never serialized or
  inserted into context/session records.
- Existing config refresh callers continue to refresh other derived state; only
  the theme assignment is conditional on the override marker.

**Implementation steps (2-5 minutes each):**

1. Add the controller adapter and snapshot mapping; exercise action outcomes with
   repository fixtures without opening a real TUI.
2. Add slash parsing and exact resolution, then add direct-apply and busy/bare
   manager tests.
3. Add manager action polling, config mutation, refresh override behavior, and
   stable selection after rescan.
4. Add completion and command-palette entries through their existing registries;
   confirm completion is read-only.
5. Run focused interactive and CLI tests, then inspect the transcript/context
   assertions before moving to ThemeDraft.

**Verification:**

```bash
rtk cargo nextest run -p neo-agent --lib theme_slash
rtk cargo nextest run -p neo-agent --lib theme_runtime
rtk cargo nextest run -p neo-agent --test cli_commands theme
```

Expected evidence includes exact id/name/ambiguity behavior, direct apply while
busy, bare-manager rejection while busy, completion/palette routing, persistent
startup-default separation, refresh override preservation, stable selection, and
context/session integrity.

**Commit boundary:**

One scoped commit after controller and interactive tests pass, for example
`feat: wire theme manager actions and slash command`.

### Task 4: Implement root-runtime ThemeDraft, permission semantics, and the structured preview card

**Files:**

- `crates/neo-agent/src/theme_draft.rs` (new repository-backed Tool adapter and
  bounded draft store)
- `crates/neo-agent/src/lib.rs` or the existing module declaration file
- `crates/neo-agent/src/modes/run/runtime/agent.rs`
- `crates/neo-agent/src/modes/run/mod.rs`
- `crates/neo-agent/src/modes/run/runtime/mod.rs`
- `crates/neo-agent/src/modes/btw.rs` only to preserve its generic registry
  boundary if a call-site flag/helper is needed; do not register ThemeDraft there
- `crates/neo-agent-core/src/tools/mod.rs` only for the existing public Tool
  registration/re-export boundary when needed
- `crates/neo-agent-core/src/permissions.rs`
- `crates/neo-agent-core/src/runtime/permission.rs`
- `crates/neo-tui/src/transcript/tool_call.rs`
- `crates/neo-agent/tests/tool_terminal_guardian.rs` only if the root registry
  construction fixture is the existing test boundary
- `crates/neo-agent-core/tests/tool_permissions.rs`
- `crates/neo-agent-core/tests/runtime_turn.rs`
- `crates/neo-agent-core/tests/multi_agent_roles.rs`
- `crates/neo-tui/tests/tool_cards.rs`
- focused unit tests in `crates/neo-agent/src/theme_draft.rs`

**Why:**

AI creation needs a host-controlled preview/save contract that ordinary `Write`
cannot provide. Permission behavior must distinguish non-mutating preview from a
special save mutation while keeping the core dependency direction intact.

**Change Necessity:**

`code-change`. This is the minimum adapter required by the approved handoff
choice. It reuses the Task 1 repository and Task 2 renderer; it does not create a
second persistence or runtime theme owner.

**Changes:**

1. Define typed tagged `ThemeDraft` input with `preview` and `save` actions. Use
   strict unknown-field rejection, bounded display-name validation, optional
   base `ThemeId`, semantic-token allowlist, and existing named/hex color parsing.
2. Add a bounded in-memory draft store, such as the most recent eight canonical
   drafts, protected by the runtime's existing shared synchronization pattern.
   Draft ids are opaque, expire with the runtime, and are evicted deterministically.
3. On preview, load the base theme or built-in default, apply overrides, fully
   materialize independent canonical role colors, compute a stable fingerprint,
   store the canonical payload, and return the required
   `theme_draft_preview` details with normalized colors and contrast warnings.
4. On save, accept only `draft_id` and `overwrite`. Revalidate the stored draft
   and current catalog, write only inside `$NEO_HOME/themes/`, return typed
   invalid/base/conflict/expired/permission/plan/atomic errors, and report
   `applied: false` on success. New colors, paths, or names require a new preview.
5. Register the tool only at the existing root-runtime registry call sites. Keep
   the generic registry helper used by Btw free of ThemeDraft, and ensure child/
   delegate registries cannot acquire it through role filtering or inherited
   `ToolContext`.
6. Extend the core permission contract with a typed `ThemeSave` operation or
   equivalent special classification. Preview runs without normal write approval;
   save is one-time approval in Ask mode, with no session-wide theme-save grant.
   Auto/Yolo preserve current semantics. Plan mode allows preview and denies save.
   The permission path must never grant generic `file_write` and must re-check the
   action from validated arguments before execution.
7. Keep permission approval presentation typed and branch on action/operation,
   never on UI label text. Ensure cached ordinary tool/session approvals cannot
   silently authorize a later ThemeDraft save.
8. Add the `theme_draft_preview` details branch to the existing tool-card
   renderer. Reuse `ThemePreviewRenderer`, show name/status/color samples/
   representative samples/warnings, and keep it non-blocking with no Apply action.
9. Confirm save never calls `NeoChromeState::set_theme`, changes transcript theme,
   appends a user message, rewrites context, or updates session metadata.

**Impact / Compatibility:**

- Adds one root-runtime tool schema and one dedicated permission operation.
- Ordinary `Write` remains workspace-contained and is not extended to Neo Home.
- Core remains independent of `neo-agent`; only generic permission/tool contracts
  are changed.
- Existing child/delegate tool visibility and role ceilings remain unchanged;
  ThemeDraft is explicitly absent from child registries.
- Preview card details are ordinary append-only tool results and do not create a
  new event type or durable draft store.

**Implementation steps (2-5 minutes each):**

1. Implement typed input, canonical draft store, preview materialization, stable
   fingerprinting, and bounded eviction against repository test fixtures.
2. Implement save lookup/conflict/overwrite/atomic path and verify no runtime
   theme mutation occurs on successful save.
3. Add root-only registration at the two top-level runtime construction sites and
   keep Btw/child construction unchanged; add registry visibility assertions.
4. Add the core `ThemeSave` permission branch, plan guard, no-session-grant
   approval options, and Ask/Auto/Yolo/plan tests.
5. Add the tool-card details branch and width tests using the shared renderer.

**Verification:**

```bash
rtk cargo nextest run -p neo-agent --lib theme_draft
rtk cargo nextest run -p neo-agent-core --test tool_permissions theme_draft
rtk cargo nextest run -p neo-agent-core --test multi_agent_roles theme_draft
rtk cargo nextest run -p neo-agent-core --test runtime_turn theme_draft
rtk cargo nextest run -p neo-tui --test tool_cards theme_draft_preview
```

Expected evidence includes strict preview/save input, stable fingerprint,
bounded/expired drafts, conflict and explicit overwrite behavior, arbitrary-path
rejection, atomic failure preservation, applied=false, no child registration,
Ask one-time approval, Auto/Yolo behavior, plan denial, and shared preview-card
rendering.

**Commit boundary:**

One scoped commit after tool, permission, child-visibility, and card tests pass,
for example `feat: add guarded ThemeDraft preview and save tool`.

### Task 5: Replace the custom-theme stub with the explicit AI workflow

**Files:**

- `crates/neo-agent-core/src/skills/builtin/custom-theme.md`
- `crates/neo-agent-core/src/skills/builtin/mod.rs` only if the embedded builtin
  registration or metadata list requires an update
- `crates/neo-agent-core/src/skills/mod.rs` only if explicit-only metadata
  parsing needs a focused contract assertion
- `crates/neo-agent-core/tests/skills.rs`
- `crates/neo-agent/src/modes/interactive/tests.rs` for explicit
  `/skill:custom-theme` activation if that is the existing skill invocation test
  boundary

**Why:**

The current stub claims a project-local path, encourages manual token edits, and
implies direct activation. The approved product requires a complete explicit-only
interview and preview-confirm-save handoff.

**Change Necessity:**

`code-change` in the built-in skill resource, plus focused metadata tests. The
skill text alone cannot persist a theme; it must name the host-controlled
ThemeDraft contract and forbid ordinary Write fallback.

**Changes:**

1. Replace the stub with valid builtin skill metadata containing
   `disableModelInvocation: true` and a clear explicit `/skill:custom-theme`
   entry condition.
2. Describe one focused question per turn: base/revision choice, light/dark and
   background/surface direction, brand, text contrast, status roles, message/diff/
   selection/approval/footer/shell readability, display name, and conflict choice.
3. Require a structured semantic-token `ThemeDraft.preview` call and explain that
   the preview is non-mutating. Present warnings without pretending they are a
   successful save.
4. Require explicit conversational confirmation before `ThemeDraft.save(draft_id)`.
   A modification starts a new preview; save is the only persistence path.
5. Require explicit overwrite choice after a conflict and prohibit changing name,
   colors, or path in a save request. Instruct the user to apply later through
   `/theme <ThemeId>` and state that save does not auto-apply.
6. If ThemeDraft is unavailable, report the missing capability and stop. Never
   fall back to ordinary `Write`, direct file editing, or a project-local theme
   path.
7. Add metadata/flow tests and negative assertions that the old stub claims are
   absent from the installed builtin content.

**Impact / Compatibility:**

- `custom-theme` remains explicit-only and is not added to model auto-invocation.
- Existing skill discovery and `/skill:<name>` routing remain the same.
- The skill becomes a consumer of the host tool contract, not an owner of theme
  schema, files, or runtime application.

**Implementation steps (2-5 minutes each):**

1. Replace the markdown body and metadata while keeping the builtin package
   format and current semantic token vocabulary.
2. Add explicit-only and no-Write-fallback assertions to the existing skill test
   target.
3. Add a focused activation/visibility test only if the existing interactive
   skill tests do not already cover explicit builtin invocation.
4. Run the skill checks and inspect the rendered guidance for stale paths or
   automatic-apply wording.

**Verification:**

```bash
rtk cargo nextest run -p neo-agent-core --test skills custom_theme
rtk cargo nextest run -p neo-agent --lib custom_theme
rtk rg -n "\.neo/themes|ordinary Write|auto-apply|directly activate|disableModelInvocation" crates/neo-agent-core/src/skills/builtin/custom-theme.md crates/neo-agent-core/tests/skills.rs
```

The positive scan must show explicit-only metadata and the ThemeDraft preview/
save flow. The negative assertions must prove the project-local path, ordinary
manual-edit path, and silent-activation claims are gone.

**Commit boundary:**

One scoped commit after skill metadata and flow tests pass, for example
`docs: define explicit custom-theme skill workflow`.

### Task 6: Synchronize user guides and architecture records

**Files:**

- `docs/user_guide/en/customization/themes.md`
- `docs/user_guide/zh/customization/themes.md`
- `docs/user_guide/en/reference/slash-commands.md`
- `docs/user_guide/zh/reference/slash-commands.md`
- `docs/user_guide/en/configuration/config-files.md`
- `docs/user_guide/zh/configuration/config-files.md`
- `docs/user_guide/en/configuration/data-locations.md`
- `docs/user_guide/zh/configuration/data-locations.md`
- `docs/user_guide/en/customization/skills.md`
- `docs/user_guide/zh/customization/skills.md`
- `docs/aegis/specs/2026-08-04-theme-manager-design.md` for the stale approval
  status line only
- `docs/aegis/INDEX.md` if the plan/spec/architecture records require an index
  entry
- the next sequential architecture decision record under `docs/aegis/adr/` after
  implementation acceptance
- the corresponding post-implementation baseline record under
  `docs/aegis/baseline/`

**Why:**

The user guide is the source of the missing `/theme` expectation, and the design
has an explicit ADR signal. Documentation must describe the final ownership and
compatibility behavior without reintroducing the retired stub semantics.

**Change Necessity:**

`documentation-change`. The command and configuration are public contracts, and
architecture governance requires an accepted decision/baseline sync after the
implementation is verified.

**Changes:**

1. Document `$NEO_HOME/themes/` as the only managed theme directory and
   `[tui].theme` as a logical id, not an absolute path.
2. Document bare `/theme` as idle-only manager entry and `/theme <name-or-id>` as
   immediate current-session application, including exact resolution behavior.
3. Document Apply for session versus Set startup default, import validation and
   conflict choices, deletion safeguards, refresh behavior, and narrow-terminal
   manager expectations.
4. Document explicit-only `custom-theme`, preview-before-save, explicit
   confirmation, conflict/overwrite behavior, no auto-apply, and later `/theme`
   application.
5. Remove project-local `.neo/themes` claims, manual token editing instructions,
   direct activation claims, and any implication that ordinary Write can save a
   custom theme.
6. Correct the written spec status line from pending review to the current
   approved/planning transition, preserving the approved decision text.
7. After implementation acceptance, record the canonical repository/runtime
   owner, `/theme` contract, ThemeDraft dependency direction, permission boundary,
   and bounded legacy fallback in the next ADR and corresponding baseline record.
   Do not create those acceptance records before the implementation evidence is
   available.

**Impact / Compatibility:**

- User-facing documentation becomes consistent with the actual command and
  configuration behavior.
- The missing authority reference remains explicitly recorded; it is not replaced
  by an invented baseline.
- ADR/baseline records document accepted implementation decisions and do not
  replace the design spec.

**Implementation steps (2-5 minutes each):**

1. Update the English theme/slash/config/data-location/skill pages in one
   terminology pass.
2. Mirror the same contract in the Chinese pages without changing semantic ids or
   introducing a second path.
3. Correct the spec status line and update the Aegis index only for records
   actually created.
4. At implementation closeout, write the next-numbered ADR and baseline from
   verified evidence, then run documentation scans.

**Verification:**

```bash
rtk rg -n "/theme|custom-theme|\[tui\]|NEO_HOME/themes|\.neo/themes" docs/user_guide/en docs/user_guide/zh docs/aegis/specs/2026-08-04-theme-manager-design.md
rtk git diff --check
```

Expected evidence is semantic parity between English and Chinese guides, no stale
project-local path or auto-apply claim, and a corrected spec approval status. ADR
and baseline records must cite the actual implementation commit/test evidence.

**Commit boundary:**

Keep guide/spec-status edits in one scoped documentation commit after the focused
implementation behavior is verified, for example
`docs: synchronize theme manager guidance`.

### Task 7: Final cross-boundary acceptance, retirement, and handoff evidence

**Files:**

- all source/test files changed by Tasks 1-6
- `docs/aegis/plans/2026-08-04-theme-manager.md`
- `docs/aegis/INDEX.md`
- accepted ADR/baseline records created by Task 6
- no unrelated dirty files

**Why:**

The feature crosses persistence, runtime, TUI, permission, skill, and context
boundaries. Focused per-task tests are necessary but do not prove that the old
owners are gone or that cross-boundary invariants remain intact.

**Change Necessity:**

`verification-and-governance`. This task should make no new product behavior. It
collects evidence, performs negative scans, and records residual risk.

**Checks:**

1. Verify the main path: repository -> controller -> `NeoChromeState` for manual
   apply, and preview -> confirmation -> ThemeDraft save -> repository for AI
   creation.
2. Verify no lingering active path for explicit-config sorted fallback, ordinary
   Write theme persistence, project-local themes, or silent AI apply.
3. Verify negative behavior: invalid explicit id does not choose another file;
   ambiguous display name does not apply; expired draft does not write; bare
   manager does not open while busy; save does not apply.
4. Verify boundaries: no arbitrary path writes, no child ThemeDraft, no generic
   `file_write` grant, no transcript/context/session mutation, and default-off
   micro compaction/snipping remains unchanged.
5. Verify formatting and documentation whitespace without staging unrelated work.

**Verification:**

```bash
rtk cargo nextest run -p neo-agent --lib theme_repository
rtk cargo nextest run -p neo-agent --lib theme_runtime
rtk cargo nextest run -p neo-agent --test cli_commands theme
rtk cargo nextest run -p neo-tui --test theme_manager theme_manager
rtk cargo nextest run -p neo-tui --test tool_cards theme_draft_preview
rtk cargo nextest run -p neo-agent-core --test tool_permissions theme_draft
rtk cargo nextest run -p neo-agent-core --test skills custom_theme
rtk cargo fmt --all --check
rtk git diff --check
rtk rg -n "\.neo/themes|run_theme_write|ThemeDraft.*Write|auto.apply|auto-apply|discover_themes\(\).*explicit" crates/neo-agent crates/neo-agent-core crates/neo-tui docs/user_guide
```

The final `rg` scan is a review aid, not a substitute for tests. Any intentional
historical reader or documentation mention must be explained in the evidence;
active behavior must not retain the retired path.

**Residual risk report:**

Focused local tests do not prove every provider, terminal implementation, or
native Windows/Linux filesystem race. Record missing credentials, unavailable
providers, platform-only filesystem observations, and any unexecuted live-TUI
check as residual risk. Do not replace a blocked live check with a broad green
local test claim.

**Commit boundary:**

Do not create a new behavior commit in this task. After the preceding scoped
commits and final evidence pass, record the final plan/test/architecture evidence
in the project-required closeout format. Never stage `CONTEXT.md` or unrelated
worktree changes.

## Verification Matrix

| Boundary | Required evidence |
| --- | --- |
| Repository/schema | semantic token strictness, exact ids/names, invalid-entry catalog, path safety, CJK/long/reserved names, symlink/reparse rejection |
| Persistence | import/copy/delete/overwrite/save-as-new, lock/re-scan, atomic failure preservation, config helper use |
| Startup | explicit id, no-field sorted compatibility, invalid explicit built-in fallback with diagnostic, no explicit fallback re-entry |
| Manager | wide/medium/narrow/short render, visible widths, filter/focus/navigation, confirmation, stable selection, selection-only preview |
| Controller | direct apply while busy, bare manager idle guard, exact ambiguity errors, palette/completion, refresh override, startup-default separation |
| Runtime theme | `NeoChromeState::set_theme`, transcript cache invalidation, no canonical transcript/context/session mutation |
| ThemeDraft | preview materialization/fingerprint, bounded/expired store, save-by-id, conflict/overwrite, arbitrary path rejection, applied=false |
| Permission | preview allowed, save one-time Ask approval, Auto/Yolo semantics, plan denial, no generic file-write or session-wide theme grant |
| Registry | root runtime registration, Btw exclusion, child/delegate exclusion |
| Tool card | `theme_draft_preview` details, shared renderer, warnings, width safety, no Apply action |
| Skill | explicit-only metadata, one-question flow, preview confirmation, no Write fallback, no auto-apply |
| Documentation | English/Chinese parity, logical id wording, single location, no retired stub semantics |
| Context integrity | append-only tool events, unchanged cache prefix, no history rewrite, micro/snipping defaults remain off |

## Plan Pressure Test

- **Does the plan preserve one owner?** Yes. Repository, config, runtime theme,
  manager state, and bounded drafts have distinct responsibilities.
- **Does it add a duplicate model?** No. JSON maps into the existing `TuiTheme`;
  UI DTOs are snapshots, not durable theme models.
- **Does it add a fallback before checking the owner?** No. Explicit invalid ids
  use built-in default with diagnostic; only absent-field startup retains the
  documented compatibility path.
- **Can a preview mutate persistent or runtime state?** No. Preview stores only a
  bounded in-memory draft and returns details; save is the only repository mutation
  and never applies.
- **Can an approval expand the write root?** No. Repository path validation and
  lock checks enforce `$NEO_HOME/themes/`; permission grants do not create a
  generic file-write capability.
- **Can a child agent gain the new capability accidentally?** No. Registration is
  root-only and child ceilings remain reducing filters.
- **Can a busy turn be interrupted by the manager?** Direct apply is render-only
  and allowed; the blocking bare manager is rejected while busy.
- **Can UI code write config/files?** No. TUI emits typed actions; controller and
  repository perform side effects.
- **Can context integrity regress?** No intended path appends theme state to
  prompt/session records; tool preview/save uses existing append-only events.
- **Is the remaining compatibility bounded?** Yes. No-field startup only, with a
  diagnostic/test-visible source and an explicit future retirement trigger.

## Execution Readiness View

- **Spec readiness:** approved by the user; the written status metadata is a
  documentation correction scheduled in Task 6.
- **Requirement readiness:** ready; no product decision is unresolved.
- **Architecture readiness:** ready with the documented missing-authority gap.
- **Task readiness:** ready; dependencies and exact file boundaries are listed.
- **Verification readiness:** ready; each task has focused package/target/filter
  commands and the final matrix covers all acceptance boundaries.
- **Safety readiness:** ready; persistent-state operations remain runtime
  confirmation-first and no destructive action is part of planning.
- **Recommended execution route:** `subagent-driven` for independent slices,
  with sequential integration at Tasks 3, 4, and 7. Each subagent must receive
  the implementer directive, the no-revert rule, the exact task file list, and
  the requirement to run only that task's focused commands before returning.
- **Current-session boundary:** this planning request stops after the plan and
  index are written and self-audited. Implementation begins only through the
  separate execution route.
