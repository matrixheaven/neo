# Themes

The Neo TUI color scheme is defined by the `TuiTheme` struct (see `crates/neo-tui/src/primitive/theme.rs`) and can be overridden via JSON theme files. Drop a `.json` file into `~/.neo/themes/` to have it discovered and loaded. Example: [`examples/config/magenta-dark.json`](../../../examples/config/magenta-dark.json).

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

Loading mechanism (`crates/neo-agent/src/themes.rs`):

- Scans all `.json` files under `~/.neo/themes/`, sorted by file name, and takes the first;
- Relative paths are resolved against `$NEO_HOME`; `~/` expansion is supported;
- Parse failures are reported at startup and do not silently fall back.

See the [`examples/config/`](../../../examples/config/) directory for more examples.

## /theme Command

| Action | Description |
| --- | --- |
| `/theme <name>` | Switch to `~/.neo/themes/<name>.json` |
| `custom-theme` skill | Interactive guide: pick base color → pick token → preview → save (`/skill:custom-theme`) |

Theme switching takes effect immediately within the interactive TUI; at startup the default theme is decided by `resolve_theme()`, and when no JSON file is found the built-in `TuiTheme::default()` (magenta dark) is used.

## Next Steps

- [Skills](skills.md) — The full flow of the `custom-theme` skill
- [Configuration Files Overview](../configuration/config-files.md) — Theme directory location
- [Interaction Guide](../guides/interaction.md) — TUI regions and color meanings
