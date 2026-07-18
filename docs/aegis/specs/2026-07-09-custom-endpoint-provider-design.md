# Custom Endpoint Provider Wizard — Design Spec

## Summary

Add a first-class `Custom endpoint` path to the Add Provider dialog. This path is for users who already have a provider endpoint, API key, and one or more model IDs, but do not have a `models.dev`-compatible registry.

The wizard writes Neo's existing config shapes:

- `[providers.<id>]` with `type`, `base_url`, and either `api_key_env` or `api_key`.
- `[models."<alias>"]` with `provider`, `model`, token limits, capability tags, typed `reasoning`, and optional `display_name`.

This is a TUI design spec only. It does not implement code.

## Current Code Anchors

- Add Provider currently opens a `ChoicePicker` from `InteractiveController::open_add_provider_picker` in `crates/neo-agent/src/modes/interactive/dialog_results.rs`.
- Provider config is `ProviderConfig` in `crates/neo-agent/src/config/types.rs`: `provider_type`, `base_url`, `api_key`, `api_key_env`.
- Model config is `ModelConfig` in `crates/neo-agent/src/config/types.rs`: `provider`, `model`, `max_context_tokens`, `max_output_tokens`, `capabilities`, `reasoning`, `display_name`.
- Runtime resolves configured providers through `ProviderResolver`; it requires a base URL and credential before constructing the matching OpenAI-compatible, OpenAI Responses, Anthropic Messages, or Google Generative AI client.
- Custom endpoint UI exposes Neo's current provider protocol choices:
  - `OpenAI-compatible` -> `type = "openai"` -> `ApiType::OpenAi`
  - `OpenAI Responses` -> `type = "openai_response"` -> `ApiType::OpenAiResponse`
  - `Anthropic Messages` -> `type = "anthropic"` -> `ApiType::Anthropic`
  - `Google Generative AI` -> `type = "google"` -> `ApiType::Google`
- Capability tags currently parsed by runtime are `streaming`, `tools` / `tool_use`, `images` / `image_in` / `vision`, `reasoning` / `thinking`, and `embeddings` / `embedding`.
- Typed reasoning supports `None`, `Toggle`, `Effort`, `BudgetTokens`, and `Combined`; effort values are `minimal`, `low`, `medium`, `high`, `xhigh`, and `max`.

## Goals

- Make the common custom-provider case visible from Add Provider.
- Avoid making users invent or host a registry just to add one endpoint.
- Preserve Neo's canonical config strings. Labels may be friendly, but written values must be `openai`, `openai_response`, `anthropic`, or `google`.
- Let manual model entry configure the actual fields Neo stores today, including typed reasoning.
- Keep `Custom registry (api.json)` as an advanced path, not the default custom-provider path.

## Non-goals

- Do not write removed OpenAI provider type aliases such as `openai-compatible` or `openai-responses`; the OpenAI family has exactly two choices in this wizard: `openai` and `openai_response`.
- Do not introduce a provider config with no credential. The current resolver requires credentials, so local endpoints use a harmless inline placeholder key instead.
- Do not require a successful network test before saving. Users may configure offline or private endpoints.

## Interaction Model

The flow is a short wizard:

1. Choose `Custom endpoint` from Add Provider.
2. Define provider identity and provider protocol type.
3. Define endpoint and credential source.
4. Add models manually, or fetch `/models` for OpenAI-family protocols.
5. Configure capabilities and reasoning for each model.
6. Review the generated config and save.

The wizard should keep existing Neo dialog conventions: bordered overlay, focused field marked with `▸`, muted helper text, `Tab` for fields, arrow keys for pickers, `Esc` for back/cancel.

## TUI Character Art

### Add Provider

```text
╭ Add Provider ─────────────────────────────────────────────╮
│ Choose a source                                           │
│                                                           │
│   Known provider       Import from models.dev catalog      │
│ ▸ Custom endpoint     Configure a base URL and models      │
│   Custom registry      Import custom api.json              │
│                                                           │
│ ↑/↓ select · Enter continue · Esc cancel                  │
╰───────────────────────────────────────────────────────────╯
```

### Step 1/4 — Provider

```text
╭ Custom Endpoint 1/4 · Provider ───────────────────────────╮
│ Provider                                                  │
│ ▸ Display name                                            │
│   Acme Gateway▏                                           │
│                                                           │
│   Provider id                                             │
│   acme▏                                                   │
│                                                           │
│   API type                                                │
│   OpenAI-compatible  ›                                    │
│                                                           │
│ ↑/↓ select · Tab field · Enter continue · Esc cancel      │
╰───────────────────────────────────────────────────────────╯
```

Provider id is the config key under `[providers.<id>]`. The wizard should suggest a lowercase id from the display name and keep it simple: lowercase letters, digits, `_`, and `-`.

### API Type Picker

```text
╭ API Type ─────────────────────────────────────────────────╮
│ ▸ OpenAI-compatible     type = "openai"                   │
│   OpenAI Responses      type = "openai_response"          │
│   Anthropic Messages    type = "anthropic"                │
│   Google Generative AI  type = "google"                   │
│                                                           │
│ ↑/↓ select · Enter choose · Esc back                      │
╰───────────────────────────────────────────────────────────╯
```

The label `OpenAI-compatible` is UI copy only. It must write the canonical config value `openai`, not a legacy alias. `OpenAI Responses` must write `openai_response`, not the removed `openai-responses` alias.

### Step 2/4 — Endpoint & Auth

```text
╭ Custom Endpoint 2/4 · Endpoint & Auth ────────────────────╮
│ Endpoint                                                  │
│ ▸ Base URL                                                │
│   https://gateway.example.com/v1▏                         │
│                                                           │
│ Auth                                                      │
│   API key source                                          │
│   Environment variable  ›                                 │
│                                                           │
│   Env var name                                            │
│   ACME_API_KEY▏                                           │
│                                                           │
│ ↑/↓ select · Tab field · Enter continue · Esc back        │
╰───────────────────────────────────────────────────────────╯
```

### API Key Source Picker

```text
╭ API Key Source ───────────────────────────────────────────╮
│ ▸ Environment variable     writes api_key_env             │
│   Paste secret             writes api_key                 │
│   Local placeholder        writes api_key = "local"        │
│                                                           │
│ ↑/↓ select · Enter choose · Esc back                      │
╰───────────────────────────────────────────────────────────╯
```

`Local placeholder` is the current-code-compatible replacement for a literal `No auth` option. Neo's production resolver requires a credential before constructing provider clients, so this option writes a harmless inline key for endpoints that ignore auth.

### Step 3/4 — Model Source

```text
╭ Custom Endpoint 3/4 · Models ─────────────────────────────╮
│ How should Neo add models?                                │
│                                                           │
│ ▸ Fetch from /models     OpenAI-family model IDs           │
│   Enter manually        Add model ID and capabilities      │
│                                                           │
│ ↑/↓ select · Enter continue · Esc back                    │
╰───────────────────────────────────────────────────────────╯
```

`/models` fetch is model-id discovery, not a Neo metadata catalog. Standard OpenAI-style responses reliably provide model ids plus basic fields such as object type, creation time, and owner; provider-specific extra fields may appear but are hints only. Neo creates draft model configs from the fetched ids, then sends every selected model through review so the user fills missing token limits, capabilities, and typed reasoning before saving.

### Fetching Models

```text
╭ Custom Endpoint · Fetching Models ────────────────────────╮
│ GET https://gateway.example.com/v1/models                 │
│                                                           │
│ ◐ Connecting...                                           │
│                                                           │
│ Esc cancel                                                │
╰───────────────────────────────────────────────────────────╯
```

### Fetch Success

```text
╭ Custom Endpoint 3/4 · Select Models ──────────────────────╮
│ 7 models found                                            │
│                                                           │
│ ▣ qwen2.5-coder-32b-instruct                              │
│ ▣ deepseek-chat                                           │
│ ☐ text-embedding-3-small                                  │
│ ☐ rerank-v3                                               │
│                                                           │
│ ↑/↓ select · Space toggle · Enter review config           │
│ / filter · Esc back                                       │
╰───────────────────────────────────────────────────────────╯
```

Fetched model rows should not guess capabilities beyond conservative defaults. After selection, each selected model is reviewed through the same model config screens used by manual entry. `Fetch from /models` is an OpenAI-family convenience; for Anthropic Messages and Google Generative AI, the wizard should default to manual entry unless a provider-specific model listing flow is added.

### Fetched Model — Review Config

```text
╭ Custom Endpoint 3/4 · Review Model 1/2 ───────────────────╮
│ Source: /models                                           │
│   id = "qwen2.5-coder-32b-instruct"                       │
│   owned_by = "acme"                                       │
│                                                           │
│ Model                                                     │
│ ▸ Model id                                                │
│   qwen2.5-coder-32b-instruct                              │
│                                                           │
│   Alias                                                   │
│   acme/qwen2.5-coder-32b-instruct                         │
│                                                           │
│ Limits                                                    │
│   Context tokens   -                                      │
│   Output tokens    -                                      │
│                                                           │
│ ↑/↓ select · Tab field · Enter capabilities · Esc back    │
╰───────────────────────────────────────────────────────────╯
```

Fetched values prefill only fields backed by the response. Blank `-` fields are required review points, not errors; users can leave optional fields blank or fill them before continuing.

### Fetch Failed

```text
╭ Custom Endpoint 3/4 · Models ─────────────────────────────╮
│ Could not fetch /models                                   │
│ 404 Not Found                                             │
│                                                           │
│ ▸ Enter models manually                                   │
│   Edit endpoint                                           │
│   Try fetch again                                         │
│                                                           │
│ ↑/↓ select · Enter continue · Esc back                    │
╰───────────────────────────────────────────────────────────╯
```

### Model Config — Identity & Limits

```text
╭ Custom Endpoint 3/4 · Model 1 ────────────────────────────╮
│ Model                                                     │
│ ▸ Model id                                                │
│   qwen2.5-coder-32b-instruct▏                             │
│                                                           │
│   Alias                                                   │
│   acme/qwen2.5-coder-32b-instruct                         │
│                                                           │
│   Display name                                            │
│   Qwen 2.5 Coder 32B                                      │
│                                                           │
│ Limits                                                    │
│   Context tokens   128000                                 │
│   Output tokens    8192                                   │
│                                                           │
│ ↑/↓ select · Tab field · Enter capabilities · Esc back    │
╰───────────────────────────────────────────────────────────╯
```

`Alias` is the `[models."<alias>"]` key. It defaults to `<provider>/<model_id>` but stays editable so routed model IDs such as `deepseek/deepseek-r1` can have a clean Neo alias.

### Model Config — Capabilities

```text
╭ Custom Endpoint 3/4 · Model Capabilities ─────────────────╮
│ acme/qwen2.5-coder-32b-instruct                           │
│                                                           │
│ ▸ [x] streaming     Server can stream tokens               │
│   [x] tools         Model supports tool calls              │
│   [ ] images        Model accepts image input              │
│   [ ] embeddings    Model is an embedding model            │
│                                                           │
│   Reasoning         Effort: low, medium, high  ›           │
│                                                           │
│ ↑/↓ select · Space toggle · Enter continue · Esc back     │
╰───────────────────────────────────────────────────────────╯
```

Reasoning is not a plain capability checkbox because Neo stores typed `ReasoningCapability`. The display line summarizes the typed reasoning draft and opens the reasoning picker.

### Reasoning Type Picker

```text
╭ Reasoning Capability ─────────────────────────────────────╮
│ ▸ None                  reasoning = { type = "none" }      │
│   Toggle                on/off reasoning                   │
│   Effort                minimal/low/medium/high/xhigh/max  │
│   Budget tokens         min/max reasoning token budget      │
│   Combined              multiple reasoning controls         │
│                                                           │
│ ↑/↓ select · Enter configure · Esc back                   │
╰───────────────────────────────────────────────────────────╯
```

### Reasoning — Toggle

```text
╭ Reasoning · Toggle ───────────────────────────────────────╮
│ Model supports simple reasoning on/off                    │
│                                                           │
│ ▸ Off supported      yes                                  │
│                                                           │
│ Writes: { type = "toggle", disable_supported = true }      │
│                                                           │
│ ↑/↓ select · ←/→ change · Enter apply · Esc back          │
╰───────────────────────────────────────────────────────────╯
```

### Reasoning — Effort

```text
╭ Reasoning · Effort ───────────────────────────────────────╮
│ Select supported effort values                            │
│                                                           │
│ ▸ [ ] minimal                                             │
│   [x] low                                                 │
│   [x] medium                                              │
│   [x] high                                                │
│   [ ] xhigh                                               │
│   [ ] max                                                 │
│                                                           │
│   Off supported      yes                                  │
│                                                           │
│ ↑/↓ select · Space toggle · ←/→ change                    │
│ Enter apply · Esc back                                    │
╰───────────────────────────────────────────────────────────╯
```

This writes `ReasoningCapability::Effort { values, disable_supported }`. At least one effort value is required before applying.

### Reasoning — Budget Tokens

```text
╭ Reasoning · Budget Tokens ────────────────────────────────╮
│ Budget bounds                                             │
│ ▸ Min tokens        0                                     │
│   Max tokens        24576                                 │
│                                                           │
│   Off supported     yes                                   │
│                                                           │
│ ↑/↓ select · Tab field · Enter apply · Esc back           │
╰───────────────────────────────────────────────────────────╯
```

This writes `ReasoningCapability::BudgetTokens { min, max, disable_supported }`. Empty min/max fields map to `None`; numeric fields map to `Some(value)`.

### Reasoning — Combined

```text
╭ Reasoning · Combined ─────────────────────────────────────╮
│ Multiple reasoning controls                               │
│                                                           │
│ ▸ [x] toggle                                              │
│   [x] effort       low, medium, high  ›                   │
│   [ ] budget       min -, max -  ›                        │
│                                                           │
│   Off supported    yes                                    │
│                                                           │
│ ↑/↓ select · Space toggle · Enter edit/apply · Esc back   │
╰───────────────────────────────────────────────────────────╯
```

Use `Combined` only when the user explicitly enables more than one reasoning family. If the user enables only one family, save the simpler `Toggle`, `Effort`, or `BudgetTokens` variant instead.

### Added Models

```text
╭ Custom Endpoint 3/4 · Models ─────────────────────────────╮
│ Models                                                    │
│   acme/qwen2.5-coder-32b-instruct   tools · reasoning      │
│   acme/deepseek-chat                 streaming · tools     │
│                                                           │
│ ▸ + Add another model                                     │
│   Continue to review                                      │
│                                                           │
│ ↑/↓ select · Enter choose · Esc back                      │
╰───────────────────────────────────────────────────────────╯
```

### Step 4/4 — Review

```text
╭ Custom Endpoint 4/4 · Review ─────────────────────────────╮
│ Provider                                                  │
│   acme · Acme Gateway                                     │
│   type = "openai"                                         │
│   base_url = "https://gateway.example.com/v1"              │
│   api_key_env = "ACME_API_KEY"                            │
│                                                           │
│ Models                                                    │
│   acme/qwen2.5-coder-32b-instruct                         │
│     tools · effort low/medium/high · ctx 128k · out 8k      │
│   acme/deepseek-chat                                      │
│     streaming · tools · ctx 128k · out -                   │
│                                                           │
│ ▸ Save provider                                           │
│   Test connection                                         │
│   Back to models                                          │
│                                                           │
│ ↑/↓ select · Enter choose · Esc cancel                    │
╰───────────────────────────────────────────────────────────╯
```

### Test Connection — Success

```text
╭ Test Connection ──────────────────────────────────────────╮
│ Request succeeded                                         │
│ acme/qwen2.5-coder-32b-instruct is reachable              │
│                                                           │
│ ▸ Save provider                                           │
│   Back to review                                          │
│                                                           │
│ ↑/↓ select · Enter choose · Esc back                      │
╰───────────────────────────────────────────────────────────╯
```

### Test Connection — Failure

```text
╭ Test Connection ──────────────────────────────────────────╮
│ Request failed                                            │
│ 401 Unauthorized                                          │
│                                                           │
│ ▸ Edit auth                                               │
│   Save anyway                                             │
│   Back to review                                          │
│                                                           │
│ ↑/↓ select · Enter choose · Esc back                      │
╰───────────────────────────────────────────────────────────╯
```

### Validation Error

```text
╭ Custom Endpoint ──────────────────────────────────────────╮
│ Cannot continue                                           │
│ Provider id must use lowercase letters, digits, `_`, `-`. │
│                                                           │
│ Enter edit                                                │
╰───────────────────────────────────────────────────────────╯
```

Other blocking validation errors:

- Missing base URL.
- Missing env var name when API key source is `Environment variable`.
- Missing pasted secret when API key source is `Paste secret`.
- No model selected or added.
- Effort reasoning has no selected effort values.
- Budget min/max fields are non-numeric, negative, or min exceeds max.

### Saved

```text
╭ Provider Added ───────────────────────────────────────────╮
│ Added Acme Gateway                                        │
│ Current model: acme/qwen2.5-coder-32b-instruct            │
│                                                           │
│ Enter close                                               │
╰───────────────────────────────────────────────────────────╯
```

## Config Mapping

### Provider

`OpenAI-compatible` writes:

```toml
[providers.acme]
type = "openai"
base_url = "https://gateway.example.com/v1"
api_key_env = "ACME_API_KEY"
```

`OpenAI Responses` writes:

```toml
[providers.acme]
type = "openai_response"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
```

`Anthropic Messages` writes:

```toml
[providers.acme]
type = "anthropic"
base_url = "https://api.anthropic.com/v1"
api_key_env = "ANTHROPIC_API_KEY"
```

`Google Generative AI` writes:

```toml
[providers.acme]
type = "google"
base_url = "https://generativelanguage.googleapis.com/v1beta"
api_key_env = "GEMINI_API_KEY"
```

Credential source mapping:

| UI option | Config |
|---|---|
| Environment variable | `api_key_env = "<NAME>"` |
| Paste secret | `api_key = "<secret>"` |
| Local placeholder | `api_key = "local"` |

### Model

Manual model fields write:

```toml
[models."acme/qwen2.5-coder-32b-instruct"]
provider = "acme"
model = "qwen2.5-coder-32b-instruct"
display_name = "Qwen 2.5 Coder 32B"
max_context_tokens = 128000
max_output_tokens = 8192
capabilities = ["streaming", "tools", "reasoning"]

[models."acme/qwen2.5-coder-32b-instruct".reasoning]
type = "effort"
values = ["low", "medium", "high"]
disable_supported = true
```

If reasoning is `None`, the wizard should omit `reasoning` from TOML when possible and omit `reasoning` from `capabilities`. If reasoning is any supporting variant, include `reasoning` in `capabilities` for picker display and fallback compatibility.

## Defaults

- API type: `OpenAI-compatible`.
- API key source: `Environment variable`.
- Base URL placeholder:
  - `OpenAI-compatible`: `https://gateway.example.com/v1`
  - `OpenAI Responses`: `https://api.openai.com/v1`
  - `Anthropic Messages`: `https://api.anthropic.com/v1`
  - `Google Generative AI`: `https://generativelanguage.googleapis.com/v1beta`
- Manual model defaults:
  - `streaming = true`
  - `tools = true`
  - `images = false`
  - `embeddings = false`
  - reasoning `None`
  - context/output token fields blank unless user enters values
- Alias defaults to `<provider_id>/<model_id>`.
- Display name defaults to the model id but is editable.
- First saved model becomes `default_model` only when there is no current default model, or when the user explicitly chooses `Set as current model` if that control is added later.

## Error Handling

- Fetch `/models` should be best-effort. Failure must offer manual entry and should not block the wizard.
- Test connection should be advisory. Failure must offer `Save anyway`.
- Network tasks must not block TUI rendering; use the same pending-background pattern used by catalog fetch.
- Secret display must be masked and never echoed in review except as `api_key = <redacted>` if pasted.
- If saving replaces an existing provider id, the review screen should say `Replace provider` and list the number of existing models that will be replaced. This follows Neo's "simplify, don't pile on" rule: one provider id has one current definition.

## Implementation Boundary For Later

The implementation should likely add a dedicated custom endpoint wizard state rather than stretching `CustomRegistryImportState`, because the new flow has multi-step provider/model/reasoning state and writes both provider and model config. Existing picker/dialog primitives can still be reused.

Expected later verification scope:

- TUI state unit tests for API type choices, auth source mapping, capability toggles, and reasoning variants.
- Config mutation tests proving the wizard output writes canonical `type = "openai"` / `type = "openai_response"` / `type = "anthropic"` / `type = "google"` and typed `reasoning`.
- One focused interactive-controller test proving Add Provider includes `Custom endpoint` and opens the wizard.

## Self-review

- No placeholder sections remain.
- UI labels are friendly, but config values are canonical.
- API type picker includes all four current provider protocol values, while the OpenAI family is restricted to `openai` and `openai_response`.
- `No auth` was replaced with `Local placeholder` to match the current resolver's credential requirement.
- Reasoning is represented as typed `ReasoningCapability`, not as a single checkbox.
- The design is scoped to a spec and TUI contract; it does not prescribe an implementation plan.
