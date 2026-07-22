---
name: custom-theme
description: Help the user create or modify a custom TUI color theme.
disableModelInvocation: true
---

Help the user create or modify a custom TUI theme.

Themes live in `.neo/themes/<name>.json` or `~/.neo/themes/<name>.json`. The JSON should follow the theme schema.

Walk the user through:
1. Choosing a base theme (dark/light/auto).
2. Selecting semantic color tokens to override.
3. Previewing and saving the theme file.
4. Activating the theme via `/theme <name>` or config.

Do not write the theme file until the user confirms the palette choices.
