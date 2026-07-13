# Custom Reasoning Effort Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Neo's closed reasoning-effort enum with a validated open string newtype that preserves provider-defined values end to end.

**Architecture:** `ReasoningEffort` remains the typed boundary but owns the provider string. Catalogs, configuration, capabilities, selectors, and OpenAI-compatible wire serialization all carry the same validated value without normalization or unknown-value filtering. Common values remain constants for built-in policy and UI presets.

**Tech Stack:** Rust 2024, serde, schemars, Cargo tests, rustdoc/Markdown documentation.

---

### Task 1: Open Core Effort Type

**Files:**
- Modify: `crates/neo-ai/src/options.rs`
- Modify: `crates/neo-ai/src/reasoning.rs`
- Test: `crates/neo-ai/tests/env_and_options.rs`

- [ ] **Step 1: Write failing tests for custom values and invalid empties**

Add tests that deserialize `"UltraMax"`, assert exact serialization and equality,
and assert that `""` and `"   "` fail. The custom test must use the public API:

```rust
let effort: ReasoningEffort = serde_json::from_str(r#""UltraMax""#).unwrap();
assert_eq!(effort.as_str(), "UltraMax");
assert_eq!(serde_json::to_string(&effort).unwrap(), r#""UltraMax""#);
```

- [ ] **Step 2: Verify the new test fails**

Run: `cargo test --package neo-ai --test env_and_options reasoning_effort_preserves_custom_provider_value --exact --nocapture`

Expected: FAIL because the closed enum rejects `UltraMax`.

- [ ] **Step 3: Replace the enum with one validated newtype**

Implement one public `ReasoningEffort(String)` type with common associated
constants, `as_str`, `TryFrom<String>`, `FromStr`, `Display`, manual serde
deserialization validation, and a string JSON schema. Do not keep the enum or a
`Custom` variant. Update `ReasoningSelection::effort` and reasoning policy code
to clone owned values where `Copy` was previously assumed.

- [ ] **Step 4: Verify the focused core tests**

Run each exact test in `env_and_options`: custom preservation, empty rejection,
stable common names, structured selection round trip, and capability validation.

### Task 2: Preserve Custom Values Through Catalog And Providers

**Files:**
- Modify: `crates/neo-ai/src/catalog.rs`
- Modify: `crates/neo-ai/src/providers/openai/compatible.rs`
- Modify: `crates/neo-ai/src/providers/openai/responses.rs`
- Modify as required by ownership: `crates/neo-ai/src/providers/anthropic.rs`
- Modify as required by ownership: `crates/neo-ai/src/providers/google.rs`
- Test: `crates/neo-ai/src/catalog.rs`
- Test: `crates/neo-ai/tests/openai_compatible_provider.rs`

- [ ] **Step 1: Write failing catalog and wire tests**

Add a catalog test whose effort values include `"ultramax"` and assert the
resulting capability retains it. Add an OpenAI-compatible request test asserting
`reasoning_effort == "ultramax"`.

- [ ] **Step 2: Verify both tests fail for the expected reasons**

Run one exact `neo-ai --lib` catalog test and one exact
`neo-ai --test openai_compatible_provider` test.

- [ ] **Step 3: Remove the catalog allowlist and migrate provider ownership**

Parse every non-empty catalog value through `ReasoningEffort`; treat `none` only
as the existing disable marker. Provider serializers use `as_str()` and clone
only where ownership requires it. Budget/toggle adapters keep their current
protocol behavior.

- [ ] **Step 4: Verify catalog and provider boundaries**

Re-run the two exact red tests, plus the existing exact OpenAI Responses,
Anthropic budget, and Google budget adapter tests touched by compilation changes.

### Task 3: Make TUI And Config Consumers Ownership-Safe

**Files:**
- Modify: `crates/neo-agent/src/config/types.rs`
- Modify: `crates/neo-agent/src/config/mod.rs`
- Modify: `crates/neo-agent/src/config/mutations.rs`
- Modify: `crates/neo-agent/src/modes/run/mod.rs`
- Modify: `crates/neo-agent/src/modes/interactive/custom_endpoint_provider.rs`
- Modify: `crates/neo-tui/src/dialogs/custom_endpoint_wizard.rs`
- Modify: `crates/neo-tui/src/dialogs/model_selector.rs`
- Modify: `crates/neo-tui/src/dialogs/tabbed_model_selector.rs`
- Test: existing unit tests in those targets

- [ ] **Step 1: Write a failing selector test for a model-declared custom value**

Construct a model with `ReasoningEffort::try_from("UltraMax")`, render its
effort segments, select the value, and assert the submitted selection preserves
the exact case.

- [ ] **Step 2: Verify the selector test fails**

Run the exact `neo-tui --lib` selector test.

- [ ] **Step 3: Migrate closed-enum assumptions**

Replace moves from borrowed efforts with clones. Keep the custom endpoint
wizard's six common values as presets, but render model selector values from
model capabilities without filtering. Sort common presets by their known order
and leave custom values stable after them.

- [ ] **Step 4: Verify exact TUI, config, and runtime tests**

Run the new selector test, custom endpoint reasoning-page test, structured
runtime config test, configured custom reasoning propagation test, and runtime
capability-validation test using exact target and test-name filters.

### Task 4: Document And Verify The Complete Path

**Files:**
- Modify: `docs/en/configuration/providers.md`
- Modify: `docs/zh/configuration/providers.md`
- Review: `docs/superpowers/specs/2026-07-13-custom-reasoning-effort-design.md`

- [ ] **Step 1: Add concise bilingual user documentation**

Document this configuration shape:

```toml
reasoning = { mode = "effort", effort = "ultramax" }
```

State that the provider defines supported values, values are case-sensitive and
preserved exactly, empty values are invalid, and users should consult provider
model documentation.

- [ ] **Step 2: Run focused formatting and lint checks**

Run `cargo fmt --all --check`, `git diff --check`, and Clippy for each touched
crate using one explicit target selector. Any unrelated concurrent failure is
reported rather than repaired.

- [ ] **Step 3: Re-run the highest-value exact regression tests**

Re-run the exact custom serde, catalog preservation, OpenAI-compatible wire,
TUI selection, config propagation, and runtime validation tests. Inspect the
final diff for leftover closed-enum matches and accidental compatibility paths.

- [ ] **Step 4: Record completion without git mutation**

Store the resolved error/design context in ICM and report the modified files and
fresh verification evidence. Do not add, commit, switch branches, or otherwise
mutate git state without explicit authorization.

