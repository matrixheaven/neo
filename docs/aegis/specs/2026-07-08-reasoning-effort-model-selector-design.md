# Neo Reasoning Effort Model Selector Design

## Background

Neo currently treats model reasoning as a boolean TUI choice named `thinking`.
The `/model` dialog can toggle thinking on or off, but the selected value is
collapsed to `ReasoningEffort::High` before a turn starts. That loses the
model-specific surface now exposed by provider catalogs and provider APIs:
some models support named effort values, some support token budgets, some only
support an on/off toggle, and some do not support reasoning at all.

The models.dev catalog currently exposes `reasoning_options` on many model
entries. The common shapes are:

- `{"type":"effort","values":[...]}`
- `{"type":"budget_tokens","min":...,"max":...}`
- `{"type":"toggle"}`

Neo's catalog parser currently keeps only `reasoning: true/false`, so `/model`
cannot tell the user what a selected model can actually accept. This design
upgrades `/model` into the canonical reasoning selection surface.

## Decision

Use Option B: embed a model-aware Reasoning control area inside `/model`.

Do not create a separate `/reasoning` dialog for the first version. Do not put
reasoning controls in `/provider`. The user is choosing a model in `/model`,
and the valid reasoning controls are a property of the selected model, so the
controls belong in the model selector.

## Goals

- Show valid reasoning controls for the currently selected model inside `/model`.
- Preserve the current fast model-switch workflow: filter, move, adjust
  reasoning, press Enter.
- Use models.dev `reasoning_options` as the primary source for imported models.
- Represent reasoning as a typed model capability and a typed user selection,
  not as a boolean `thinking` flag.
- Validate unsupported selections before provider requests are sent.
- Provide clear TUI feedback when switching to a model that cannot support the
  current reasoning selection.
- Keep the design local-only and config-backed; no hosted service or sync.

## Non-Goals

- Do not add a separate `/reasoning` command in this design.
- Do not keep the old `/model` boolean thinking path as a compatibility branch.
- Do not infer undocumented effort values from model names when
  `reasoning_options` is available.
- Do not make provider APIs accept all Neo effort values blindly.
- Do not silently coerce an unsupported user selection without showing a status
  message.
- Do not implement arbitrary free-form provider JSON overrides in `/model`.

## Strong Constraints

1. `/model` is the only TUI surface for changing model reasoning in this design.
2. The selected model row and its Reasoning control area must be visible in the
   same dialog viewport.
3. The dialog must never show effort values the selected model does not support.
4. `off` must always be available as a user action for models where reasoning is
   optional. If the model is always-reasoning and the provider cannot disable it,
   the UI must say so explicitly.
5. `none` from provider/catalog metadata is treated as the provider's explicit
   no-reasoning value. In the UI it is displayed as `off`; internally it must
   remain distinguishable where a provider needs to send `"none"`.
6. Budget entry must be bounded by the model's declared min/max. Out-of-range
   values cannot be submitted.
7. If a model has only `reasoning: true` and no `reasoning_options`, Neo must
   show a conservative fallback control rather than pretending to know all
   effort values.
8. The provider request builder must receive a validated `ReasoningSelection`;
   it must not reinterpret a stale boolean.
9. Existing config values may be migrated, but there must not be two runtime
   contracts after implementation. The canonical contract replaces the old
   boolean-like flow.
10. Tests must use narrow exact targets and must not broaden to workspace-wide
    `cargo test` as evidence.

## UI Design

The model selector remains list-first. The lower section is a compact
model-aware Reasoning control area. It changes as the selected row changes.

### Effort Model

For a model whose capability is `effort(values)`, show only supported values.
The selected value is bracketed. `off` appears when the model supports disabling
reasoning.

```text
╭ Models ─────────────────────────────────────────────────────────╮
│ / filter                                                        │
│                                                                │
│ › GPT-5.2                         openai          ← current     │
│   GPT-5.2 Pro                     openai                       │
│   Claude Opus 4.6                 anthropic                    │
│   Gemini 3 Flash Preview          google                       │
│                                                                │
│ Reasoning:  off   low   medium  [high]  xhigh                  │
│             ←/→ choose · Space off · Enter select              │
╰────────────────────────────────────────────────────────────────╯
```

If the model supports `max`, show `max`. If it supports `minimal`, show
`minimal`. Do not alias these in the UI.

```text
│ Reasoning:  off   minimal   low   medium   high   xhigh  [max] │
```

### Budget Model

For a model whose capability is `budget_tokens(min,max)`, show bounded presets
plus a custom entry mode. Presets are derived from the range and must stay in
range. The rendered range line is mandatory.

```text
╭ Models ─────────────────────────────────────────────────────────╮
│ › Gemini 2.5 Flash                 google                       │
│   Claude Sonnet 4.5                anthropic                    │
│                                                                │
│ Reasoning budget:  off   1k   [8k]   24k   custom              │
│ Range: 0..24576 tokens       Custom: _                         │
│ ←/→ preset · e edit custom · Space off · Enter select           │
╰────────────────────────────────────────────────────────────────╯
```

When editing a custom budget, invalid input is shown inline and Enter does not
submit the dialog.

```text
│ Reasoning budget:  off   1k   8k   24k  [custom]               │
│ Range: 0..24576 tokens       Custom: 40000                     │
│ Error: budget must be between 0 and 24576 tokens                │
```

### Toggle-Only Model

For `toggle`, show an on/off segmented control. This is the only case where the
old boolean interaction remains semantically true.

```text
╭ Models ─────────────────────────────────────────────────────────╮
│ › Qwen3 32B                       qiniu-ai                     │
│                                                                │
│ Reasoning:  [on]  off                                          │
│ Space toggle · Enter select                                    │
╰────────────────────────────────────────────────────────────────╯
```

### No Reasoning

For a model with no reasoning capability, the control area is read-only.

```text
╭ Models ─────────────────────────────────────────────────────────╮
│ › GPT-4o mini                     openai                       │
│                                                                │
│ Reasoning: unavailable for this model                           │
│ Enter select                                                   │
╰────────────────────────────────────────────────────────────────╯
```

### Unknown Reasoning Options

For a model with `reasoning: true` but no usable `reasoning_options`, show a
conservative fallback. The fallback is explicit and does not pretend the catalog
knows exact values.

```text
│ Reasoning:  off   [on]                                         │
│ Catalog has no effort metadata for this model                   │
```

## Interaction Rules

- Up/down changes the selected model row.
- Left/right changes the selected reasoning value within the visible control.
- Space toggles between the nearest enabled value and `off` when `off` is valid.
- `e` enters budget custom edit mode for budget models.
- Backspace edits custom budget only while in budget edit mode; otherwise it
  continues to edit the filter query.
- Enter selects the model and current reasoning value unless the budget value is
  invalid.
- Esc exits custom budget edit mode first, then clears filter, then cancels the
  dialog, preserving the existing dialog cancellation pattern.
- Switching rows keeps a per-model draft while the dialog is open. A draft is
  discarded if it becomes invalid because the model list is rebuilt.

## Data Model

Add typed reasoning metadata to model definitions.

```text
ReasoningCapability
  None
  Toggle { disable_supported: bool }
  Effort { values: Vec<ReasoningEffortValue>, disable_supported: bool }
  BudgetTokens { min: Option<u32>, max: Option<u32>, disable_supported: bool }
  Combined { toggle: bool, effort: Option<...>, budget: Option<...> }
```

The final Rust type names may differ, but the contract must expose these
semantic variants directly. A consumer must be able to distinguish toggle,
effort, budget, and unavailable reasoning without parsing capability strings.

```text
ReasoningSelection
  Off
  On
  Effort(ReasoningEffortValue)
  BudgetTokens(u32)
```

```text
ReasoningEffortValue
  Minimal
  Low
  Medium
  High
  XHigh
  Max
```

Provider/catalog value `none` is not a display effort. It maps to
`ReasoningSelection::Off` plus provider metadata saying the wire value is
explicit `none` when required.

## Catalog Import

`neo-ai` must parse `reasoning_options` from models.dev model entries.

The catalog conversion must carry reasoning metadata into imported model config.
Imported models should no longer be reduced to capability strings such as only
`"reasoning"`. Capability strings may remain for display or old config parsing
during migration, but the canonical model contract must be typed reasoning
metadata.

When `reasoning_options` is missing:

- `reasoning: false` -> `ReasoningCapability::None`
- `reasoning: true` -> conservative unknown/toggle-style fallback

When multiple options exist, prefer richer controls in this order:

1. `effort(values)` when values are present.
2. `budget_tokens(min,max)` when no effort values are present.
3. `toggle`.

If both effort and budget are present and both are meaningful, the first version
must show effort as the primary control and must not expose a competing budget
control in the compact `/model` footer. A later design can add an advanced
budget path for combined-capability models, but this design keeps the v1 UI
single-mode per selected model.

## Config Semantics

The canonical persisted selection is structured, not a boolean.

Use this TOML shape for the runtime default reasoning selection:

```toml
[runtime.reasoning]
mode = "effort"
effort = "high"
```

```toml
[runtime.reasoning]
mode = "budget_tokens"
budget_tokens = 8192
```

```toml
[runtime.reasoning]
mode = "off"
```

- One canonical place for runtime reasoning selection.
- No lasting dual-path between old `runtime.reasoning_effort` and new selection.
- A deterministic migration from old `runtime.reasoning_effort` to the new
  structured value.
- Config display/redaction commands must show the selected reasoning mode
  clearly.

## Runtime Data Flow

```text
models.dev api.json
  -> CatalogModel.reasoning_options
  -> ModelConfig.reasoning
  -> ModelEntry.reasoning
  -> /model Reasoning control
  -> ModelSelection { alias, reasoning_selection }
  -> TurnRequest.reasoning_selection
  -> ChatRequest.options.reasoning
  -> provider-specific request mapping
```

The TUI must not convert `on` to `High` on its own. Provider mapping belongs at
the provider boundary after model-aware validation.

## Provider Mapping

Provider adapters must accept only validated selections.

OpenAI Responses:

- Send `reasoning.effort` only for effort values supported by the model.
- Send no reasoning object for `Off` unless the selected model/provider requires
  explicit `none`.
- Preserve reasoning summary behavior separately from effort selection.

OpenAI-compatible:

- Support provider/model-specific effort aliases from catalog metadata or a
  small typed mapping table.
- Known examples such as `xhigh -> max` or `low/medium -> high` must be explicit
  mappings, not hidden global coercions.
- If no mapping exists, reject before request.

Anthropic:

- Budget-capable models use bounded `budget_tokens`.
- Effort-capable models use the provider's effort/adaptive-thinking shape when
  supported by the provider API used by Neo.
- Do not send temperature when the selected reasoning mode conflicts with
  provider extended-thinking rules.

Google:

- Budget-capable Gemini models use `thinkingBudget`.
- Effort-capable Gemini models use the provider's effort/level shape if exposed
  by the API path Neo uses.
- `Off` maps to budget `0` only for models whose metadata allows disabling via
  zero or explicit none.

## Error Handling

- Unsupported reasoning selection for a model: reject before sending the
  provider request and show a status message naming the model and unsupported
  selection.
- Catalog entry has malformed `reasoning_options`: ignore malformed option,
  keep parsing the rest, and show conservative fallback if no usable option
  remains.
- Custom budget out of range: keep dialog open and show inline error.
- Switching model invalidates current selection: choose `off` when allowed;
  otherwise choose the model's first supported value and show a status message.
- Provider rejects a supposedly valid selection: surface the provider error and
  do not mutate the saved reasoning selection automatically.

## Testing

Use focused tests only.

- `neo-ai` catalog tests:
  - Parses effort values including `minimal`, `xhigh`, `max`, and `none`.
  - Parses budget min/max and toggle.
  - Falls back conservatively when only `reasoning: true` exists.

- `neo-tui` model selector tests:
  - Effort model renders only supported values and moves selection with arrows.
  - Budget model renders range, presets, custom entry, and invalid budget error.
  - Toggle-only model renders on/off.
  - No-reasoning model renders unavailable text.
  - Enter returns `ModelSelection` with structured reasoning selection.

- `neo-agent` interactive tests:
  - Applying a model selection updates runtime config with structured reasoning.
  - Starting a turn passes structured reasoning instead of forcing High.
  - Switching from a reasoning model to a no-reasoning model normalizes the
    selection and emits a status message.

- Provider request tests:
  - OpenAI Responses emits supported effort.
  - OpenAI-compatible rejects unsupported effort without a mapping.
  - Anthropic budget mapping respects min/max.
  - Google budget mapping supports explicit off only when allowed.

Exact single-test commands are preferred during implementation, for example:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::<test_name> --exact --nocapture --include-ignored
```

## Acceptance Criteria

- `/model` shows a Reasoning control area for the selected model.
- The UI character and keyboard behavior match this spec.
- Imported models from models.dev preserve `reasoning_options`.
- Neo can represent `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`,
  toggle, and budget-token reasoning capabilities.
- TUI selection returns structured reasoning, not boolean thinking.
- Runtime no longer forces enabled reasoning to `High`.
- Provider requests are built from validated structured reasoning selections.
- Unsupported selections fail before provider requests.
- No old boolean thinking path remains as a parallel contract.

## Review Notes

- The design intentionally keeps reasoning controls in `/model` because the
  valid controls are selected-model-specific.
- The compact footer avoids a second modal while still showing capability
  differences.
- The old boolean `thinking` concept survives only as visual wording where a
  model is genuinely toggle-only; it is not the internal contract.
- No placeholders remain.
