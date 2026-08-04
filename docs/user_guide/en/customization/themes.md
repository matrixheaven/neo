# Themes

The Neo TUI color scheme is defined by the `TuiTheme` struct (see `crates/neo-tui/src/primitive/theme.rs`) and can be overridden via JSON theme files. `$NEO_HOME/themes/` (by default `~/.neo/themes/`) is the **only** managed theme directory: drop a `.json` file there and it becomes a catalog entry the manager, `/theme`, and startup resolution can use. [`examples/config/magenta-dark.json`](../../../../examples/config/magenta-dark.json) shows the file shape — note that example uses legacy keys; see the note under the token table below.

## JSON Theme Format

A theme file is a top-level object; under `colors`, each key corresponds to a semantic color token and the value is a color string:

```json
{
  "name": "magenta-dark",
  "colors": {
    "brand": "#C678DD",
    "status_ok": "#4EC87E",
    "status_error": "#E85454"
  }
}
```

| Field | Description |
| --- | --- |
| `name` | Optional; defaults to the file name stem when omitted |
| `colors` | Color token table; all keys are optional, missing ones fall back to the default theme |

Color values support three forms:

| Form | Example | Description |
| --- | --- | --- |
| `#RRGGBB` | `"#C678DD"` | 24-bit true color, recommended |
| Named color | `"darkgray"` | ANSI named color |
| `reset` | `"reset"` | Follow the terminal default |

> The loader is strict about unknown keys (`deny_unknown_fields`); a misspelled token name will cause loading to fail outright. Align precisely with the table below.

## Color Token Table

| Token | Default usage |
| --- | --- |
| `text_primary` | Body text |
| `prompt` | Prompt / input box foreground |
| `brand` | Brand color (overlay borders, selection highlight) |
| `status_ok` | Success state |
| `status_error` | Error / danger |
| `status_warn` | Warning / approval title |
| `status_pending` | Pending state |
| `status_cancelled` | Cancelled state |
| `text_muted` | Secondary / gray text |
| `user_message` | User message color |
| `diff_added` | Diff added lines |
| `diff_removed` | Diff removed lines |
| `diff_hunk` | Diff hunk header |
| `diff_context` | Diff context lines |
| `selection_bg` | Selection background |
| `approval_border` | Approval dialog border |
| `selected_fg` / `selected_bg` | Selected item foreground / background |
| `overlay_border` | Overlay border |
| `footer_permission_allow` | Footer: allow |
| `footer_permission_ask` | Footer: ask |
| `footer_permission_deny` | Footer: deny |
| `footer_working` | Footer: working |
| `footer_context_ok` | Footer: context sufficient |
| `footer_context_warn` | Footer: context warning |
| `footer_context_critical` | Footer: context critical |
| `shell_mode` | Shell mode indicator color |

> Note: `examples/config/magenta-dark.json` uses `accent` / `success` / `danger`, which are legacy aliases **no longer recognized by the current loader**. Use the new keys like `brand` / `status_ok` / `status_error` from the table above. The example below uses the new schema.

## Example

A complete dark theme (`~/.neo/themes/magenta-dark.json`):

```json
{
  "name": "magenta-dark",
  "colors": {
    "brand": "#C678DD",
    "status_ok": "#4EC87E",
    "status_error": "#E85454",
    "status_warn": "#E8A838",
    "text_muted": "#8B949A",
    "text_primary": "#C6D0F5",
    "prompt": "#C6D0F5",
    "user_message": "#E5C890",
    "diff_added": "#4EC87E",
    "diff_removed": "#E85454",
    "diff_hunk": "#E8A838",
    "diff_context": "#8B949A",
    "footer_permission_ask": "#C678DD",
    "footer_working": "#C678DD"
  }
}
```

Theme repository (`crates/neo-agent/src/themes.rs`):

- `$NEO_HOME/themes/` is the only theme location; every `*.json` file there is a catalog entry, and a malformed file is an invalid entry rather than a reason to hide the other valid themes.
- `[tui].theme` is a **logical id** relative to `$NEO_HOME/themes/`, never an absolute path. At startup Neo resolves that exact id; if it is missing or invalid, Neo starts with the built-in `TuiTheme::default()` and emits a visible diagnostic — it does not silently pick another JSON file and does not rewrite the config.
- If `[tui].theme` is absent, Neo keeps the legacy sorted-first discovery (the first `.json` file by name) as a bounded compatibility fallback.
- Parse failures are reported and never silently fall back.

See the [`examples/config/`](../../../../examples/config/) directory for more examples.

## /theme Command

| Form | Behavior |
| --- | --- |
| `/theme` | Open the theme manager. This requires the main turn to be idle; while a turn is running Neo keeps the turn active and shows the idle requirement. |
| `/theme <name-or-id>` | Apply the theme to the **current session** immediately, including while a model turn is running. Resolution is exact: the logical `ThemeId` first, then a unique exact display name. There is no fuzzy or prefix matching — an unknown or ambiguous name produces a local error. |
| `/theme reload` | Clear the current session override and re-apply the theme resolved from `[tui].theme`. |

`/theme <name-or-id>` changes the current session only — it does not write `config.toml` and does not change the startup default.

### Theme manager

Bare `/theme` opens a manager with list, filter, and preview panels. Selecting an entry only previews it; nothing applies until you choose an action:

| Action | Effect |
| --- | --- |
| Apply for session | Switches the current TUI session to the selected theme; no config write. The session override survives unrelated config refreshes. |
| Set startup default | Writes the logical id to `[tui].theme`; the current session is unchanged. |
| Import | Validates and copies an external theme file into `$NEO_HOME/themes/`. A same-name destination requires an explicit choice — Overwrite or Save as new; there is no silent overwrite. |
| Copy | Duplicates the selected theme under a new display name. |
| Delete | Removes a managed theme after confirmation. The currently active theme and the startup default are protected until you apply or set another one. |
| Refresh | Rescans `$NEO_HOME/themes/` and reparses the catalog. |

The manager adapts to narrow terminals by rendering one focused panel at a time, with the focus shown in the title line.

### Startup default

At startup the theme is resolved from `[tui].theme` (see [Configuration Files](../configuration/config-files.md)); when the field is absent and no JSON file exists, the built-in `TuiTheme::default()` (magenta dark) is used. To create a theme with AI assistance, use the explicit-only `custom-theme` skill — it previews before saving, never auto-applies, and hands application back to `/theme`. See [Skills](skills.md).

## Next Steps

- [Skills](skills.md) — The full flow of the `custom-theme` skill
- [Configuration Files Overview](../configuration/config-files.md) — Theme directory location
- [Interaction Guide](../guides/interaction.md) — TUI regions and color meanings
