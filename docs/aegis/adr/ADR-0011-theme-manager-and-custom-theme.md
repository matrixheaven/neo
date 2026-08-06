# ADR-0011 - Theme Repository, Manager, and Host-Guarded AI Theme Creation

Status: `accepted`
Date: `2026-08-04`

## Source Evidence

- Approved design: `docs/aegis/specs/2026-08-04-theme-manager-design.md`
  (commit `72f91320`, later status line updated in `7dc1dbf0`).
- Implementation plan: `docs/aegis/plans/2026-08-04-theme-manager.md`
  (commit `ae9d9129`).
- Landed implementation commits, in task order:
  - `baf6fb39` — `feat: add canonical theme repository and startup selection`;
  - `a3e8e5d6` — `feat: add responsive theme manager overlay and preview renderer`;
  - `4efc740e` — `feat: wire theme manager actions and slash command`;
  - `58dd4878` — `feat: add guarded ThemeDraft preview and save tool`;
  - `5380f1fb` — `docs: define explicit custom-theme skill workflow`;
  - `7dc1dbf0` — `docs: synchronize theme manager guidance`;
  - `49b4def0` — `fix: scope theme drafts to the interactive session`.
- Landed baseline: `docs/aegis/baseline/2026-08-04-theme-manager.md`.

## Context

The user guide described `/theme <name>` but the interactive slash router had no
implementation and no manager UI. The theme loader only discovered the first JSON
file at startup. The `custom-theme` skill stub described project-local paths,
manual file editing, and direct activation. The feature needed one canonical
theme-file owner, an explicit startup-selection contract, a full manual manager,
and a separate explicit-only AI creation path with a host-controlled preview and
save boundary.

## Decision

- **Canonical repository:** `crates/neo-agent/src/themes.rs` is the single owner
  of theme files under `$NEO_HOME/themes/`, the validated logical `ThemeId`
  boundary, catalog (valid + invalid entries), exact id/display-name resolution,
  canonical materialization, and atomic import/copy/delete/overwrite/save-as-new
  under the managed-directory lock with symlink/reparse containment.
- **Startup selection:** `[tui].theme` is an optional logical id. Explicit id
  resolves first; missing/invalid explicit ids fall back to the built-in default
  with a visible diagnostic and never re-enter sorted discovery; the legacy
  sorted-first discovery is retained only when the field is absent.
- **Runtime owner:** `NeoChromeState/TuiTheme` remains the only current render
  owner. Apply-for-session sets chrome and a controller-owned ephemeral override
  marker; unrelated `refresh_config()` calls preserve the override; an explicit
  Reload startup default clears it.
- **Manager:** `ThemeManagerState` is a transient TUI overlay that receives a
  snapshot and emits typed `ThemeManagerAction`s; `neo-tui` never reads the
  filesystem or config. `/theme` is idle-only; `/theme <name-or-id>` applies
  exactly (id first, then unique display name) even while busy.
- **AI creation:** `custom-theme` is explicit-only (`/skill:custom-theme`,
  `disableModelInvocation: true`). `ThemeDraft` is a host tool implemented in
  `neo-agent` (no `neo-agent-core -> neo-agent` dependency), registered only in
  the root interactive runtime, absent from Btw and child/delegate registries.
  Preview is non-mutating; save accepts only `draft_id` (+ `overwrite`) and
  reports `applied: false`; drafts are in-memory, bounded, and scoped to the
  interactive session so a multi-turn interview can confirm and save a later
  preview. The skill never falls back to ordinary `Write` and never auto-applies.
- **Permission:** a typed `ThemeSave` classification gives preview no write
  approval, save a one-time Ask approval with no session-wide theme-save grant,
  Auto/Yolo unchanged, plan mode denying save. No generic `file_write` is
  granted; the repository enforces the only writable root.

## Alternatives Considered

- Lightweight picker instead of a full manager: rejected because the two-pane
  preview, filter/focus model, invalid-entry handling, and mutation confirmations
  cannot be expressed by existing picker states.
- Project-local `.neo/themes` as a second location: rejected; single Neo Home is
  the source of truth.
- Ordinary `Write` for AI theme saving: rejected; it cannot bind previewed
  content to a stable draft id/fingerprint or enforce the theme-directory
  boundary.
- Per-turn draft store: rejected by cross-boundary review; it broke the
  multi-turn preview → confirm → save flow, so drafts are session-scoped.
- Second durable theme store or second `TuiTheme`: rejected; existing owners are
  canonical.

## Consequences

- `/theme` is now a real command; completion, command palette, and both guides
  describe the shipped behavior.
- Existing no-field users keep sorted-first startup as a bounded compatibility
  exception; explicit configuration never re-enters it.
- AI-created themes require explicit preview, confirmation, and later `/theme`
  application; save never mutates the current session, transcript, or context.
- Persistent-state operations (delete, overwrite, config default) remain
  confirmation-first with repository revalidation.
- Residual risks: live TUI interaction and Windows-specific junction behavior
  are covered by focused tests but not a manual full-terminal walkthrough;
  `docs/current/AEGIS_MINIMALITY_REFERENCE.md` remains missing and is recorded as
  an authority gap, not silently satisfied.

## Compatibility Boundary

Existing built-in themes, explicit startup selection, absent-field sorted
discovery, current-session application, config reload, transcript and context
semantics remain unchanged. AI theme save never applies a theme and never gains
generic file-write permission.

## Retirement Impact

The undocumented `/theme <name>` stub behavior, project-local theme directory,
ordinary Write-based AI save, per-turn draft store, and duplicate theme owner
are retired. The absent-field sorted discovery is the only bounded compatibility
exception and has an explicit startup-only scope.

## Baseline Sync

- Needed: `resolved`
- Target: `docs/aegis/baseline/2026-08-04-theme-manager.md`
- Action: current landed baseline exists
- Reason: repository, manager, startup selection, draft lifetime, permission,
  and confirmation boundaries are recorded there.

## Evidence References

- `docs/aegis/specs/2026-08-04-theme-manager-design.md`
- `docs/aegis/plans/2026-08-04-theme-manager.md`
- `docs/aegis/baseline/2026-08-04-theme-manager.md`
- Landed commits listed in Source Evidence

This ADR is an advisory Aegis Method Pack record. It does not grant completion authority or replace project-authoritative architecture sources.
