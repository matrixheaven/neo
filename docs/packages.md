# Local Extension And Prompt Assets

Neo's local-only documentation treats extensions, prompt templates, and themes
as files under the project or user configuration tree. It does not document a
hosted catalog, publish flow, publisher identity system, or root trust chain as
a supported local-agent feature.

## Extensions

Local extensions are directories that contain `neo-extension.toml`:

```toml
id = "echo"
name = "Echo"
version = "0.1.0"
description = "Local echo extension"

[runner]
command = "python3"
args = ["-u", "extension.py"]
```

Install and manage them with local commands:

```bash
neo extensions install path/to/extension
neo extensions list
neo extensions status echo
neo extensions disable echo
neo extensions enable echo
neo extensions update echo
neo extensions call echo tool.echo '{"value":42}'
```

Installed extensions live under `.neo/extensions/<id>`. Neo records local
source paths in `.neo/extensions-sources.toml` and enablement in
`.neo/extensions-state.toml`.

Provider-backed turns discover enabled project extensions by calling each
extension's JSONL RPC `tools.list` method and advertise returned tools as
`extension__<extension>__<tool>` functions. `--extension <PATH>` adds an
explicit root for one invocation, and `--no-extensions` disables automatic
project extension discovery.

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
