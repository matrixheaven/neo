# Reasoning Effort Model Selector Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Neo's boolean `/model` thinking toggle with a model-aware Reasoning control that supports effort values, token budgets, toggle-only models, and unavailable reasoning.

**Architecture:** Add typed reasoning capability and selection models in `neo-ai`, carry those types through catalog import, config, runtime request construction, provider adapters, and TUI model selection. `/model` becomes the canonical TUI reasoning surface; provider requests are built only from validated structured selections.

**Tech Stack:** Rust 2024 workspace, serde/schemars, ratatui-style ANSI rendering in `neo-tui`, models.dev JSON catalog, existing exact `cargo test --package ... -- ... --exact --nocapture --include-ignored` verification style.

---

## Execution Guardrails

- Do not run git mutations from this plan unless the user gives explicit per-instance authorization. In this repository, that includes `git add`, `git commit`, branch operations, reset, checkout, stash, rebase, clean, merge, and push.
- Do not keep the old boolean thinking path as a parallel runtime contract. Legacy config deserialization may exist only as a migration input; runtime and request code must use structured reasoning.
- Do not broaden tests to workspace-wide `cargo test`. Every verification command below names one package, one target selector, and one exact test name.
- Work in the existing dirty worktree. Ignore unrelated dirty files.

## File Structure

- Modify `crates/neo-ai/src/options.rs`: canonical `ReasoningEffort`, `ReasoningSelection`, `ReasoningCapability`, budget bounds, validation helpers.
- Modify `crates/neo-ai/src/types.rs`: make `ModelCapabilities` carry typed reasoning capability, and make `ChatRequest` continue to own `RequestOptions`.
- Modify `crates/neo-ai/src/catalog.rs`: parse models.dev `reasoning_options` and produce typed reasoning metadata in `CatalogModelInfo`.
- Modify `crates/neo-agent/src/config/types.rs`: add typed model reasoning metadata and structured runtime reasoning config; keep legacy `reasoning_effort` only as a deserialization-only migration input.
- Modify `crates/neo-agent/src/config/mod.rs` and `crates/neo-agent/src/config/loader.rs`: load and expose the new runtime reasoning selection.
- Modify `crates/neo-agent/src/config/mutations.rs`: persist catalog-imported reasoning metadata and update runtime reasoning selection from `/model`.
- Modify `crates/neo-agent/src/modes/run/runtime/model.rs`: convert configured models into `ModelSpec` with typed reasoning.
- Modify `crates/neo-agent-core/src/runtime/config.rs` and `crates/neo-agent-core/src/runtime/chat_request.rs`: carry structured reasoning into `ChatRequest` and validate before requests.
- Modify provider adapters in `crates/neo-ai/src/providers/openai/responses.rs`, `crates/neo-ai/src/providers/openai/compatible.rs`, `crates/neo-ai/src/providers/anthropic.rs`, and `crates/neo-ai/src/providers/google.rs`: map structured selections to provider wire shapes.
- Modify `crates/neo-tui/src/dialogs/model_selector.rs` and `crates/neo-tui/src/dialogs/tabbed_model_selector.rs`: render and edit the Reasoning control area.
- Modify `crates/neo-agent/src/modes/interactive/model_picker.rs`, `dialog_results.rs`, `turn.rs`, `controller_factory.rs`, and `mod.rs`: feed typed reasoning into `/model`, apply selections, start turns.
- Modify focused tests in `crates/neo-ai/tests/env_and_options.rs`, `crates/neo-ai/tests/real_provider_adapters.rs`, `crates/neo-ai/tests/openai_compatible_provider.rs`, `crates/neo-agent-core/tests/runtime_turn.rs`, `crates/neo-agent/src/modes/interactive/tests.rs`, and `crates/neo-tui/src/dialogs/model_selector.rs`.

## Task 1: Canonical Reasoning Types In `neo-ai`

**Files:**
- Modify: `crates/neo-ai/src/options.rs`
- Modify: `crates/neo-ai/src/types.rs`
- Modify: `crates/neo-ai/src/lib.rs`
- Test: `crates/neo-ai/tests/env_and_options.rs`

- [ ] **Step 1: Write failing serialization and validation tests**

Append these tests to `crates/neo-ai/tests/env_and_options.rs`:

```rust
#[test]
fn reasoning_effort_serializes_max_and_stable_names() {
    assert_eq!(
        serde_json::to_value(ReasoningEffort::Max).expect("serialize max"),
        serde_json::json!("max")
    );
    assert_eq!(
        serde_json::from_value::<ReasoningEffort>(serde_json::json!("xhigh"))
            .expect("deserialize xhigh"),
        ReasoningEffort::XHigh
    );
}

#[test]
fn reasoning_selection_round_trips_structured_modes() {
    let effort = ReasoningSelection::Effort {
        effort: ReasoningEffort::High,
    };
    let encoded = serde_json::to_value(&effort).expect("serialize effort selection");
    assert_eq!(encoded, serde_json::json!({ "mode": "effort", "effort": "high" }));
    assert_eq!(
        serde_json::from_value::<ReasoningSelection>(encoded).expect("deserialize effort"),
        effort
    );

    let budget = ReasoningSelection::BudgetTokens {
        budget_tokens: 8192,
    };
    let encoded = serde_json::to_value(&budget).expect("serialize budget selection");
    assert_eq!(
        encoded,
        serde_json::json!({ "mode": "budget_tokens", "budget_tokens": 8192 })
    );
    assert_eq!(
        serde_json::from_value::<ReasoningSelection>(encoded).expect("deserialize budget"),
        budget
    );

    assert_eq!(
        serde_json::to_value(ReasoningSelection::Off).expect("serialize off"),
        serde_json::json!({ "mode": "off" })
    );
}

#[test]
fn reasoning_capability_validates_supported_selection() {
    let capability = ReasoningCapability::Effort {
        values: vec![ReasoningEffort::Low, ReasoningEffort::High],
        disable_supported: true,
    };
    assert!(capability.supports(&ReasoningSelection::Off));
    assert!(capability.supports(&ReasoningSelection::Effort {
        effort: ReasoningEffort::High,
    }));
    assert!(!capability.supports(&ReasoningSelection::Effort {
        effort: ReasoningEffort::Medium,
    }));
    assert!(!capability.supports(&ReasoningSelection::BudgetTokens {
        budget_tokens: 1024,
    }));
}

#[test]
fn reasoning_budget_bounds_accept_only_range_values() {
    let capability = ReasoningCapability::BudgetTokens {
        min: Some(512),
        max: Some(24_576),
        disable_supported: true,
    };
    assert!(capability.supports(&ReasoningSelection::BudgetTokens {
        budget_tokens: 512,
    }));
    assert!(capability.supports(&ReasoningSelection::BudgetTokens {
        budget_tokens: 8192,
    }));
    assert!(!capability.supports(&ReasoningSelection::BudgetTokens {
        budget_tokens: 128,
    }));
    assert!(!capability.supports(&ReasoningSelection::BudgetTokens {
        budget_tokens: 32_000,
    }));
}
```

Update the import list at the top of the same file to include:

```rust
use neo_ai::{ReasoningCapability, ReasoningEffort, ReasoningSelection};
```

- [ ] **Step 2: Run the new `neo-ai` type test and confirm failure**

Run:

```bash
cargo test --package neo-ai --test env_and_options -- reasoning_selection_round_trips_structured_modes --exact --nocapture
```

Expected: FAIL because `ReasoningSelection` and `ReasoningCapability` do not exist yet.

- [ ] **Step 3: Implement canonical reasoning types**

In `crates/neo-ai/src/options.rs`, replace the current `ReasoningEffort` block with this extended block and add the new types below it:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    #[serde(alias = "Minimal")]
    Minimal,
    #[serde(alias = "Low")]
    Low,
    #[serde(alias = "Medium")]
    Medium,
    #[serde(alias = "High")]
    High,
    #[serde(rename = "xhigh", alias = "XHigh")]
    XHigh,
    #[serde(alias = "Max")]
    Max,
}

impl ReasoningEffort {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ReasoningSelection {
    Off,
    On,
    Effort { effort: ReasoningEffort },
    BudgetTokens { budget_tokens: u32 },
}

impl ReasoningSelection {
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        !matches!(self, Self::Off)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReasoningCapability {
    None,
    Toggle {
        disable_supported: bool,
    },
    Effort {
        values: Vec<ReasoningEffort>,
        disable_supported: bool,
    },
    BudgetTokens {
        min: Option<u32>,
        max: Option<u32>,
        disable_supported: bool,
    },
    Combined {
        toggle: bool,
        effort: Option<Vec<ReasoningEffort>>,
        budget: Option<ReasoningBudget>,
        disable_supported: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReasoningBudget {
    pub min: Option<u32>,
    pub max: Option<u32>,
}

impl Default for ReasoningCapability {
    fn default() -> Self {
        Self::None
    }
}

impl ReasoningCapability {
    #[must_use]
    pub const fn supports_reasoning(&self) -> bool {
        !matches!(self, Self::None)
    }

    #[must_use]
    pub fn supports(&self, selection: &ReasoningSelection) -> bool {
        match (self, selection) {
            (_, ReasoningSelection::Off) => self.disable_supported(),
            (Self::Toggle { .. }, ReasoningSelection::On) => true,
            (Self::Effort { values, .. }, ReasoningSelection::Effort { effort }) => {
                values.contains(effort)
            }
            (Self::BudgetTokens { min, max, .. }, ReasoningSelection::BudgetTokens { budget_tokens }) => {
                min.is_none_or(|value| *budget_tokens >= value)
                    && max.is_none_or(|value| *budget_tokens <= value)
            }
            (
                Self::Combined {
                    toggle,
                    effort,
                    budget,
                    ..
                },
                selection,
            ) => match selection {
                ReasoningSelection::On => *toggle,
                ReasoningSelection::Effort { effort: selected } => {
                    effort.as_ref().is_some_and(|values| values.contains(selected))
                }
                ReasoningSelection::BudgetTokens { budget_tokens } => budget
                    .as_ref()
                    .is_some_and(|bounds| {
                        bounds.min.is_none_or(|value| *budget_tokens >= value)
                            && bounds.max.is_none_or(|value| *budget_tokens <= value)
                    }),
                ReasoningSelection::Off => self.disable_supported(),
            },
            _ => false,
        }
    }

    #[must_use]
    pub const fn disable_supported(&self) -> bool {
        match self {
            Self::None => true,
            Self::Toggle { disable_supported }
            | Self::Effort {
                disable_supported, ..
            }
            | Self::BudgetTokens {
                disable_supported, ..
            }
            | Self::Combined {
                disable_supported, ..
            } => *disable_supported,
        }
    }
}
```

In `RequestOptions`, replace:

```rust
pub reasoning_effort: Option<ReasoningEffort>,
```

with:

```rust
pub reasoning: ReasoningSelection,
```

and in `Default for RequestOptions`, replace:

```rust
reasoning_effort: None,
```

with:

```rust
reasoning: ReasoningSelection::Off,
```

In `crates/neo-ai/src/types.rs`, update `ModelCapabilities` so its reasoning field uses the typed capability. Replace the bool field:

```rust
pub reasoning: bool,
```

with:

```rust
pub reasoning: ReasoningCapability,
```

and set constructor defaults to `ReasoningCapability::None`. Add this helper to the `impl ModelCapabilities` block:

```rust
#[must_use]
pub fn supports_reasoning(&self) -> bool {
    self.reasoning.supports_reasoning()
}
```

In `crates/neo-ai/src/lib.rs`, export the new types from the same place where options are currently exported:

```rust
pub use options::{
    CacheRetention, ReasoningBudget, ReasoningCapability, ReasoningEffort, ReasoningSelection,
    RequestMetadata, RequestOptions,
};
```

- [ ] **Step 4: Run the `neo-ai` type tests**

Run:

```bash
cargo test --package neo-ai --test env_and_options -- reasoning_selection_round_trips_structured_modes --exact --nocapture
```

Expected: PASS.

Run:

```bash
cargo test --package neo-ai --test env_and_options -- reasoning_capability_validates_supported_selection --exact --nocapture
```

Expected: PASS.

- [ ] **Step 5: Checkpoint**

Do not commit unless the user explicitly authorizes git mutations for this task. Record touched files and test results in the handoff.

## Task 2: Parse models.dev `reasoning_options`

**Files:**
- Modify: `crates/neo-ai/src/catalog.rs`
- Modify: `crates/neo-agent/src/main.rs`
- Modify: `crates/neo-agent/src/config/mutations.rs`

- [ ] **Step 1: Add failing catalog tests**

Append these tests to the existing `#[cfg(test)] mod tests` in `crates/neo-ai/src/catalog.rs`:

```rust
#[test]
fn catalog_model_capability_reads_effort_reasoning_options() {
    let model: CatalogModel = serde_json::from_value(serde_json::json!({
        "id": "gpt-test",
        "reasoning": true,
        "reasoning_options": [
            { "type": "effort", "values": ["none", "minimal", "low", "medium", "high", "xhigh", "max"] }
        ]
    }))
    .expect("catalog model");

    assert_eq!(
        catalog_model_reasoning(&model),
        ReasoningCapability::Effort {
            values: vec![
                ReasoningEffort::Minimal,
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
                ReasoningEffort::XHigh,
                ReasoningEffort::Max,
            ],
            disable_supported: true,
        }
    );
}

#[test]
fn catalog_model_capability_reads_budget_reasoning_options() {
    let model: CatalogModel = serde_json::from_value(serde_json::json!({
        "id": "gemini-test",
        "reasoning": true,
        "reasoning_options": [
            { "type": "toggle" },
            { "type": "budget_tokens", "min": 0, "max": 24576 }
        ]
    }))
    .expect("catalog model");

    assert_eq!(
        catalog_model_reasoning(&model),
        ReasoningCapability::BudgetTokens {
            min: Some(0),
            max: Some(24_576),
            disable_supported: true,
        }
    );
}

#[test]
fn catalog_model_capability_falls_back_for_unknown_reasoning_metadata() {
    let model: CatalogModel = serde_json::from_value(serde_json::json!({
        "id": "unknown-reasoner",
        "reasoning": true
    }))
    .expect("catalog model");

    assert_eq!(
        catalog_model_reasoning(&model),
        ReasoningCapability::Toggle {
            disable_supported: true,
        }
    );
}
```

At the top of the test module, add:

```rust
use crate::{ReasoningCapability, ReasoningEffort};
```

- [ ] **Step 2: Run one catalog test and confirm failure**

Run:

```bash
cargo test --package neo-ai --lib catalog::tests::catalog_model_capability_reads_effort_reasoning_options --exact --nocapture
```

Expected: FAIL because `reasoning_options` and `catalog_model_reasoning` are not implemented.

- [ ] **Step 3: Implement catalog parsing**

In `crates/neo-ai/src/catalog.rs`, add imports:

```rust
use serde_json::Value;

use crate::{ApiType, ReasoningBudget, ReasoningCapability, ReasoningEffort};
```

Add the field to `CatalogModel`:

```rust
#[serde(default)]
pub reasoning_options: Vec<Value>,
```

Add a typed reasoning field to `CatalogModelInfo`:

```rust
pub reasoning: ReasoningCapability,
```

In `catalog_provider_models`, add:

```rust
reasoning: catalog_model_reasoning(m),
```

Add these helpers near the existing capability helpers:

```rust
fn catalog_model_reasoning(model: &CatalogModel) -> ReasoningCapability {
    if !model.reasoning.unwrap_or(false) {
        return ReasoningCapability::None;
    }

    let mut toggle = false;
    let mut effort: Option<(Vec<ReasoningEffort>, bool)> = None;
    let mut budget: Option<ReasoningBudget> = None;

    for option in &model.reasoning_options {
        let Some(option_type) = option.get("type").and_then(Value::as_str) else {
            continue;
        };
        match option_type {
            "toggle" => toggle = true,
            "effort" => {
                let mut disable_supported = false;
                let values = option
                    .get("values")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter_map(|value| match value {
                        "none" => {
                            disable_supported = true;
                            None
                        }
                        "minimal" => Some(ReasoningEffort::Minimal),
                        "low" => Some(ReasoningEffort::Low),
                        "medium" => Some(ReasoningEffort::Medium),
                        "high" => Some(ReasoningEffort::High),
                        "xhigh" => Some(ReasoningEffort::XHigh),
                        "max" => Some(ReasoningEffort::Max),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                if !values.is_empty() {
                    effort = Some((values, disable_supported));
                }
            }
            "budget_tokens" => {
                budget = Some(ReasoningBudget {
                    min: option.get("min").and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok()),
                    max: option.get("max").and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok()),
                });
            }
            _ => {}
        }
    }

    if let Some((values, disable_supported)) = effort {
        return ReasoningCapability::Effort {
            values,
            disable_supported: disable_supported || toggle,
        };
    }
    if let Some(bounds) = budget {
        return ReasoningCapability::BudgetTokens {
            min: bounds.min,
            max: bounds.max,
            disable_supported: toggle || bounds.min == Some(0),
        };
    }
    if toggle || model.reasoning.unwrap_or(false) {
        return ReasoningCapability::Toggle {
            disable_supported: true,
        };
    }
    ReasoningCapability::None
}
```

Keep `catalog_model_capabilities` adding `"reasoning"` when `catalog_model_reasoning(model).supports_reasoning()` is true.

- [ ] **Step 4: Carry reasoning into catalog display and config mutation**

In `crates/neo-agent/src/main.rs`, update `catalog_models_json` so each model JSON includes reasoning metadata:

```rust
"reasoning": model.reasoning,
```

In `crates/neo-agent/src/config/mutations.rs`, update `catalog_model_config` to populate the new `reasoning` field on `ModelConfig`:

```rust
reasoning: model_info.reasoning.clone(),
```

- [ ] **Step 5: Run catalog tests**

Run:

```bash
cargo test --package neo-ai --lib catalog::tests::catalog_model_capability_reads_effort_reasoning_options --exact --nocapture
```

Expected: PASS.

Run:

```bash
cargo test --package neo-ai --lib catalog::tests::catalog_model_capability_reads_budget_reasoning_options --exact --nocapture
```

Expected: PASS.

- [ ] **Step 6: Checkpoint**

Do not commit unless explicitly authorized. Record the new catalog JSON/config fields and exact test output.

## Task 3: Structured Config And ModelSpec Wiring

**Files:**
- Modify: `crates/neo-agent/src/config/types.rs`
- Modify: `crates/neo-agent/src/config/mod.rs`
- Modify: `crates/neo-agent/src/config/loader.rs`
- Modify: `crates/neo-agent/src/modes/run/runtime/model.rs`
- Modify: `crates/neo-ai/src/registry.rs`
- Test: `crates/neo-agent/src/config/mod.rs`
- Test: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Add failing config migration test**

In `crates/neo-agent/src/config/mod.rs`, inside the existing test module, add:

```rust
#[test]
fn runtime_reasoning_uses_structured_config_and_migrates_legacy_effort() {
    let parsed: crate::config::types::FileConfig = toml::from_str(
        r#"
        [runtime]
        reasoning_effort = "high"
        replay_reasoning = true
        "#,
    )
    .expect("parse legacy config");

    let runtime = super::loader::runtime_from_file_for_tests(parsed.runtime);
    assert_eq!(
        runtime.reasoning,
        neo_ai::ReasoningSelection::Effort {
            effort: neo_ai::ReasoningEffort::High,
        }
    );
}
```

If `loader::runtime_from_file` is private, expose a test-only wrapper:

```rust
#[cfg(test)]
pub(crate) fn runtime_from_file_for_tests(
    runtime: Option<crate::config::types::FileRuntimeConfig>,
) -> RuntimeConfig {
    runtime_from_file(runtime)
}
```

- [ ] **Step 2: Run the migration test and confirm failure**

Run:

```bash
cargo test --package neo-agent --bin neo -- config::tests::runtime_reasoning_uses_structured_config_and_migrates_legacy_effort --exact --nocapture --include-ignored
```

Expected: FAIL because `RuntimeConfig.reasoning` does not exist.

- [ ] **Step 3: Add structured config fields**

In `crates/neo-agent/src/config/types.rs`, add a typed model field:

```rust
#[serde(default)]
pub reasoning: neo_ai::ReasoningCapability,
```

to `ModelConfig`.

In `FileRuntimeConfig`, add the new field:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub(crate) reasoning: Option<neo_ai::ReasoningSelection>,
```

Keep the legacy field as deserialization-only migration input:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub(crate) reasoning_effort: Option<ReasoningEffort>,
```

Do not add `reasoning_effort` to `RuntimeConfig`.

In `crates/neo-agent/src/config/mod.rs`, replace `RuntimeConfig.reasoning_effort` with:

```rust
pub reasoning: neo_ai::ReasoningSelection,
```

and default it to:

```rust
reasoning: neo_ai::ReasoningSelection::Off,
```

- [ ] **Step 4: Migrate loader logic**

In `crates/neo-agent/src/config/loader.rs`, replace the old runtime mapping:

```rust
reasoning_effort: runtime.reasoning_effort,
```

with:

```rust
reasoning: runtime.reasoning.unwrap_or_else(|| {
    runtime
        .reasoning_effort
        .map_or(neo_ai::ReasoningSelection::Off, |effort| {
            neo_ai::ReasoningSelection::Effort { effort }
        })
}),
```

- [ ] **Step 5: Update configured models to typed reasoning**

In `crates/neo-agent/src/modes/run/runtime/model.rs`, update `parse_model_capabilities` to accept a `ReasoningCapability`:

```rust
fn parse_model_capabilities(
    caps: &[String],
    reasoning: neo_ai::ReasoningCapability,
    max_context_tokens: Option<u32>,
    max_output_tokens: Option<u32>,
) -> neo_ai::ModelCapabilities {
    let mut mc = neo_ai::ModelCapabilities::tool_chat();
    mc.streaming = false;
    mc.tools = false;
    mc.images = false;
    mc.reasoning = reasoning;
    mc.embeddings = false;
    for cap in caps {
        match cap.trim().to_ascii_lowercase().as_str() {
            "streaming" => mc.streaming = true,
            "tools" | "tool_use" => mc.tools = true,
            "images" | "image_in" | "vision" => mc.images = true,
            "reasoning" | "thinking" if !mc.reasoning.supports_reasoning() => {
                mc.reasoning = neo_ai::ReasoningCapability::Toggle {
                    disable_supported: true,
                };
            }
            "embeddings" | "embedding" => mc.embeddings = true,
            _ => {}
        }
    }
    mc.max_context_tokens = max_context_tokens;
    mc.max_output_tokens = max_output_tokens;
    mc
}
```

Update the call site to pass `cfg.reasoning.clone()`.

In `crates/neo-ai/src/registry.rs`, update built-in model constructors so reasoning-capable built-ins use a conservative capability:

```rust
capabilities.reasoning = ReasoningCapability::Toggle {
    disable_supported: true,
};
```

and non-reasoning built-ins use `ReasoningCapability::None`.

- [ ] **Step 6: Run the config migration test**

Run:

```bash
cargo test --package neo-agent --bin neo -- config::tests::runtime_reasoning_uses_structured_config_and_migrates_legacy_effort --exact --nocapture --include-ignored
```

Expected: PASS.

- [ ] **Step 7: Checkpoint**

Do not commit unless explicitly authorized. Record any compile failures caused by old `capabilities.reasoning` bool call sites; those are handled in later tasks.

## Task 4: Runtime Request Validation And ChatRequest Plumbing

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/config.rs`
- Modify: `crates/neo-agent-core/src/runtime/chat_request.rs`
- Modify: `crates/neo-agent-core/tests/runtime_turn.rs`
- Modify: `crates/neo-agent/src/modes/run/runtime/agent.rs`
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`

- [ ] **Step 1: Replace runtime tests with structured reasoning expectations**

In `crates/neo-agent-core/tests/runtime_turn.rs`, replace `runtime_rejects_reasoning_effort_when_model_lacks_reasoning_before_request` with:

```rust
#[tokio::test]
async fn runtime_rejects_reasoning_selection_when_model_lacks_reasoning_before_request() {
    let mut harness = FakeHarness::new();
    let mut config = harness.config();
    config.model.capabilities.reasoning = ReasoningCapability::None;
    config.reasoning = ReasoningSelection::Effort {
        effort: ReasoningEffort::Low,
    };

    let error = harness.run_turn_with_config(config, "hello").await.unwrap_err();

    assert!(
        error.to_string().contains("does not support reasoning"),
        "unsupported reasoning should fail before provider request: {error}"
    );
    assert!(harness.requests().is_empty());
}
```

Replace `runtime_passes_reasoning_effort_into_chat_request_options` with:

```rust
#[tokio::test]
async fn runtime_passes_reasoning_selection_into_chat_request_options() {
    let mut harness = FakeHarness::new();
    let mut config = harness.config();
    config.model.capabilities.reasoning = ReasoningCapability::Effort {
        values: vec![ReasoningEffort::Low, ReasoningEffort::High],
        disable_supported: true,
    };
    config.reasoning = ReasoningSelection::Effort {
        effort: ReasoningEffort::Low,
    };

    harness.run_turn_with_config(config, "hello").await.unwrap();

    assert_eq!(
        harness.requests()[0].options.reasoning,
        ReasoningSelection::Effort {
            effort: ReasoningEffort::Low,
        }
    );
}
```

Update imports in the test file:

```rust
use neo_ai::{ReasoningCapability, ReasoningEffort, ReasoningSelection};
```

- [ ] **Step 2: Run the runtime rejection test and confirm failure**

Run:

```bash
cargo test --package neo-agent-core --test runtime_turn -- runtime_rejects_reasoning_selection_when_model_lacks_reasoning_before_request --exact --nocapture
```

Expected: FAIL because runtime config still uses `reasoning_effort`.

- [ ] **Step 3: Update runtime config and request construction**

In `crates/neo-agent-core/src/runtime/config.rs`, replace:

```rust
pub reasoning_effort: Option<ReasoningEffort>,
```

with:

```rust
pub reasoning: ReasoningSelection,
```

and default it to `ReasoningSelection::Off`.

In `crates/neo-agent-core/src/runtime/chat_request.rs`, replace:

```rust
reasoning_effort: config.reasoning_effort,
```

with:

```rust
reasoning: config.reasoning.clone(),
```

Replace validation:

```rust
if request.options.reasoning_effort.is_some() && !capabilities.reasoning {
```

with:

```rust
if request.options.reasoning.is_enabled()
    && !capabilities.reasoning.supports(&request.options.reasoning)
{
```

and keep the existing error message shape.

In `crates/neo-agent/src/modes/run/runtime/agent.rs`, map app config to runtime config:

```rust
agent_config.reasoning = config.runtime.reasoning.clone();
```

- [ ] **Step 4: Update interactive `TurnRequest` field**

In `crates/neo-agent/src/modes/interactive/mod.rs`, replace:

```rust
pub reasoning_effort: Option<neo_ai::ReasoningEffort>,
```

with:

```rust
pub reasoning: neo_ai::ReasoningSelection,
```

Update `TurnRequest::new` argument and initializer accordingly.

- [ ] **Step 5: Run runtime tests**

Run:

```bash
cargo test --package neo-agent-core --test runtime_turn -- runtime_rejects_reasoning_selection_when_model_lacks_reasoning_before_request --exact --nocapture
```

Expected: PASS.

Run:

```bash
cargo test --package neo-agent-core --test runtime_turn -- runtime_passes_reasoning_selection_into_chat_request_options --exact --nocapture
```

Expected: PASS.

- [ ] **Step 6: Checkpoint**

Do not commit unless explicitly authorized. Record remaining `reasoning_effort` compile errors for provider/task cleanup.

## Task 5: Provider Wire Mapping

**Files:**
- Modify: `crates/neo-ai/src/providers/openai/responses.rs`
- Modify: `crates/neo-ai/src/providers/openai/compatible.rs`
- Modify: `crates/neo-ai/src/providers/anthropic.rs`
- Modify: `crates/neo-ai/src/providers/google.rs`
- Modify: `crates/neo-ai/tests/real_provider_adapters.rs`
- Modify: `crates/neo-ai/tests/openai_compatible_provider.rs`

- [ ] **Step 1: Update provider tests to structured reasoning**

In `crates/neo-ai/tests/real_provider_adapters.rs`, replace the body of `openai_responses_client_serializes_reasoning_effort_with_encrypted_handoff` setup so it uses:

```rust
request.options.reasoning = ReasoningSelection::Effort {
    effort: ReasoningEffort::High,
};
```

Add this test near the Google reasoning tests:

```rust
#[tokio::test]
async fn google_generative_ai_client_serializes_budget_reasoning_selection() {
    let server = MockServer::start(vec![sse_response(&[json!({
        "candidates": [{
            "content": { "role": "model", "parts": [{ "text": "done" }] },
            "finishReason": "STOP"
        }]
    })])]);
    let client = GoogleGenerativeAiClient::new(server.url.clone(), "test-key");
    let mut request = request(ApiKind::GoogleGenerativeAi);
    request.options.reasoning = ReasoningSelection::BudgetTokens { budget_tokens: 8192 };

    let events = client
        .stream_chat(request)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert!(!events.is_empty());
    let sent = server.requests().pop().expect("request");
    assert_eq!(
        sent.body["generationConfig"]["thinkingConfig"]["thinkingBudget"],
        8192
    );
}
```

In `crates/neo-ai/tests/openai_compatible_provider.rs`, replace `openai_rejects_unsupported_reasoning_effort_without_posting` with:

```rust
#[tokio::test]
async fn openai_rejects_budget_reasoning_selection_without_posting() {
    let server = MockServer::start(Vec::new());
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");
    let request = request(RequestOptions {
        reasoning: ReasoningSelection::BudgetTokens { budget_tokens: 8192 },
        retries: Some(0),
        ..RequestOptions::default()
    });

    let error = client
        .stream_chat(request)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .next()
        .expect("one error")
        .expect_err("unsupported selection");

    assert!(
        error.to_string().contains("does not support budget reasoning"),
        "error: {error}"
    );
    assert!(server.requests().is_empty());
}
```

- [ ] **Step 2: Run one provider test and confirm failure**

Run:

```bash
cargo test --package neo-ai --test real_provider_adapters -- google_generative_ai_client_serializes_budget_reasoning_selection --exact --nocapture
```

Expected: FAIL because providers still read `reasoning_effort`.

- [ ] **Step 3: Implement OpenAI Responses mapping**

In `crates/neo-ai/src/providers/openai/responses.rs`, replace the `reasoning_effort` block with:

```rust
match &request.options.reasoning {
    ReasoningSelection::Off => {}
    ReasoningSelection::Effort { effort } => {
        body["reasoning"] = json!({
            "effort": effort.as_str(),
            "summary": "auto",
        });
        body["include"] = json!(["reasoning.encrypted_content"]);
    }
    ReasoningSelection::On => {
        body["reasoning"] = json!({
            "effort": "high",
            "summary": "auto",
        });
        body["include"] = json!(["reasoning.encrypted_content"]);
    }
    ReasoningSelection::BudgetTokens { .. } => {
        return Err(ProviderError::Unsupported(
            "OpenAI Responses provider does not support budget reasoning selections".to_owned(),
        ));
    }
}
```

Import `ReasoningSelection`.

- [ ] **Step 4: Implement OpenAI-compatible mapping**

In `crates/neo-ai/src/providers/openai/compatible.rs`, replace `openai_reasoning_effort` with:

```rust
fn openai_reasoning_selection(selection: &ReasoningSelection) -> Result<Option<&'static str>, ProviderError> {
    match selection {
        ReasoningSelection::Off => Ok(None),
        ReasoningSelection::On => Ok(Some("high")),
        ReasoningSelection::Effort { effort } => match effort {
            ReasoningEffort::Low => Ok(Some("low")),
            ReasoningEffort::Medium => Ok(Some("medium")),
            ReasoningEffort::High => Ok(Some("high")),
            ReasoningEffort::Minimal | ReasoningEffort::XHigh | ReasoningEffort::Max => {
                Err(ProviderError::Unsupported(format!(
                    "OpenAI-compatible provider type 'openai' supports reasoning effort low, medium, or high without an explicit model mapping; got {}",
                    effort.as_str()
                )))
            }
        },
        ReasoningSelection::BudgetTokens { .. } => Err(ProviderError::Unsupported(
            "OpenAI-compatible provider type 'openai' does not support budget reasoning selections"
                .to_owned(),
        )),
    }
}
```

Replace request-body insertion with:

```rust
if let Some(reasoning_effort) = openai_reasoning_selection(&request.options.reasoning)? {
    body["reasoning_effort"] = json!(reasoning_effort);
}
```

- [ ] **Step 5: Implement Anthropic and Google budget mapping**

In `crates/neo-ai/src/providers/anthropic.rs`, replace the old effort block with:

```rust
match &request.options.reasoning {
    ReasoningSelection::Off => {
        if let Some(temperature) = request.options.temperature {
            body["temperature"] = json!(rounded_f64(temperature));
        }
    }
    ReasoningSelection::On => {
        body["thinking"] = json!({
            "type": "enabled",
            "budget_tokens": 8192,
            "display": "summarized",
        });
    }
    ReasoningSelection::Effort { effort } => {
        body["thinking"] = json!({
            "type": "enabled",
            "budget_tokens": thinking_budget_tokens(*effort),
            "display": "summarized",
        });
    }
    ReasoningSelection::BudgetTokens { budget_tokens } => {
        body["thinking"] = json!({
            "type": "enabled",
            "budget_tokens": budget_tokens,
            "display": "summarized",
        });
    }
}
```

Update `thinking_budget_tokens` to handle `ReasoningEffort::Max`:

```rust
ReasoningEffort::Max => 32_768,
```

In `crates/neo-ai/src/providers/google.rs`, replace the old effort block with:

```rust
match &request.options.reasoning {
    ReasoningSelection::Off => {}
    ReasoningSelection::On => {
        generation_config.insert(
            "thinkingConfig".to_owned(),
            json!({
                "includeThoughts": true,
                "thinkingBudget": 8192,
            }),
        );
    }
    ReasoningSelection::Effort { effort } => {
        generation_config.insert(
            "thinkingConfig".to_owned(),
            json!({
                "includeThoughts": true,
                "thinkingBudget": thinking_budget_tokens(*effort),
            }),
        );
    }
    ReasoningSelection::BudgetTokens { budget_tokens } => {
        generation_config.insert(
            "thinkingConfig".to_owned(),
            json!({
                "includeThoughts": true,
                "thinkingBudget": budget_tokens,
            }),
        );
    }
}
```

Update Google `thinking_budget_tokens` for `Max`:

```rust
ReasoningEffort::Max => 32_768,
```

- [ ] **Step 6: Run provider tests**

Run:

```bash
cargo test --package neo-ai --test real_provider_adapters -- google_generative_ai_client_serializes_budget_reasoning_selection --exact --nocapture
```

Expected: PASS.

Run:

```bash
cargo test --package neo-ai --test openai_compatible_provider -- openai_rejects_budget_reasoning_selection_without_posting --exact --nocapture
```

Expected: PASS.

- [ ] **Step 7: Checkpoint**

Do not commit unless explicitly authorized. Record provider tests and remaining old-field references from `rg -n "reasoning_effort" crates`.

## Task 6: `/model` Reasoning Control UI

**Files:**
- Modify: `crates/neo-tui/src/dialogs/model_selector.rs`
- Modify: `crates/neo-tui/src/dialogs/tabbed_model_selector.rs`

- [ ] **Step 1: Add failing TUI tests**

In `crates/neo-tui/src/dialogs/model_selector.rs`, replace the old `thinking_toggle_respects_capabilities` test with these tests:

```rust
#[test]
fn effort_reasoning_model_renders_supported_values_only() {
    let state = ModelSelectorState::new(ModelSelectorOptions {
        models: vec![ModelEntry {
            alias: "openai/gpt-5.2".into(),
            provider_id: "openai".into(),
            display_name: "GPT-5.2".into(),
            model_id: "gpt-5.2".into(),
            capabilities: vec!["reasoning".into()],
            reasoning: ReasoningCapability::Effort {
                values: vec![ReasoningEffort::Low, ReasoningEffort::High, ReasoningEffort::XHigh],
                disable_supported: true,
            },
            max_context_tokens: Some(128_000),
        }],
        current_alias: "openai/gpt-5.2".into(),
        selected_alias: None,
        current_reasoning: ReasoningSelection::Effort {
            effort: ReasoningEffort::High,
        },
        theme: theme(),
    });

    let combined = state.render_lines(80).join("\n");
    assert!(combined.contains("Reasoning:"));
    assert!(combined.contains("off"));
    assert!(combined.contains("low"));
    assert!(combined.contains("[high]"));
    assert!(combined.contains("xhigh"));
    assert!(!combined.contains("medium"));
}

#[test]
fn budget_reasoning_model_renders_range_and_custom_error() {
    let mut state = ModelSelectorState::new(ModelSelectorOptions {
        models: vec![ModelEntry {
            alias: "google/gemini-2.5-flash".into(),
            provider_id: "google".into(),
            display_name: "Gemini 2.5 Flash".into(),
            model_id: "gemini-2.5-flash".into(),
            capabilities: vec!["reasoning".into()],
            reasoning: ReasoningCapability::BudgetTokens {
                min: Some(0),
                max: Some(24_576),
                disable_supported: true,
            },
            max_context_tokens: Some(1_048_576),
        }],
        current_alias: "google/gemini-2.5-flash".into(),
        selected_alias: None,
        current_reasoning: ReasoningSelection::BudgetTokens { budget_tokens: 8192 },
        theme: theme(),
    });

    state.handle_input(&InputEvent::Insert('e'));
    for ch in "40000".chars() {
        state.handle_input(&InputEvent::Insert(ch));
    }

    let combined = state.render_lines(80).join("\n");
    assert!(combined.contains("Reasoning budget:"));
    assert!(combined.contains("Range: 0..24576 tokens"));
    assert!(combined.contains("Custom: 40000"));
    assert!(combined.contains("budget must be between 0 and 24576 tokens"));
}

#[test]
fn no_reasoning_model_returns_off_selection() {
    let mut state = ModelSelectorState::new(ModelSelectorOptions {
        models: vec![ModelEntry {
            alias: "openai/gpt-4o-mini".into(),
            provider_id: "openai".into(),
            display_name: "GPT-4o mini".into(),
            model_id: "gpt-4o-mini".into(),
            capabilities: vec!["streaming".into()],
            reasoning: ReasoningCapability::None,
            max_context_tokens: Some(128_000),
        }],
        current_alias: "openai/gpt-4o-mini".into(),
        selected_alias: None,
        current_reasoning: ReasoningSelection::Off,
        theme: theme(),
    });

    assert!(state.render_lines(80).join("\n").contains("Reasoning: unavailable"));
    state.handle_input(&InputEvent::Submit);
    assert_eq!(
        state.take_result(),
        Some(ModelSelectorResult::Selected(ModelSelection {
            alias: "openai/gpt-4o-mini".into(),
            reasoning: ReasoningSelection::Off,
        }))
    );
}
```

Update test imports:

```rust
use neo_ai::{ReasoningCapability, ReasoningEffort, ReasoningSelection};
```

- [ ] **Step 2: Run one TUI test and confirm failure**

Run:

```bash
cargo test --package neo-tui --lib dialogs::model_selector::tests::effort_reasoning_model_renders_supported_values_only --exact --nocapture
```

Expected: FAIL because `ModelEntry.reasoning` and `current_reasoning` do not exist.

- [ ] **Step 3: Replace boolean selector state with structured state**

In `crates/neo-tui/src/dialogs/model_selector.rs`, update public structs:

```rust
pub struct ModelEntry {
    pub alias: String,
    pub provider_id: String,
    pub display_name: String,
    pub model_id: String,
    pub capabilities: Vec<String>,
    pub reasoning: neo_ai::ReasoningCapability,
    pub max_context_tokens: Option<u32>,
}

pub struct ModelSelection {
    pub alias: String,
    pub reasoning: neo_ai::ReasoningSelection,
}

pub struct ModelSelectorOptions {
    pub models: Vec<ModelEntry>,
    pub current_alias: String,
    pub selected_alias: Option<String>,
    pub current_reasoning: neo_ai::ReasoningSelection,
    pub theme: TuiTheme,
}
```

Replace `thinking_drafts` with:

```rust
reasoning_drafts: Vec<Option<neo_ai::ReasoningSelection>>,
budget_edit: Option<String>,
```

and replace `current_thinking` with:

```rust
current_reasoning: neo_ai::ReasoningSelection,
```

- [ ] **Step 4: Implement selection helpers**

Add these methods to `impl ModelSelectorState`:

```rust
fn effective_reasoning(&self, entry: &ModelEntry) -> neo_ai::ReasoningSelection {
    let idx = self.list.selected_index();
    self.reasoning_drafts
        .get(idx)
        .cloned()
        .flatten()
        .filter(|selection| entry.reasoning.supports(selection))
        .unwrap_or_else(|| default_reasoning_for(&entry.reasoning, &self.current_reasoning))
}

fn set_reasoning_draft(&mut self, selection: neo_ai::ReasoningSelection) {
    let idx = self.list.selected_index();
    if let Some(draft) = self.reasoning_drafts.get_mut(idx) {
        *draft = Some(selection);
    }
}
```

Add free helpers:

```rust
fn default_reasoning_for(
    capability: &neo_ai::ReasoningCapability,
    current: &neo_ai::ReasoningSelection,
) -> neo_ai::ReasoningSelection {
    if capability.supports(current) {
        return current.clone();
    }
    match capability {
        neo_ai::ReasoningCapability::None => neo_ai::ReasoningSelection::Off,
        neo_ai::ReasoningCapability::Toggle { .. } => neo_ai::ReasoningSelection::On,
        neo_ai::ReasoningCapability::Effort { values, disable_supported } => values
            .first()
            .copied()
            .map(|effort| neo_ai::ReasoningSelection::Effort { effort })
            .unwrap_or_else(|| {
                if *disable_supported {
                    neo_ai::ReasoningSelection::Off
                } else {
                    neo_ai::ReasoningSelection::On
                }
            }),
        neo_ai::ReasoningCapability::BudgetTokens { min, max, disable_supported } => {
            if *disable_supported {
                neo_ai::ReasoningSelection::Off
            } else {
                neo_ai::ReasoningSelection::BudgetTokens {
                    budget_tokens: min.or(*max).unwrap_or(8192),
                }
            }
        }
        neo_ai::ReasoningCapability::Combined { effort, budget, disable_supported, .. } => {
            if let Some(values) = effort
                && let Some(effort) = values.first().copied()
            {
                return neo_ai::ReasoningSelection::Effort { effort };
            }
            if let Some(bounds) = budget {
                return neo_ai::ReasoningSelection::BudgetTokens {
                    budget_tokens: bounds.min.or(bounds.max).unwrap_or(8192),
                };
            }
            if *disable_supported {
                neo_ai::ReasoningSelection::Off
            } else {
                neo_ai::ReasoningSelection::On
            }
        }
    }
}
```

- [ ] **Step 5: Render the Reasoning control area**

Replace the old thinking indicator block with:

```rust
if let Some(entry) = self.selected_entry() {
    lines.extend(self.render_reasoning_control(entry, inner_w));
}
```

Add `render_reasoning_control` that produces the spec text:

```rust
fn render_reasoning_control(&self, entry: &ModelEntry, width: usize) -> Vec<String> {
    let selection = self.effective_reasoning(entry);
    match &entry.reasoning {
        neo_ai::ReasoningCapability::None => vec![
            style_line(" Reasoning: unavailable for this model", width, self.theme.text_muted, Color::Reset),
            style_line(" Enter select", width, self.theme.text_muted, Color::Reset),
        ],
        neo_ai::ReasoningCapability::Toggle { .. } => vec![
            style_line(&format!(" Reasoning:  {}", toggle_label(&selection)), width, self.theme.brand, Color::Reset),
            style_line(" Space toggle · Enter select", width, self.theme.text_muted, Color::Reset),
        ],
        neo_ai::ReasoningCapability::Effort { values, disable_supported } => vec![
            style_line(
                &format!(" Reasoning:  {}", effort_labels(values, *disable_supported, &selection)),
                width,
                self.theme.brand,
                Color::Reset,
            ),
            style_line("             ←/→ choose · Space off · Enter select", width, self.theme.text_muted, Color::Reset),
        ],
        neo_ai::ReasoningCapability::BudgetTokens { min, max, disable_supported } => {
            self.render_budget_control(*min, *max, *disable_supported, &selection, width)
        }
        neo_ai::ReasoningCapability::Combined { effort: Some(values), disable_supported, .. } => vec![
            style_line(
                &format!(" Reasoning:  {}", effort_labels(values, *disable_supported, &selection)),
                width,
                self.theme.brand,
                Color::Reset,
            ),
            style_line("             ←/→ choose · Space off · Enter select", width, self.theme.text_muted, Color::Reset),
        ],
        neo_ai::ReasoningCapability::Combined { budget: Some(bounds), disable_supported, .. } => {
            self.render_budget_control(bounds.min, bounds.max, *disable_supported, &selection, width)
        }
        neo_ai::ReasoningCapability::Combined { .. } => vec![
            style_line(" Reasoning:  off   [on]", width, self.theme.brand, Color::Reset),
            style_line(" Catalog has no effort metadata for this model", width, self.theme.text_muted, Color::Reset),
        ],
    }
}
```

Implement `effort_labels`, `toggle_label`, and `render_budget_control` with exact strings from the spec.

- [ ] **Step 6: Update input handling**

In `handle_input`, replace left/right thinking toggles with calls that advance the visible reasoning selection:

```rust
InputEvent::MoveLeft => {
    self.move_reasoning(false);
    InputResult::Handled
}
InputEvent::MoveRight => {
    self.move_reasoning(true);
    InputResult::Handled
}
InputEvent::Insert(' ') => {
    self.toggle_reasoning_off();
    InputResult::Handled
}
InputEvent::Insert('e') => {
    if self.selected_entry().is_some_and(|entry| matches!(entry.reasoning, neo_ai::ReasoningCapability::BudgetTokens { .. })) {
        self.budget_edit = Some(String::new());
        InputResult::Handled
    } else {
        self.list.handle_key("e");
        InputResult::Handled
    }
}
```

When submitting, return:

```rust
reasoning: self.effective_reasoning(&entry),
```

and block submit when `budget_edit` contains an out-of-range value.

- [ ] **Step 7: Update tabbed selector options**

In `crates/neo-tui/src/dialogs/tabbed_model_selector.rs`, replace `current_thinking` with `current_reasoning: neo_ai::ReasoningSelection` in options, state, and calls to `ModelSelectorOptions`.

- [ ] **Step 8: Run TUI tests**

Run:

```bash
cargo test --package neo-tui --lib dialogs::model_selector::tests::effort_reasoning_model_renders_supported_values_only --exact --nocapture
```

Expected: PASS.

Run:

```bash
cargo test --package neo-tui --lib dialogs::model_selector::tests::budget_reasoning_model_renders_range_and_custom_error --exact --nocapture
```

Expected: PASS.

- [ ] **Step 9: Checkpoint**

Do not commit unless explicitly authorized. Include a rendered snippet from the test output or state that assertions passed.

## Task 7: Interactive `/model` Integration

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/model_picker.rs`
- Modify: `crates/neo-agent/src/modes/interactive/dialog_results.rs`
- Modify: `crates/neo-agent/src/modes/interactive/turn.rs`
- Modify: `crates/neo-agent/src/modes/interactive/controller_factory.rs`
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Add failing interactive test**

Append this test to `crates/neo-agent/src/modes/interactive/tests.rs`:

```rust
#[tokio::test]
async fn model_selection_applies_structured_reasoning_without_forcing_high() {
    let mut controller = test_controller().await;
    controller.local_config.as_mut().expect("config").models.insert(
        "openai/gpt-5.2".to_owned(),
        crate::config::ModelConfig {
            provider: "openai".to_owned(),
            model: "gpt-5.2".to_owned(),
            max_context_tokens: Some(128_000),
            max_output_tokens: Some(32_000),
            capabilities: vec!["streaming".to_owned(), "tools".to_owned(), "reasoning".to_owned()],
            reasoning: neo_ai::ReasoningCapability::Effort {
                values: vec![neo_ai::ReasoningEffort::Low, neo_ai::ReasoningEffort::Medium],
                disable_supported: true,
            },
            display_name: Some("GPT-5.2".to_owned()),
        },
    );

    controller.apply_model_selection(&neo_tui::dialogs::ModelSelection {
        alias: "openai/gpt-5.2".to_owned(),
        reasoning: neo_ai::ReasoningSelection::Effort {
            effort: neo_ai::ReasoningEffort::Medium,
        },
    });

    assert_eq!(
        controller.local_config.as_ref().expect("config").runtime.reasoning,
        neo_ai::ReasoningSelection::Effort {
            effort: neo_ai::ReasoningEffort::Medium,
        }
    );
    assert_eq!(
        controller.current_reasoning,
        neo_ai::ReasoningSelection::Effort {
            effort: neo_ai::ReasoningEffort::Medium,
        }
    );
}
```

- [ ] **Step 2: Run the interactive test and confirm failure**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::model_selection_applies_structured_reasoning_without_forcing_high --exact --nocapture --include-ignored
```

Expected: FAIL because controller state still uses `current_thinking`.

- [ ] **Step 3: Replace controller boolean state**

In `crates/neo-agent/src/modes/interactive/mod.rs`, replace:

```rust
current_thinking: bool,
```

with:

```rust
pub(crate) current_reasoning: neo_ai::ReasoningSelection,
```

Initialize it as `ReasoningSelection::Off`.

In `controller_factory.rs`, replace:

```rust
controller.current_thinking = config.runtime.reasoning_effort.is_some();
controller.tui.chrome_mut().set_thinking_enabled(controller.current_thinking);
```

with:

```rust
controller.current_reasoning = config.runtime.reasoning.clone();
controller
    .tui
    .chrome_mut()
    .set_thinking_enabled(controller.current_reasoning.is_enabled());
```

- [ ] **Step 4: Feed reasoning metadata into model entries**

In `crates/neo-agent/src/modes/interactive/model_picker.rs`, when creating `ModelEntry` from config, add:

```rust
reasoning: model.reasoning.clone(),
```

When creating from built-in `ModelSpec`, add:

```rust
reasoning: model.capabilities.reasoning.clone(),
```

Pass the current selection to the TUI selector:

```rust
current_reasoning: self.current_reasoning.clone(),
```

- [ ] **Step 5: Apply structured selection**

In `crates/neo-agent/src/modes/interactive/dialog_results.rs`, replace the thinking assignment block with:

```rust
self.current_reasoning = selection.reasoning.clone();
self.tui
    .chrome_mut()
    .set_thinking_enabled(selection.reasoning.is_enabled());
if let Some(config) = self.local_config.as_mut() {
    config.runtime.reasoning = selection.reasoning.clone();
    config.default_model.clone_from(&selection.alias);
    if let Some(model) = &self.active_model {
        config.default_provider.clone_from(&model.provider);
    }
}
```

Update status text:

```rust
let notice = if selection.reasoning.is_enabled() {
    format!("Switched to {} (reasoning: {})", selection.alias, reasoning_label(&selection.reasoning))
} else {
    format!("Switched to {}", selection.alias)
};
```

Add helper:

```rust
fn reasoning_label(selection: &neo_ai::ReasoningSelection) -> String {
    match selection {
        neo_ai::ReasoningSelection::Off => "off".to_owned(),
        neo_ai::ReasoningSelection::On => "on".to_owned(),
        neo_ai::ReasoningSelection::Effort { effort } => effort.as_str().to_owned(),
        neo_ai::ReasoningSelection::BudgetTokens { budget_tokens } => {
            format!("{budget_tokens} tokens")
        }
    }
}
```

- [ ] **Step 6: Start turns with structured reasoning**

In `crates/neo-agent/src/modes/interactive/turn.rs`, replace:

```rust
if self.current_thinking {
    Some(neo_ai::ReasoningEffort::High)
} else {
    None
},
```

with:

```rust
self.current_reasoning.clone(),
```

- [ ] **Step 7: Run interactive test**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::model_selection_applies_structured_reasoning_without_forcing_high --exact --nocapture --include-ignored
```

Expected: PASS.

- [ ] **Step 8: Update existing slash-new preservation test**

In `crates/neo-agent/src/modes/interactive/tests.rs`, update `slash_new_preserves_model_permission_thinking_and_plan_mode` so it sets:

```rust
controller.current_reasoning = neo_ai::ReasoningSelection::Effort {
    effort: neo_ai::ReasoningEffort::High,
};
controller.tui.chrome_mut().set_thinking_enabled(true);
```

and asserts:

```rust
assert!(controller.chrome().thinking_enabled());
assert_eq!(
    controller.current_reasoning,
    neo_ai::ReasoningSelection::Effort {
        effort: neo_ai::ReasoningEffort::High,
    }
);
```

- [ ] **Step 9: Checkpoint**

Do not commit unless explicitly authorized. Record exact test results.

## Task 8: Config Mutation And Catalog Import Persistence

**Files:**
- Modify: `crates/neo-agent/src/config/mutations.rs`
- Modify: `crates/neo-agent/src/main.rs`
- Modify: `crates/neo-agent/tests/cli_commands.rs`

- [ ] **Step 1: Add failing catalog import persistence test**

In `crates/neo-agent/src/config/mutations.rs`, update the existing `catalog_entry()` test helper so the reasoning model includes:

```rust
reasoning_options: vec![serde_json::json!({
    "type": "effort",
    "values": ["none", "low", "medium", "high"]
})],
```

Add this assertion to `add_provider_from_catalog_entry_replaces_existing_provider_models`:

```rust
assert!(contents.contains("reasoning = { type = \"effort\", values = [\"low\", \"medium\", \"high\"], disable_supported = true }"));
```

If TOML serialization expands the struct table, assert these exact substrings instead:

```rust
assert!(contents.contains("[models.openai-gpt-test.reasoning]"));
assert!(contents.contains("type = \"effort\""));
assert!(contents.contains("values = [\"low\", \"medium\", \"high\"]"));
assert!(contents.contains("disable_supported = true"));
```

- [ ] **Step 2: Run the config mutation test and confirm failure**

Run:

```bash
cargo test --package neo-agent --bin neo -- config::mutations::tests::add_provider_from_catalog_entry_replaces_existing_provider_models --exact --nocapture --include-ignored
```

Expected: FAIL until `ModelConfig.reasoning` is serialized by catalog import.

- [ ] **Step 3: Persist typed model reasoning**

In `catalog_model_config`, ensure the model config literal includes:

```rust
reasoning: model_info.reasoning.clone(),
```

Update any test helper `ModelConfig` literals in this file to include:

```rust
reasoning: neo_ai::ReasoningCapability::None,
```

or the reasoning capability being tested.

- [ ] **Step 4: Update CLI catalog detail JSON/text**

In `crates/neo-agent/src/main.rs`, update `catalog_models_json` so it emits reasoning metadata:

```rust
json!({
    "id": model.id,
    "name": model.name,
    "max_context_tokens": model.max_context_tokens,
    "max_output_tokens": model.max_output_tokens,
    "capabilities": model.capabilities,
    "reasoning": model.reasoning,
})
```

Update `catalog_model_text` to append `reasoning:<kind>` for reasoning models:

```rust
let reasoning = match &model.reasoning {
    neo_ai::ReasoningCapability::None => None,
    neo_ai::ReasoningCapability::Toggle { .. } => Some("reasoning:toggle"),
    neo_ai::ReasoningCapability::Effort { .. } => Some("reasoning:effort"),
    neo_ai::ReasoningCapability::BudgetTokens { .. } => Some("reasoning:budget"),
    neo_ai::ReasoningCapability::Combined { .. } => Some("reasoning:combined"),
};
```

Then push that label into the existing capability text list.

- [ ] **Step 5: Run config mutation test**

Run:

```bash
cargo test --package neo-agent --bin neo -- config::mutations::tests::add_provider_from_catalog_entry_replaces_existing_provider_models --exact --nocapture --include-ignored
```

Expected: PASS.

- [ ] **Step 6: Checkpoint**

Do not commit unless explicitly authorized. Record TOML output shape if assertions needed adjustment.

## Task 9: Remove Old Boolean/Effort Contract References

**Files:**
- Modify all files still returned by the search commands in this task.

- [ ] **Step 1: Search for old runtime field usage**

Run:

```bash
rg -n "reasoning_effort|current_thinking|thinking_drafts|effective_thinking|toggle_thinking" crates docs/superpowers/specs/2026-07-08-reasoning-effort-model-selector-design.md
```

Expected before cleanup: matches in old code and possibly the design spec background.

- [ ] **Step 2: Remove old code references**

For Rust code matches:

- Replace `reasoning_effort` runtime/request fields with `reasoning`.
- Replace `current_thinking` controller state with `current_reasoning`.
- Replace `thinking_drafts`, `effective_thinking`, and `toggle_thinking` selector internals with structured reasoning draft helpers.
- Keep `thinking` only for transcript content and provider reasoning text, not for `/model` selection state.

When updating `ReasoningPolicy` in `crates/neo-ai/src/reasoning.rs`, make it resolve to `ReasoningSelection`:

```rust
pub const fn resolve_for_model(self, model: &ModelSpec) -> ReasoningSelection {
    match self {
        Self::Off => ReasoningSelection::Off,
        Self::Auto if model.capabilities.reasoning.supports_reasoning() => {
            ReasoningSelection::Effort {
                effort: ReasoningEffort::Medium,
            }
        }
        Self::Auto => ReasoningSelection::Off,
        Self::Minimal => ReasoningSelection::Effort {
            effort: ReasoningEffort::Minimal,
        },
        Self::Low => ReasoningSelection::Effort {
            effort: ReasoningEffort::Low,
        },
        Self::Medium => ReasoningSelection::Effort {
            effort: ReasoningEffort::Medium,
        },
        Self::High => ReasoningSelection::Effort {
            effort: ReasoningEffort::High,
        },
        Self::XHigh => ReasoningSelection::Effort {
            effort: ReasoningEffort::XHigh,
        },
    }
}
```

- [ ] **Step 3: Confirm old runtime field is gone from Rust code**

Run:

```bash
rg -n "reasoning_effort|current_thinking|thinking_drafts|effective_thinking|toggle_thinking" crates -g '*.rs'
```

Expected: no matches for `/model` or runtime request state. Matches are acceptable only if they are legacy deserialization field names in `config/types.rs` or comments explaining migration.

- [ ] **Step 4: Run narrow regression tests**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_new_preserves_model_permission_thinking_and_plan_mode --exact --nocapture --include-ignored
```

Expected: PASS after updating the test to assert structured reasoning.

Run:

```bash
cargo test --package neo-ai --test env_and_options -- reasoning_effort_serializes_max_and_stable_names --exact --nocapture
```

Expected: PASS.

- [ ] **Step 5: Checkpoint**

Do not commit unless explicitly authorized. Include the final `rg` result in the handoff.

## Task 10: Final Focused Verification And Handoff

**Files:**
- No source edits unless one of the verification commands exposes a task-owned failure.

- [ ] **Step 1: Verify catalog parsing**

Run:

```bash
cargo test --package neo-ai --lib catalog::tests::catalog_model_capability_reads_effort_reasoning_options --exact --nocapture
```

Expected: PASS.

- [ ] **Step 2: Verify TUI effort rendering**

Run:

```bash
cargo test --package neo-tui --lib dialogs::model_selector::tests::effort_reasoning_model_renders_supported_values_only --exact --nocapture
```

Expected: PASS.

- [ ] **Step 3: Verify TUI budget rendering**

Run:

```bash
cargo test --package neo-tui --lib dialogs::model_selector::tests::budget_reasoning_model_renders_range_and_custom_error --exact --nocapture
```

Expected: PASS.

- [ ] **Step 4: Verify runtime rejection**

Run:

```bash
cargo test --package neo-agent-core --test runtime_turn -- runtime_rejects_reasoning_selection_when_model_lacks_reasoning_before_request --exact --nocapture
```

Expected: PASS.

- [ ] **Step 5: Verify interactive selection**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::model_selection_applies_structured_reasoning_without_forcing_high --exact --nocapture --include-ignored
```

Expected: PASS.

- [ ] **Step 6: Verify provider mapping**

Run:

```bash
cargo test --package neo-ai --test real_provider_adapters -- google_generative_ai_client_serializes_budget_reasoning_selection --exact --nocapture
```

Expected: PASS.

Run:

```bash
cargo test --package neo-ai --test openai_compatible_provider -- openai_rejects_budget_reasoning_selection_without_posting --exact --nocapture
```

Expected: PASS.

- [ ] **Step 7: Verify no broad old contract remains**

Run:

```bash
rg -n "reasoning_effort|current_thinking|thinking_drafts|effective_thinking|toggle_thinking" crates -g '*.rs'
```

Expected: no matches except the `FileRuntimeConfig.reasoning_effort` migration input and a migration test name if it still documents legacy config parsing.

- [ ] **Step 8: Final handoff**

Report:

```text
Implemented /model structured reasoning controls.
Verified:
- catalog effort parsing: PASS
- TUI effort rendering: PASS
- TUI budget rendering: PASS
- runtime unsupported selection rejection: PASS
- interactive selection does not force High: PASS
- Google budget provider mapping: PASS
- OpenAI-compatible unsupported budget rejection: PASS
- old boolean/effect contract search: PASS with only legacy migration input remaining
Git mutations were not run because Neo requires explicit per-instance authorization.
```

## Self-Review

- Spec coverage: The plan covers `/model` embedded controls, effort/budget/toggle/no-reasoning UI, models.dev `reasoning_options`, structured config, runtime validation, provider mapping, unsupported selection errors, and focused tests.
- Placeholder scan: No task relies on a future unspecified test or unnamed file. Every command names a package, target, and exact test.
- Type consistency: The plan consistently uses `ReasoningCapability`, `ReasoningSelection`, `ReasoningEffort`, `ModelEntry.reasoning`, `ModelSelection.reasoning`, `RuntimeConfig.reasoning`, and `RequestOptions.reasoning`.
- Scope check: This is one cross-layer feature, but each task is independently verifiable and maps to one boundary.
