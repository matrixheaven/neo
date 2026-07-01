# Local Prompt And Theme Assets

Neo can load prompt templates and themes from local project or user
configuration directories. This page does not define a hosted catalog, publish
flow, publisher identity system, or root trust chain as a supported local-agent
feature.

## Prompt Templates

Project prompt templates live under `.neo/prompts/*.md`; user-global templates
live under `~/.neo/prompts/*.md`. Neo can also load configured selectors from
`prompt_templates` in config or explicit `--prompt-template` flags.

```bash
neo prompts list
neo prompts preview review
neo --prompt-template review print src/lib.rs
```

Template directories from explicit selectors are non-recursive. Auto-discovered
user prompt templates are selected by slash name during `print`, `run`, RPC
prompts, and TUI turns.

## Themes

Themes live under `~/.neo/themes` (or `$NEO_HOME/themes`):

```bash
neo themes list
neo themes preview night-owl
neo --theme ~/.neo/themes/night-owl.json
```

## Out Of Scope

Neo's local-only docs intentionally omit marketplace search/install/publish,
hosted package registries, package accounts, publisher identity binding,
transparency logs, and root trust anchors. If package archive validation code is
present in the repository, treat it as an implementation detail until the
product chooses a supported distribution model.
