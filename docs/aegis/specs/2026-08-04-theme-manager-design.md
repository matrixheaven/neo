# Neo Theme Manager and AI-Assisted Custom Theme Design

Date: `2026-08-04`
Status: `design approved; planning and implementation landed — ADR and baseline pending final regression acceptance`

Architecture review required: `yes`

ADR signal: `yes`. This design changes a public slash command, configuration
schema, persistence source-of-truth, runtime ownership boundaries, dependency
direction, permission behavior, and compatibility retirement. After
implementation and acceptance, record the accepted architecture decision and
update the relevant baseline. This document is not an accepted ADR.

## 1. Decision

Neo SHALL implement two separate, deliberately different theme workflows:

```text
/theme [<name-or-id>]       Human-operated theme management
/skill:custom-theme         Explicit-only AI-assisted theme creation
```

`/theme` is a complete manual manager. It SHALL support:

- theme selection and filtering;
- a representative real-TUI preview;
- applying a theme to the current session;
- setting a startup default independently from current-session application;
- importing and archiving an external theme file;
- copying a theme;
- confirmed deletion with active/default safeguards.

`custom-theme` is a complete AI-assisted creation workflow. It SHALL:

1. interview the user about the desired theme;
2. produce a structured semantic-token draft;
3. preview that exact draft through a host-controlled tool;
4. wait for explicit user confirmation;
5. save only the exact previewed draft through the canonical theme repository;
6. leave application to `/theme` and never auto-apply the saved theme.

The existing `TuiTheme` model and runtime propagation path remain canonical.
The design does not create a second durable theme store or a second long-term
runtime color owner.

## 2. Problem

The user guide currently describes `/theme <name>`, but the interactive slash
router has no `/theme` implementation and no associated manager UI. The theme
loader already supports semantic JSON colors, but it only discovers a theme at
startup and does not expose management operations.

The repository also contains a short `custom-theme` skill stub. It describes
manual file editing and direct activation, which does not provide a complete AI
creation workflow and does not respect the boundary between AI-assisted
creation and human-operated management.

The feature must therefore connect the existing theme parser and renderer to:

- slash parsing and completion;
- a blocking but responsive manager overlay;
- live current-session application;
- explicit startup-default persistence;
- safe file operations under `$NEO_HOME/themes/`;
- a controlled AI preview/save path;
- permission, atomicity, error, documentation, and test contracts.

## 3. Product Requirements

### 3.1 Manual manager requirements

1. Bare `/theme` opens a manager only when the main turn is idle.
2. `/theme <name-or-id>` applies an exact theme immediately, including while a
   model turn is running.
3. Manager selection changes only the local preview; it does not apply until
   the user chooses Apply for session.
4. Apply for session affects the current runtime only and does not write
   `config.toml`.
5. Set startup default is a separate explicit action that writes the logical
   theme id to `[tui].theme` and does not change the current session.
6. The manager supports list, filter, preview, import, copy, delete, refresh,
   and close.
7. Import copies a validated external file into `$NEO_HOME/themes/` and never
   persists its original absolute path.
8. Same-name import requires Overwrite or Save as new. There is no silent
   overwrite.
9. Deleting the current active theme or the configured startup default is
   blocked until the user handles that dependency explicitly.
10. Manager operations are safe for CJK, long names, long paths, and narrow
    terminals.

### 3.2 AI creation requirements

1. `custom-theme` is explicit-only and cannot be selected by model invocation.
2. The skill asks one focused question at a time and uses the current semantic
   token schema.
3. It cannot use ordinary `Write` to write `$NEO_HOME/themes/`.
4. Preview returns a stable draft id and fingerprint.
5. Save accepts only an existing draft id, never new arbitrary color content or
   an arbitrary path.
6. A conflict returns without writing; overwrite requires an explicit choice.
7. Save does not apply the theme to the current TUI.
8. Preview/save events are normal append-only tool events and do not rewrite
   session history, system prompts, or context cache prefixes.

## 4. Scope

### 4.1 In scope

- Theme catalog and repository operations in `neo-agent`.
- A `ThemeId` logical relative-path contract.
- `[tui].theme` configuration and bounded legacy startup compatibility.
- `/theme` parsing, dispatch, completion, and command-palette entry.
- `ThemeManagerState` and responsive manager rendering in `neo-tui`.
- A shared representative theme preview renderer.
- Current-session theme application and refresh-config override handling.
- Import, copy, delete, and default persistence with locking and atomicity.
- The explicit-only `custom-theme` skill.
- A host-registered `ThemeDraft` tool with preview/save actions.
- Structured preview tool-card rendering.
- English and Chinese documentation updates.
- Targeted tests for repository, config, runtime, UI, slash, skill, tool, and
  context-integrity boundaries.

### 4.2 Out of scope

- Hosted theme synchronization, marketplace, profiles, or cross-machine sync.
- Project-local `.neo/themes` as a second theme location.
- A second durable theme store or a second `TuiTheme` model.
- Automatic theme generation or saving without the explicit skill flow.
- Automatic application of an AI-created theme.
- Arbitrary external filesystem writes by `custom-theme`.
- A new theme description/metadata/`extends` schema in v1.
- Theme history, favorites, recently-used ranking, or analytics.
- Persisting current-session theme state in sessions, transcript context, or
  system prompts.

## 5. Existing Owners and Evidence

| Responsibility | Existing owner to reuse or extend |
| --- | --- |
| Theme JSON parsing and startup discovery | `crates/neo-agent/src/themes.rs` |
| Runtime theme value | `neo-tui::primitive::TuiTheme` |
| Current chrome theme propagation | `NeoChromeState::set_theme` and existing transcript propagation |
| Config parsing and derived theme | `crates/neo-agent/src/config/loader.rs`, `config/mod.rs` |
| Atomic config mutation | `update_file_config` and config mutation helpers |
| Slash routing | `crates/neo-agent/src/modes/interactive/slash_commands.rs` |
| Command palette | `crates/neo-agent/src/modes/interactive/command_palette.rs` |
| Picker/dialog/overlay input patterns | existing `neo-tui` overlays and dialogs |
| Tool registration | `ToolRegistry::register` and `tool_registry_for_config` |
| Tool permission | `neo-agent-core` runtime permission pipeline |
| Skill activation | existing built-in skill and `/skill:<name>` path |
| Session event persistence | existing append-only AgentEvent/session pipeline |

The existing `themes.rs` parser currently models `ThemeFile { name, colors }`
with strict semantic color keys. The existing `TuiTheme` contains derived and
runtime-only values beyond those JSON keys. The implementation SHALL extend
these owners rather than copy them.

The project authority file `docs/current/AEGIS_MINIMALITY_REFERENCE.md` is
currently missing. This is recorded as an authority gap. The design relies on
observed existing owners and the repository's current governance document and
must not treat the missing file as evidence for additional surfaces.

## 6. Terminology

### 6.1 ThemeId

`ThemeId` is a logical path relative to `$NEO_HOME/themes/`. Its persisted
representation uses `/` separators on every platform. It is not an absolute
path and is not a display name.

A managed file such as:

```text
$NEO_HOME/themes/solarized-dark.json
```

has the logical id `solarized-dark.json`.

The repository converts this logical representation to platform paths only at
its boundary. Each component is validated against traversal, control
characters, symlink/reparse behavior, and platform filename rules.

### 6.2 Display name

The display name is the optional `name` field in the theme JSON. If absent,
the repository uses a user-facing name derived from the file name. Display
names are not required to be unique.

Exact `/theme <argument>` resolution is:

1. exact `ThemeId` match;
2. otherwise an exact display-name match if it is unique;
3. otherwise a local error explaining ambiguity or absence.

No fuzzy or prefix match is used for direct application.

### 6.3 Session override

A session override is ephemeral controller metadata indicating that the user
applied a theme for the current session without changing the startup default.
The actual color value remains in `NeoChromeState/TuiTheme`. The marker exists
only so unrelated config refreshes do not replace the current runtime theme.

### 6.4 Theme draft

A draft is an in-memory, canonicalized theme payload returned by
`ThemeDraft.preview`. It is not a durable file and is not a second source of
truth. It expires with the interactive session runtime (spans turns) or
bounded draft-store eviction.

## 7. Architecture and Ownership

### 7.1 Canonical ownership

| Data or behavior | Canonical owner | Explicit non-owner |
| --- | --- | --- |
| Theme JSON content | `neo-agent` theme repository | manager, skill, TUI state |
| Startup theme selection | `[tui].theme` | session transcript, skill context |
| Current rendered colors | existing `NeoChromeState/TuiTheme` | manager, repository |
| Manager selection/filter/preview state | `ThemeManagerState` | repository, config |
| AI draft validation and save | repository-backed `ThemeDraft` adapter | ordinary `Write`, skill text |
| Preview samples | shared pure `ThemePreviewRenderer` | persistent theme store |

### 7.2 Repository extension

`crates/neo-agent/src/themes.rs` SHALL grow from a startup loader into the
canonical repository boundary. Its responsibilities include:

- list descriptors and parse valid/invalid entries;
- resolve a `ThemeId` or exact display name;
- validate a complete theme payload;
- materialize base plus overrides for AI drafts;
- import and copy into the managed directory;
- atomically write a canonical JSON document;
- delete a managed file after controller confirmation;
- provide the startup resolver used by `AppConfig::load`.

The repository SHALL not expose arbitrary path writes. Import accepts an
external input path only as a read source; the destination is generated inside
the managed theme directory.

### 7.3 Manager ownership

`ThemeManagerState` is a transient `neo-tui` overlay state. It contains no
filesystem handles, config writer, or long-lived theme catalog authority. It
receives a snapshot and emits typed actions. The interactive controller maps
those actions to repository/config/runtime operations and returns a refreshed
snapshot.

### 7.4 AI tool dependency direction

`ThemeDraft` is implemented by `neo-agent`, which already owns the Neo Home
path and theme repository. It implements the existing core `Tool` contract and
is registered through the existing `ToolRegistry::register` path in the main
runtime construction. This avoids a `neo-agent-core -> neo-agent` dependency.

The tool is not added to child/delegate registries. A child agent must not gain
a new theme persistence capability merely because the parent runtime has the
skill available.

## 8. Startup and Configuration

### 8.1 Config schema

Add an optional field to the existing `FileTuiConfig`:

```toml
[tui]
theme = "solarized-dark.json"
```

The serialized value is a validated logical `ThemeId`. No absolute path is
accepted as the persisted source of truth.

### 8.2 Resolution rules

- If `[tui].theme` is present, resolve that exact id.
- If the field is absent, preserve the existing sorted-first JSON discovery as
  a bounded compatibility exception.
- Once the user sets an explicit startup default, sorted discovery is no
  longer a fallback for that configuration path.
- If an explicit id is missing or invalid, start with built-in
  `TuiTheme::default()` and emit a visible diagnostic. Do not silently select
  another JSON file and do not rewrite the invalid config automatically.
- A malformed individual theme file does not prevent the manager from listing
  other valid files.

### 8.3 Config refresh

`refresh_config()` SHALL continue to reload config and other derived settings.
It SHALL preserve the current runtime theme while a session override marker is
present. An explicit user action such as Reload startup default clears that
marker and applies the resolved config theme.

Set startup default uses the existing `update_file_config` lock and atomic
replacement path. If the write fails, the current runtime and previous config
remain unchanged.

## 9. Slash and Command-Palette Contract

### 9.1 Grammar

```text
theme_picker := "/theme" whitespace*
theme_apply  := "/theme" whitespace+ name_or_id
```

The parser trims boundary whitespace but keeps the argument as one exact value.
Only the lowercase command is special. `/themeish` and embedded prose remain
ordinary prompts.

Examples:

| Input | Result |
| --- | --- |
| `/theme` | open manager when idle |
| `/theme   ` | open manager when idle |
| `/theme solarized-dark.json` | apply exact ThemeId |
| `/theme Solarized Dark` | apply exact display name if unique |
| `/theme missing` | local not-found error |
| `/theme ambiguous-name` | local ambiguity error |
| `/theme` while busy | keep turn active and show manager-idle requirement |

`/theme <name-or-id>` is allowed during a running model turn because it only
changes render state. Bare `/theme` is blocking and therefore requires an idle
main turn.

### 9.2 Completion

Completion SHALL expose `/theme` and live exact catalog candidates. It SHALL
not write files, mutate config, or apply a theme. Completion uses stable ids
and display names and does not invent fuzzy direct-application semantics.

### 9.3 Command palette

Add a “Manage themes” command that opens the same manager as bare `/theme`.
It is an alternate entry point, not a replacement for the documented slash
command.

## 10. Theme Manager UI

### 10.1 State contract

`ThemeManagerState` contains:

- catalog snapshot;
- filter text and filtered indices;
- selected `ThemeId`;
- focus (`List`, `Preview`, `Actions`, or `Filter`);
- selected preview value;
- pending action and status/error message.

Its output is a typed `ThemeManagerAction`:

```text
ApplySession(ThemeId)
SetStartupDefault(ThemeId)
Import(path, conflict_policy)
Duplicate(ThemeId, new_display_name)
Delete(ThemeId)
Refresh
Close
```

No action directly writes a file from `neo-tui`.

### 10.2 Responsive layout

- Wide: width `>= 100`, height `>= 18`; left list about 34%, right preview
  about 66%, fixed action bar.
- Medium: width `68..99`; list and preview stack vertically.
- Narrow: width `< 68`; render one focused panel at a time and show the focus
  in the title line.
- Very short terminals retain the title, focus, status, and essential action;
  preview content is clipped safely and never overflows.

All rows use visible-width accounting. CJK, long names, status text, and paths
cannot exceed the available row width. v1 does not add a persisted description
field; the manager uses name, id, source, and status data.

### 10.3 List and preview

The list is sorted by display name, then `ThemeId`. It marks current active,
startup default, and invalid entries. Invalid entries can be inspected and
removed/replaced but cannot be applied or made default.

Selection only changes the local preview. The preview uses a shared renderer
with these samples:

- welcome/banner;
- user and assistant messages;
- tool status and working footer;
- diff added, removed, hunk, and context;
- approval border/title/selection;
- footer permission and context states.

The preview renderer accepts a `TuiTheme` value and sample model. It does not
read the transcript, mutate the chrome, or append a session event.

### 10.4 Input contract

- `Up/Down`, `j/k`: move selection.
- `PageUp/PageDown`, `Home/End`: page and boundary navigation.
- `/`: focus filter; `Enter` commits; `Backspace` removes; `Esc` clears first,
  then closes.
- `Tab`/`Shift+Tab`: cycle focus.
- `Enter` on list or preview: Apply for session and close.
- `D`: Set startup default; the current session remains unchanged.
- `I`: import path dialog.
- `C`: copy dialog.
- `X`: deletion confirmation.
- `R`: rescan and reparse catalog.
- `Esc`: close when no filter is active.

The action bar exposes equivalent actions for users who do not use shortcuts.

### 10.5 Mutation outcomes

After import or copy, re-scan the catalog and select the new id without
applying it. After delete, select the nearest remaining stable item. After
refresh, restore the previous id if it still exists; do not jump to the first
item without cause.

Delete is disabled for the current active or startup-default id. The user must
first apply another theme or set another startup default. The repository
re-checks this condition under its lock before deleting.

## 11. Theme Repository Mutations and Safety

Every mutation follows this sequence:

1. acquire the existing Neo Home/theme-directory lock;
2. re-scan and re-check the target against current disk state;
3. read and strictly validate external or source content in memory;
4. reject traversal, symlink/reparse, invalid name, or out-of-root target;
5. write a temp file inside the theme directory;
6. atomically replace/commit through the existing helper;
7. re-scan and return a new catalog snapshot.

Import is copy-and-archive. The outside path is never stored. Overwrite and
save-as-new are explicit conflict choices. A failed operation leaves the old
file and config intact.

The implementation SHALL use `Path`/`PathBuf` and platform-safe standard
library operations. It SHALL not assume Unix signals, permission bits, or
shell commands for file operations.

## 12. `custom-theme` Skill

Replace the existing stub at
`crates/neo-agent-core/src/skills/builtin/custom-theme.md` with a full
explicit-only skill package.

### 12.1 Interview flow

The skill asks one question per turn, covering only information needed to
construct the theme:

- create from default, modify an existing ThemeId, or revise a previous draft;
- light/dark and background/surface direction;
- brand/accent intent;
- primary/muted text contrast;
- status success/warning/error/pending/cancelled distinction;
- user-message, diff, selection, approval, footer, and shell readability;
- desired display name and conflict behavior.

The skill explains semantic roles in user terms and must not ask the user to
know internal Rust field names. It uses current token names in the structured
payload and rejects retired token names.

### 12.2 Draft policy

The skill builds a `ThemeDraft.preview` request with an optional base ThemeId
and role-color overrides. It waits for the structured preview result and
explains any contrast warnings. It does not write a JSON file itself.

After preview it must say that the preview is non-mutating and ask whether to
save. “Modify” starts another preview. “Save” is the only path to
`ThemeDraft.save`. The skill never calls an apply operation.

If the tool is unavailable, the skill reports the missing capability and stops;
it does not use ordinary Write as a fallback.

## 13. `ThemeDraft` Tool Contract

### 13.1 Registration and scope

Implement the tool in `neo-agent` and register it in the existing production
registry construction. It is not a generic filesystem tool and is excluded
from child/delegate registries.

The tool uses the repository and an `Arc<Mutex<BoundedDraftStore>>` shared by
the interactive session runtime. One bounded store is created when the
interactive controller starts and is threaded through every turn's runtime, so
a `ThemeDraft.preview` in one turn can be saved in a later turn of the same
session. The store keeps a bounded number of canonical drafts and expires with
the interactive session runtime (spans turns).

### 13.2 Input

The tool uses a typed tagged input shape equivalent to:

```json
{
  "action": "preview",
  "name": "Aurora Night",
  "base_theme": "default.json",
  "colors": {
    "text_primary": "#e6edf3",
    "brand": "#58a6ff",
    "status_ok": "#3fb950"
  }
}
```

or:

```json
{
  "action": "save",
  "draft_id": "draft-...",
  "overwrite": false
}
```

The actual Rust type SHALL use the existing typed JSON schema convention. The
implementation may choose a stable generated id format; callers must treat it
as opaque.

`preview` validation includes:

- non-empty bounded display name;
- no control characters, separators, or platform-reserved target name;
- valid optional base ThemeId;
- current semantic color-token allowlist;
- valid named or hex colors supported by the existing parser;
- no unknown JSON fields.

### 13.3 Preview

Preview loads the base theme or built-in default, applies overrides, materializes
an independent canonical role-color set, computes a stable fingerprint, stores
the draft, and returns:

```json
{
  "kind": "theme_draft_preview",
  "draft_id": "draft-...",
  "fingerprint": "sha256:...",
  "display_name": "Aurora Night",
  "candidate_theme_id": "aurora-night.json",
  "base_theme_id": "default.json",
  "normalized_colors": {},
  "contrast_warnings": []
}
```

Preview has no persistent side effect and should not require a normal write
approval. The normalized content is the only content save can later persist.

### 13.4 Save

Save looks up `draft_id`, revalidates the stored canonical payload and current
catalog, and writes only inside `$NEO_HOME/themes/`.

- Missing/expired id: terminal error, no write.
- Existing destination with `overwrite=false`: typed conflict, no write.
- Existing destination with `overwrite=true`: explicit overwrite path and
  normal Ask-mode one-time approval, no session-wide mutation approval.
- New destination: strict validation, lock, atomic commit.
- Success: return logical ThemeId, fingerprint, and `applied: false`.

A save request cannot provide new colors, a new path, or a different name. A
new name requires a new preview. The tool never calls runtime theme apply.

### 13.5 Permission behavior

- Preview is a non-mutating tool action.
- Save is a special mutation action. In Ask mode it requires an approval for
  that save and does not offer a broad session approval for future theme saves.
- Auto and yolo follow the existing permission-mode contract. The skill's
  explicit conversational confirmation remains mandatory before it issues save.
- Plan mode permits preview and denies save.
- Tool access does not grant arbitrary `file_write`; the repository itself
  enforces the only writable root.

### 13.6 Structured card

The existing tool-result presentation gains a `theme_draft_preview` details
branch using `ThemePreviewRenderer`. The card is non-blocking and includes the
name, status, color samples, representative TUI samples, and warnings. It is
not an interactive manager and has no Apply action.

## 14. Error Handling

- A bad theme file is an invalid catalog entry, not a reason to hide valid
  themes.
- Direct apply errors are local and side-effect free.
- Manager mutation errors keep the manager open with the current selection and
  a retryable status.
- Concurrent changes trigger a rescan and require a fresh selection.
- Config mutation errors do not alter current runtime colors.
- ThemeDraft errors have stable categories for the skill: invalid input,
  missing base, conflict, expired draft, permission denied, plan blocked, and
  atomic write failure.
- No error result may be described as a successful save or apply.

Theme JSON is untrusted data. Its `name` and colors are presentation data only;
no file content is interpreted as instructions or context injection.

## 15. Context Integrity and Runtime Safety

Applying a manual theme updates only the existing render owner and its caches.
It does not append a user message, modify transcript records, alter session
metadata, or rewrite model context.

AI preview and save tool calls use the existing append-only event pipeline. They
are ordinary new events; they do not rewrite historical messages or any cache
prefix. The feature must leave micro compaction and snip+dedup default-off
behavior unchanged.

A live `/theme <name>` apply is safe during a model turn because it changes
only render state. Bare `/theme` remains blocking and is rejected while the
main turn is busy.

## 16. Documentation Synchronization

Update both English and Chinese versions of:

- `docs/user_guide/*/customization/themes.md`;
- `docs/user_guide/*/reference/slash-commands.md`;
- `docs/user_guide/*/configuration/config-files.md`;
- `docs/user_guide/*/configuration/data-locations.md`;
- `docs/user_guide/*/customization/skills.md`.

The documentation must state:

- theme files live under `$NEO_HOME/themes/`;
- `[tui].theme` stores a logical id;
- bare `/theme` opens the manager only when idle;
- `/theme <name-or-id>` applies the current session immediately;
- Set startup default is independent and persistent;
- import copies and validates, with explicit conflict handling;
- deletion protects active and startup-default themes;
- `custom-theme` is explicit-only, previews before save, does not auto-apply,
  and hands application back to `/theme`.

Remove the stub's project-local path, ordinary manual-edit instructions, and
claim that `custom-theme` directly activates a theme.

## 17. Verification Matrix

### 17.1 Repository and configuration

- semantic token parsing and unknown-key rejection;
- exact ThemeId normalization and safe path conversion;
- CJK/long name and platform-reserved name handling;
- symlink/reparse and traversal rejection;
- explicit configured theme resolution;
- old no-field sorted discovery;
- explicit missing/invalid fallback to built-in with diagnostic and no sorted
  fallback;
- import archive, overwrite/save-as-new, copy, delete guards;
- atomic failure leaves old file/config unchanged;
- `[tui].theme` mutation uses the existing lock/helper;
- session override survives unrelated `refresh_config()`.

### 17.2 Slash, runtime, and manager

- bare/direct parser and completion behavior;
- direct exact id/display-name resolution and ambiguity;
- direct apply while busy;
- bare manager rejection while busy;
- command-palette entry;
- wide, medium, narrow, and short-terminal rendering;
- visible-width safety for CJK and long values;
- filter/focus/navigation/action mapping;
- selection preview does not mutate runtime;
- Apply changes chrome/transcript theme;
- Set startup default leaves current session unchanged;
- catalog refresh restores stable selection;
- import/copy/delete action results and guards.

### 17.3 ThemeDraft and skill

- explicit-only skill metadata and documented flow;
- preview base materialization, strict overrides, stable fingerprint, bounded
  draft store, and contrast warnings;
- save only by draft id;
- expired draft, conflict, explicit overwrite, and atomic failure;
- arbitrary path and invalid-token rejection;
- Ask/auto/yolo/plan permission behavior;
- no child/delegate registration;
- structured preview card uses the shared renderer;
- successful save reports `applied: false` and does not change `TuiTheme`;
- context prefix and append-only event invariants remain intact.

## 18. Compatibility and Retirement Boundary

### Retained compatibility

- Existing users with no `[tui].theme` retain sorted-first startup discovery.
- Existing valid semantic theme JSON remains loadable.

### Retired behavior

- No explicit-config sorted fallback after `[tui].theme` exists.
- No project-local `.neo/themes` path for this feature.
- No direct ordinary-Write implementation of `custom-theme`.
- No skill behavior that silently applies a generated theme.
- No duplicate theme parser, durable store, or runtime color owner.
- No acceptance of retired theme keys such as `accent`.

### Retirement verification

The test and documentation suite must prove that explicit configuration does
not re-enter sorted discovery and that the former custom-theme stub semantics
are absent from the user-visible skill instructions.

## 19. ADR and Baseline Alignment Signals

This design has an ADR signal for:

- canonical theme repository and runtime owner;
- public `/theme` grammar and busy behavior;
- `[tui].theme` configuration source-of-truth;
- `ThemeDraft` dependency direction and host registration;
- dedicated save permission semantics;
- bounded legacy fallback retirement.

`Baseline Role Alignment` for this design is:

- Product / Requirement Baseline: aligned with the user's confirmed manager,
  persistence, layout, import, busy-turn, compatibility, and AI-save choices.
- Architecture / Runtime Boundary Baseline: aligned with existing theme,
  config, TUI, tool registry, permission, and skill owners; new manager and
  draft surfaces have explicit add-with-proof justification.
- Scope: `both`.
- Authority gap: `docs/current/AEGIS_MINIMALITY_REFERENCE.md` is missing and
  must not be treated as silently satisfied.

## 20. Aegis Working Artifacts

### TaskIntentDraft

- Outcome: deliver a full manual theme manager and a separate AI-assisted
  custom-theme creation path.
- Goal: make the documented `/theme` command real without duplicating theme
  ownership or weakening local file safety.
- Success evidence: command/UI/runtime/config/repository/skill/tool/docs/tests
  cover the accepted behaviors in this spec.
- Stop condition: this spec is user-reviewed and approved before
  implementation planning.
- Non-goals: hosted sync, project-local theme stores, automatic AI apply, and
  context/session theme persistence.
- Risks: owner duplication, explicit-config fallback drift, external path write,
  stale preview/save content, and narrow-terminal overflow.

### BaselineReadSetHint

- `crates/neo-agent/src/themes.rs`;
- `crates/neo-tui/src/primitive/theme.rs`;
- `crates/neo-tui/src/shell/state.rs`;
- `crates/neo-tui/src/transcript/pane.rs`;
- `crates/neo-agent/src/config/loader.rs`;
- `crates/neo-agent/src/config/mod.rs`;
- `crates/neo-agent/src/config/mutations.rs`;
- `crates/neo-agent/src/modes/interactive/slash_commands.rs`;
- `crates/neo-agent/src/modes/interactive/command_palette.rs`;
- `crates/neo-agent/src/modes/run/runtime/agent.rs`;
- `crates/neo-agent-core/src/tools/mod.rs`;
- `crates/neo-agent-core/src/runtime/permission.rs`;
- `crates/neo-agent-core/src/skills/builtin/custom-theme.md`;
- the English/Chinese themes, slash, config, data-location, and skills guides.

### BaselineUsageDraft

- Required baseline refs: existing theme loader/model/runtime propagation,
  config mutation/refresh, slash/command palette, ToolRegistry/permission,
  skill package, and user guides.
- Delivered context refs: codegraph exploration, prior TUI/owner/test surveys,
  current ToolRegistry registration and permission classification.
- Acknowledged before plan refs: `docs/aegis/README.md`,
  `docs/aegis/BASELINE-GOVERNANCE.md`, and the current design evidence above.
- Cited in design refs: paths and owner table in Sections 5-7 and the test/docs
  boundaries in Sections 16-17.
- Missing refs: `docs/current/AEGIS_MINIMALITY_REFERENCE.md`.
- Decision: `continue`, with the missing authority explicitly recorded.

### ImpactStatementDraft

- Affected layers: `neo-agent` repository/config/controller, `neo-tui` manager
  and preview renderer, `neo-agent-core` tool/permission contracts, built-in
  skill content, documentation, and targeted tests.
- Canonical owners: repository for files, config for startup id, chrome/TuiTheme
  for current render value, manager for transient UI, ThemeDraft adapter for
  AI save intent.
- Invariants: no duplicate durable store, no arbitrary external write, no
  session/context mutation for render-only apply, no explicit-config sorted
  fallback, no AI auto-apply.
- Compatibility: absent `[tui].theme` retains sorted discovery; explicit field
  retires it for that path.
- Non-goals: hosted sync, project-local themes, descriptions/extends schema,
  history/favorites, and child-agent theme persistence.

## 21. Approval Transition

The four design sections were approved interactively by the user:

1. Canonical owner and full data flow.
2. `/theme` manager UI and action contract.
3. `custom-theme` and `ThemeDraft` tool contract.
4. Persistence, compatibility, verification, and delivery contract.

This written spec now requires a fresh user review for wording, omissions, and
implementation-boundary corrections. Implementation planning must not begin
until that review is complete.
