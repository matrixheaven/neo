---
name: custom-theme
description: Guide the user through creating or revising a custom TUI color theme via the explicit ThemeDraft preview/save flow.
disableModelInvocation: true
---

# Custom Theme

Create or revise a custom TUI color theme. This skill runs only when the user
explicitly activates it with `/skill:custom-theme`; it is never invoked
automatically, and you must not start this workflow on your own.

## Role split

The host owns themes. You are the interviewer and translator: you turn the
user's plain-language color preferences into one structured semantic-token
`ThemeDraft` call. `ThemeDraft.save` is the only persistence path; there is no
theme file you may write by hand.

## Capability check first

Before asking anything, confirm that the `ThemeDraft` tool is registered in
the current runtime. If it is unavailable, tell the user that custom-theme
creation is not supported in this session and stop. Never fall back to a plain
`Write` call, hand-edited JSON, or a project-local theme file.

## Interview: one focused question per turn

Ask exactly one focused question per turn and wait for the answer before
continuing, in this order:

1. **Base and direction.** New theme from the built-in default, or revise an
   existing saved theme? The base theme id (for example `default.json` under
   `$NEO_HOME/themes/`, or any other saved theme) decides the light/dark and
   background/surface direction; start from a dark base for a dark theme.
2. **Brand color.** The accent color used for highlights and focus.
3. **Text contrast.** Normal body text, muted/secondary text, and the input
   prompt text.
4. **Status roles.** Success, error, warning, pending, and cancelled.
5. **Readability surfaces.** User message text; diff added, removed, hunk, and
   context lines; selection and selected-item colors; approval dialog border;
   footer indicators (permission allow/ask/deny, working, context
   ok/warn/critical); and the shell-mode indicator.
6. **Display name.** Short, 1-64 characters, no `/` or `\` separators and no
   control characters; the saved theme id is derived from this name.

Translate each answer into the matching semantic tokens for the preview call.
Use the canonical token vocabulary at the end of this skill; never ask the
user for internal field names.

## Preview (non-mutating)

Once the interview is complete, issue a single structured `ThemeDraft` call
with `action: "preview"`, the display name, the chosen base theme id (omit it
for the built-in default), and the `colors` map of semantic-token overrides.
Do not preview token-by-token; the interview answers go into one preview.

Preview only materializes a draft in memory: it writes no files and changes
nothing on screen. Present the returned candidate theme id, normalized colors,
and any contrast warnings to the user honestly. Warnings are not a save; never
present a preview result as if the theme were saved.

If the user changes their mind, run a fresh preview with the updated name or
colors. Every modification starts a new preview; drafts are never edited in
place.

## Confirm, then save

After the preview, ask explicitly for confirmation to save. Only after a clear
conversational yes, call `ThemeDraft` with `action: "save"` and the `draft_id`
returned by the preview, plus `overwrite` only when replacing an existing
theme (see Conflicts).

A save request never carries a name, colors, or a path: changing any of them
requires a new preview. Save is the only persistence path.

## Conflicts

If save reports that the theme already exists, do not overwrite silently. Ask
the user explicitly whether to replace the existing theme. On yes, save again
with `overwrite: true`. On no, start a new preview with a different display
name.

## After saving

A successful save reports `applied: false`: the theme is stored under
`$NEO_HOME/themes/` but the running session does not change. Tell the user to
apply it later with `/theme <ThemeId>` (for example `/theme my-theme.json`).
Saving never applies the theme itself.

## Semantic-token vocabulary

Map the user's preferences to these canonical tokens in the `colors` map:

| Token | What it colors |
| --- | --- |
| `text_primary` | primary body text |
| `text_muted` | secondary text (timestamps, metadata) |
| `prompt` | text in the input prompt line |
| `brand` | accent color for highlights and focus |
| `status_ok` | success indicators |
| `status_error` | error indicators |
| `status_warn` | warning indicators |
| `status_pending` | pending indicators |
| `status_cancelled` | cancelled indicators |
| `user_message` | user message text |
| `diff_added` | added lines in diffs |
| `diff_removed` | removed lines in diffs |
| `diff_hunk` | hunk headers in diffs |
| `diff_context` | unchanged context lines in diffs |
| `selection_bg` | list and menu selection background |
| `selected_fg` | selected item text |
| `selected_bg` | selected item background |
| `approval_border` | approval dialog border |
| `overlay_border` | overlay and modal border |
| `footer_permission_allow` | footer "allowed" indicator |
| `footer_permission_ask` | footer "ask" indicator |
| `footer_permission_deny` | footer "denied" indicator |
| `footer_working` | footer working state |
| `footer_context_ok` | footer context healthy indicator |
| `footer_context_warn` | footer context warning indicator |
| `footer_context_critical` | footer context critical indicator |
| `shell_mode` | shell-mode indicator |
