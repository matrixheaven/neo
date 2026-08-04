# Theme Manager and Custom Theme — Landed Baseline

Status: `recorded-from-work`
Date: `2026-08-04`
ADR: `docs/aegis/adr/ADR-0011-theme-manager-and-custom-theme.md`

This baseline records the landed theme manager and AI-assisted custom-theme
implementation, in task order:

- `baf6fb39` — `feat: add canonical theme repository and startup selection`
  (validated `ThemeId`, catalog with valid/invalid entries, exact id/unique
  display-name resolution, canonical materialization, atomic import/copy/
  delete/overwrite/save-as-new under the managed-directory lock with symlink
  containment, optional `[tui].theme` logical id, explicit/absent/invalid
  startup branches, `set_startup_theme` via `update_file_config`);
- `a3e8e5d6` — `feat: add responsive theme manager overlay and preview renderer`
  (pure `ThemePreviewRenderer` shared by manager and tool card; transient
  `ThemeManagerState` with snapshot/action contract, wide/medium/narrow/short
  layouts, visible-width-safe CJK rows, single-action-per-keystroke);
- `4efc740e` — `feat: wire theme manager actions and slash command`
  (controller adapter, `/theme` idle-only bare vs exact direct apply while busy,
  session override marker preserved by `refresh_config()`, Reload startup
  default, read-only completion, `theme.manager` palette command);
- `58dd4878` — `feat: add guarded ThemeDraft preview and save tool`
  (root-only registration, typed preview/save, bounded draft store, typed
  `ThemeSave` permission with plan-deny and one-time no-session approval, no
  `file_write` grant, `theme_draft_preview` tool card via shared renderer);
- `5380f1fb` — `docs: define explicit custom-theme skill workflow`
  (explicit-only metadata, one-question interview, preview → confirm → save,
  no Write/project-local fallback, no auto-apply);
- `7dc1dbf0` — `docs: synchronize theme manager guidance`
  (English/Chinese guide parity, spec status line updated);
- `49b4def0` — `fix: scope theme drafts to the interactive session`
  (draft store lifted from per-turn to session scope so multi-turn
  preview → confirm → save works; spec §13.1 wording aligned).

## Compatibility Boundary

Retained: no-field `[tui].theme` keeps sorted-first startup discovery as a
bounded compatibility exception; valid semantic-token JSON remains loadable;
retired tokens such as `accent` stay rejected.

Retired: explicit-config sorted fallback, project-local `.neo/themes` claims,
ordinary-Write AI theme saving, silent AI auto-apply, and duplicate
parser/store/runtime-color owners.

## Verification Evidence

Focused regression suites (exact `cargo test` filters; `cargo nextest` not
installed in this environment):

- `-p neo-agent --bin neo themes::` — 21 passed (repository, ThemeId safety,
  catalog, materialization, mutations, resolution).
- `-p neo-agent --bin neo modes::interactive::tests::theme_` — 19 passed
  (slash grammar, busy/idle, override/refresh/reload, completion, palette,
  manager actions, gap tests).
- `-p neo-agent --test cli_commands theme` — 5 passed.
- `-p neo-tui --test theme_manager` — 18 passed (responsive layouts, CJK width
  safety, single-action contract).
- `-p neo-tui --test tool_cards theme_draft_preview` — 2 passed.
- `-p neo-agent-core --test tool_permissions theme_draft` — 1 passed;
  `runtime_turn theme_draft` — 4 passed; `multi_agent_roles theme_draft` —
  2 passed (permission, plan deny, no child registration).
- `-p neo-agent-core --test skills custom_theme` — 2 passed.
- `cargo fmt --all --check` and `git diff --check` clean.

## Authority Gap

`docs/current/AEGIS_MINIMALITY_REFERENCE.md` remains missing; recorded as an
authority gap in the design, plan, and this baseline. It is not treated as
satisfied.

## Residual Risk

Focused local tests do not prove every terminal implementation or native
Windows/Linux filesystem race. Windows symlink/junction fixtures may require
Developer Mode (existing repo convention). A full manual terminal walkthrough of
the manager and tool card was not performed in this environment.
