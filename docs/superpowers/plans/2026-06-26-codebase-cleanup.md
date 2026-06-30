# Neo Codebase Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove all dead code, obsolete tests, and duplicated logic identified in the full audit — leaving a clean, kernel-grade codebase with zero `#[allow(dead_code)]` in production, no copy-paste utilities, and no brittle/tautological tests.

**Architecture:** Cleanup proceeds in 14 phases ordered by risk: pure deletions first (P1–P4), stale comment fixes (P5), test cleanup (P6–P8), then logic consolidation (P9–P14). Each phase compiles independently. Dead-code deletions verify via `cargo check`; consolidation tasks verify via focused tests.

**Tech Stack:** Rust 2024 edition, `cargo check` for compilation, `cargo nextest run` for test verification, `cargo clippy` for lint.

---

## Pre-Flight

**Before starting any task, read these files:**
- `AGENTS.md` — workspace rules (especially: never `git checkout`/`git restore` files; use `cargo nextest run`)
- `crates/neo-ai/src/lib.rs` — re-export map for neo-ai
- `crates/neo-agent-core/src/lib.rs` — re-export map for agent-core
- `crates/neo-agent-core/src/mode/mod.rs` — explicit re-export list to edit

**Verification commands:**
```bash
# After each deletion task:
cargo check -p <crate-name> 2>&1 | head -30

# After test-modifying tasks:

# After each phase completion:
cargo clippy -p <crate-name> -- -D warnings 2>&1 | head -30
```

---

## Phase 1: Delete Dead Code in `neo-ai`

**Crate:** `crates/neo-ai/`

### Task 1.1: Delete dead pub functions and struct field

**Files:**
- Modify: `crates/neo-ai/src/catalog.rs` — delete `catalog_model_to_capabilities` (line ~263) and `reasoning_key` field (line ~92) + its write at line ~192 + helper `catalog_reasoning_key` at line ~228
- Modify: `crates/neo-ai/src/registry.rs` — delete `supports_api` (~256), `upsert` (~131), `list` (~141), `credential_status` (~146)
- Modify: `crates/neo-ai/src/types.rs` — delete `api_kind_from_str` (~93)
- Modify: `crates/neo-ai/src/env_api_keys.rs` — delete `find_env_keys` (~21) and `env_api_key` (~26)
- Modify: `crates/neo-ai/src/lib.rs` — remove dead items from re-exports (lines ~15, ~29)
- Modify: `docs/providers.md` — update or remove references to deleted `find_env_keys` / `env_api_key` at line ~50

- [ ] **Step 1: Delete `catalog_model_to_capabilities`**

Open `crates/neo-ai/src/catalog.rs`. Find the function starting around line 263:
```rust
pub fn catalog_model_to_capabilities(info: &CatalogModelInfo) -> ModelCapabilities {
```
Delete the entire function body.

- [ ] **Step 2: Delete `reasoning_key` field + helper**

In `crates/neo-ai/src/catalog.rs`:
- Delete the field `pub reasoning_key: Option<String>,` from `CatalogModelInfo` struct (~line 92)
- Delete the assignment `reasoning_key: catalog_reasoning_key(m),` from the construction site (~line 192)
- Delete the helper function `fn catalog_reasoning_key(...)` (~line 228)

- [ ] **Step 3: Delete dead registry methods**

In `crates/neo-ai/src/registry.rs`, delete these four methods from `impl ProviderRegistry` / `impl ProviderSpec`:
```rust
pub fn upsert(&mut self, provider: ProviderSpec)  // ~line 131
pub fn list(&self) -> Vec<ProviderSpec>           // ~line 141
pub fn credential_status(&self, provider: &str) -> Option<ProviderCredentialStatus>  // ~line 146
pub fn supports_api(&self, api: &ApiKind) -> bool  // ~line 256, on ProviderSpec
```

- [ ] **Step 4: Delete `api_kind_from_str`**

In `crates/neo-ai/src/types.rs`, delete (~line 93):
```rust
pub fn api_kind_from_str(s: &str) -> Option<ApiKind> { ... }
```

- [ ] **Step 5: Delete `find_env_keys` and `env_api_key`**

In `crates/neo-ai/src/env_api_keys.rs`, delete (~lines 21-30):
```rust
pub fn find_env_keys(provider: &str) -> Vec<String> { ... }
pub fn env_api_key(provider: &str) -> Option<String> { ... }
```
Keep their `_from` variants.

- [ ] **Step 6: Update `lib.rs` re-exports**

In `crates/neo-ai/src/lib.rs`:
- Line ~15: change `pub use env_api_keys::{env_api_key, env_api_key_from, find_env_keys, find_env_keys_from};` to `pub use env_api_keys::{env_api_key_from, find_env_keys_from};`
- Line ~29: change `pub use types::{ApiType, api_kind_from_str};` to `pub use types::ApiType;`
- If line ~28 has `pub use types::*;`, the `api_kind_from_str` will be naturally gone after deletion.

- [ ] **Step 7: Update docs**

In `docs/providers.md` around line 50, update or remove references to `find_env_keys(provider)` and `env_api_key(provider)`. Replace with their `_from` variants if the documentation still needs the concept, or remove the paragraph if it's stale.

- [ ] **Step 8: Verify compilation**

Run: `cargo check -p neo-ai 2>&1 | head -30`
Expected: PASS — zero errors.

- [ ] **Step 9: Verify workspace**

Run: `cargo check --workspace 2>&1 | head -30`
Expected: PASS — no downstream breakage.

- [ ] **Step 10: Commit**

```bash
git add crates/neo-ai/src/ crates/neo-ai/tests/ docs/providers.md
git commit -m "refactor(neo-ai): delete 8 dead pub functions, reasoning_key field, and no-op match

Removed: catalog_model_to_capabilities, ProviderSpec::supports_api,
ProviderRegistry::{upsert,list,credential_status}, api_kind_from_str,
find_env_keys, env_api_key, CatalogModelInfo::reasoning_key field.
Updated lib.rs re-exports and docs/providers.md."
```

### Task 1.2: Delete no-op cache match in anthropic provider

**Files:**
- Modify: `crates/neo-ai/src/providers/anthropic.rs:214-216`

- [ ] **Step 1: Delete the no-op match**

In `crates/neo-ai/src/providers/anthropic.rs`, find (~line 214):
```rust
match request.options.cache {
    CacheRetention::None | CacheRetention::Short | CacheRetention::Long => {}
}
```
Delete the entire block. If this was the only use of `CacheRetention` in this file, the unused import warning will guide you to also remove the import.

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p neo-ai 2>&1 | head -30`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add crates/neo-ai/src/providers/anthropic.rs
git commit -m "refactor(neo-ai): remove no-op cache match in anthropic provider

All three CacheRetention arms were empty bodies — pure dead code."
```

---

## Phase 2: Delete Dead Code in `neo-agent-core`

**Crate:** `crates/neo-agent-core/`

### Task 2.1: Delete `AgentMode` enum

**Files:**
- Modify: `crates/neo-agent-core/src/mode/plan.rs` — delete enum (~line 7-12)
- Modify: `crates/neo-agent-core/src/mode/mod.rs` — remove from re-export (~line 4)

- [ ] **Step 1: Delete the enum**

In `crates/neo-agent-core/src/mode/plan.rs`, delete:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentMode {
    #[default]
    Default,
    Plan,
}
```

- [ ] **Step 2: Update re-export**

In `crates/neo-agent-core/src/mode/mod.rs`, line ~4:
Change `pub use plan::{AgentMode, PlanData, PlanInjectionVariant, PlanMode};`
To: `pub use plan::{PlanData, PlanInjectionVariant, PlanMode};`

- [ ] **Step 3: Verify**

Run: `cargo check -p neo-agent-core 2>&1 | head -30`
Expected: PASS

Run: `cargo check --workspace 2>&1 | head -30`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/neo-agent-core/src/mode/
git commit -m "refactor(neo-agent-core): delete dead AgentMode enum

Zero references anywhere in the workspace. The runtime uses PlanMode
and PermissionMode separately; AgentMode was never wired."
```

### Task 2.2: Remove `#[allow(dead_code)]` on `PreparedToolCall` and delete dead fields

**Files:**
- Modify: `crates/neo-agent-core/src/runtime.rs` — lines ~88-100

- [ ] **Step 1: Edit the struct**

In `crates/neo-agent-core/src/runtime.rs`, find the struct (~line 91):
```rust
#[allow(dead_code)]
struct PreparedToolCall {
    tool_call: AgentToolCall,
    result: PreparedToolCallResult,
    scheduling: ToolSchedulingClass,
    access: ToolAccess,
}
```

Change to:
```rust
struct PreparedToolCall {
    result: PreparedToolCallResult,
    access: ToolAccess,
}
```

- [ ] **Step 2: Fix all construction sites**

Search for all places `PreparedToolCall { ... }` is constructed (grep `PreparedToolCall {`). At each site, remove the `tool_call:` and `scheduling:` fields from the struct literal. Keep `result:` and `access:`.

- [ ] **Step 3: Verify**

Run: `cargo check -p neo-agent-core 2>&1 | head -30`
Expected: PASS — no warnings about unused fields.

- [ ] **Step 4: Commit**

```bash
git add crates/neo-agent-core/src/runtime.rs
git commit -m "refactor(neo-agent-core): remove dead PreparedToolCall fields

Deleted tool_call and scheduling fields (written, never read).
Removed #[allow(dead_code)] suppression."
```

### Task 2.3: Delete dead methods in `ToolRegistry`, `OAuthStore`, `SkillStore`, `ExtensionRunner`, `ExtensionLifecycleStore`, `GoalStore`

**Files:**
- Modify: `crates/neo-agent-core/src/tools/mod.rs` — delete `retain_named` (~404) and `remove_named` (~408)
- Modify: `crates/neo-agent-core/src/oauth/store.rs` — delete `remove_token` (~151)
- Modify: `crates/neo-agent-core/src/skills/mod.rs` — delete `available_for_slash` (~160)
- Modify: `crates/neo-agent-core/src/tools/extensions/runner.rs` — delete `child_id` (~105)
- Modify: `crates/neo-agent-core/src/tools/extensions/installation.rs` — delete `lifecycle` (~97)
- Modify: `crates/neo-agent-core/src/goal/mod.rs` — delete `is_empty` (~155)

- [ ] **Step 1: Delete `retain_named` and `remove_named`**

In `crates/neo-agent-core/src/tools/mod.rs`, delete (~line 404-410):
```rust
pub fn retain_named(&mut self, names: &BTreeSet<String>) {
    self.tools.retain(|name, _| names.contains(name));
}

pub fn remove_named(&mut self, names: &BTreeSet<String>) {
    self.tools.retain(|name, _| !names.contains(name));
}
```

- [ ] **Step 2: Delete `remove_token`**

In `crates/neo-agent-core/src/oauth/store.rs`, delete (~line 151):
```rust
pub fn remove_token(&mut self, key: &str) -> bool {
    self.remove(key)
}
```

- [ ] **Step 3: Delete `available_for_slash`**

In `crates/neo-agent-core/src/skills/mod.rs`, delete (~line 160):
```rust
pub fn available_for_slash(&self) -> Vec<&LoadedSkill> {
    self.skills.values().collect()
}
```

- [ ] **Step 4: Delete `child_id`**

In `crates/neo-agent-core/src/tools/extensions/runner.rs`, delete (~line 105):
```rust
pub fn child_id(&self) -> Option<u32> {
    self.child.id()
}
```

- [ ] **Step 5: Delete `lifecycle`**

In `crates/neo-agent-core/src/tools/extensions/installation.rs`, delete (~line 97):
```rust
pub fn lifecycle(&self) -> ExtensionLifecycleStore {
    ExtensionLifecycleStore::new(&self.state_path)
}
```

- [ ] **Step 6: Delete `GoalStore::is_empty`**

In `crates/neo-agent-core/src/goal/mod.rs`, delete (~line 155):
```rust
pub fn is_empty(&self) -> bool {
    self.active.is_none() && self.queue.is_empty()
}
```

- [ ] **Step 7: Verify**

Run: `cargo check -p neo-agent-core 2>&1 | head -30`
Expected: PASS

Run: `cargo check --workspace 2>&1 | head -30`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add crates/neo-agent-core/src/
git commit -m "refactor(neo-agent-core): delete 7 dead methods

Removed: ToolRegistry::{retain_named, remove_named},
OAuthStore::remove_token, SkillStore::available_for_slash,
ExtensionRunner::child_id, ExtensionLifecycleStore::lifecycle,
GoalStore::is_empty. All had zero callers."
```

### Task 2.4: Handle `AgentEvent::PlanModeCancelled` variant

**Files:**
- Modify: `crates/neo-agent-core/src/events.rs` — delete variant (~244)
- Modify: `crates/neo-agent-core/src/runtime.rs` — delete match arms (~730, ~1380)
- Modify: `crates/neo-tui/src/chrome.rs` — delete match arm (~1049)
- Modify: `crates/neo-agent-core/tests/` — fix any serialization test that constructs this variant (~events.rs:410 in test module)

- [ ] **Step 1: Check what the serialization test does**

Search for `PlanModeCancelled` in the test at `events.rs:410` area. This is inside the `events.rs` file's `#[cfg(test)]` module — a serialization roundtrip test.

- [ ] **Step 2: Delete the variant**

In `crates/neo-agent-core/src/events.rs`, delete the enum variant (~line 244):
```rust
PlanModeCancelled {
    turn: u32,
    id: String,
},
```

- [ ] **Step 3: Remove from serialization test**

In the serialization roundtrip test in `events.rs` test module, remove the `PlanModeCancelled { .. }` entry from the test vector list. This is likely a `Vec::<AgentEvent>::new()` or array of events being serialized.

- [ ] **Step 4: Delete match arms in runtime.rs**

In `crates/neo-agent-core/src/runtime.rs`, search for `PlanModeCancelled`. Delete the match arms at the lines where it appears (~730, ~1380). These are defensive fallback arms — removing them is safe because the variant no longer exists.

- [ ] **Step 5: Delete match arm in chrome.rs**

In `crates/neo-tui/src/chrome.rs`, search for `PlanModeCancelled`. Delete the match arm (~line 1049).

- [ ] **Step 6: Verify**

Run: `cargo check --workspace 2>&1 | head -50`
Expected: PASS — the compiler will surface any remaining references.

Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add crates/neo-agent-core/src/events.rs crates/neo-agent-core/src/runtime.rs crates/neo-tui/src/chrome.rs
git commit -m "refactor: delete dead AgentEvent::PlanModeCancelled variant

Never emitted by runtime (cancellation uses PlanUpdated{enabled:false}).
Removed variant, match arms in runtime and chrome, and serialization test entry."
```

### Task 2.5: Delete dead `_kind` field on `SupervisedProcess`

**Files:**
- Modify: `crates/neo-agent-core/src/tools/process_supervisor.rs`

- [ ] **Step 1: Read the file to understand the struct**

Read `crates/neo-agent-core/src/tools/process_supervisor.rs` fully (it's small, ~70 lines).

- [ ] **Step 2: Delete `_kind` field**

Remove `_kind: ProcessKind` from the `SupervisedProcess` struct definition (~line 17).

- [ ] **Step 3: Remove `kind` parameter from `register` method**

If the `register` method takes a `kind: ProcessKind` parameter only to pass it to `_kind`, remove the parameter and update callers. Search for `.register(` to find call sites in `mcp_manager.rs` or elsewhere.

- [ ] **Step 4: Check if `ProcessKind` enum is now dead**

If `ProcessKind` was only used for `_kind`, delete the enum definition too.

- [ ] **Step 5: Verify**

Run: `cargo check -p neo-agent-core 2>&1 | head -30`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/neo-agent-core/src/tools/process_supervisor.rs
git commit -m "refactor(neo-agent-core): remove dead _kind field on SupervisedProcess

Field was stored but never read. ProcessKind enum removed if no other users."
```

---

## Phase 3: Delete Dead Code in `neo-tui`

**Crate:** `crates/neo-tui/`

### Task 3.1: Delete dead structs, functions, and trait method (batch)

**Files (all in `crates/neo-tui/src/`):**
- `ansi.rs:486` — delete `StyledLine` struct + impl
- `components.rs:10,23` — delete `ChromeLayout` struct + `chrome_layout()` fn
- `input/raw_input.rs:462-476` — delete `ParsedKitty.event_type` field, `KeyEventType` enum, `parse_event_type()` fn
- `core/component.rs:22` — delete `invalidate()` default method from `Component` trait
- `transcript/tool_call.rs:82` — delete `append_progress()`
- `transcript/tool_call.rs:167` — delete `into_state()`
- `chrome.rs:1307` — delete `open_model_selector()`
- `chrome.rs:1375` — delete `open_text_input()`
- `chrome.rs:3434` — delete `promote_oldest_follow_up_to_steer()`
- `chrome.rs:2494` — delete `clear_filter()`
- `chrome.rs:1402` — delete `trust_dialog_result()` (keep `take_trust_dialog_result()`)
- `chrome.rs:4341` — delete `visible_items()`
- `chrome.rs:4198` — delete `with_selected()`
- `terminal/renderer.rs:451` — delete `force_clear()`
- `transcript/entry.rs:220` — delete `status_severity()`
- `transcript/tool_renderers.rs:22` — delete `tool_header()`
- `input/raw_input.rs:448` — delete `RawInputParser::clear()`

- [ ] **Step 1: Delete `StyledLine` struct + impl**

In `crates/neo-tui/src/ansi.rs`, delete from ~line 486 to the end of the impl block (includes `new()` and `to_ansi()`).

- [ ] **Step 2: Delete `ChromeLayout` + `chrome_layout()`**

In `crates/neo-tui/src/components.rs`, delete the struct definition (~line 10) and the function (~line 23 to its end).

- [ ] **Step 3: Delete `ParsedKitty.event_type` field + `KeyEventType` enum + `parse_event_type()` fn**

In `crates/neo-tui/src/input/raw_input.rs`:
- Delete the `event_type: KeyEventType` field from `ParsedKitty` struct (~line 468)
- Delete the `enum KeyEventType { Press, Repeat, Release }` (~line 472)
- Delete the `fn parse_event_type(...)` helper that constructs `KeyEventType` values
- Remove all `event_type:` assignments at construction sites (~lines 539, 560, 586, 601)
- Remove the `#[allow(dead_code)]` on `ParsedKitty` if it was only for this field

- [ ] **Step 4: Delete `Component::invalidate()`**

In `crates/neo-tui/src/core/component.rs`, delete the default method (~line 22):
```rust
fn invalidate(&mut self) {}
```

- [ ] **Step 5: Delete `append_progress()` and `into_state()`**

In `crates/neo-tui/src/transcript/tool_call.rs`, delete `append_progress` (~line 82) and `into_state` (~line 167).

- [ ] **Step 6: Delete dead chrome.rs methods**

In `crates/neo-tui/src/chrome.rs`, delete these methods (search for each by name):
- `open_model_selector` (~line 1307)
- `open_text_input` (~line 1375)
- `trust_dialog_result` (~line 1402) — keep `take_trust_dialog_result`!
- `clear_filter` (~line 2494)
- `promote_oldest_follow_up_to_steer` (~line 3434)
- `with_selected` (~line 4198)
- `visible_items` (~line 4341)

- [ ] **Step 7: Delete remaining dead functions**

- `crates/neo-tui/src/terminal/renderer.rs:451` — delete `force_clear()`
- `crates/neo-tui/src/transcript/entry.rs:220` — delete `status_severity()`
- `crates/neo-tui/src/transcript/tool_renderers.rs:22` — delete `tool_header()`
- `crates/neo-tui/src/input/raw_input.rs:448` — delete `RawInputParser::clear()`

- [ ] **Step 8: Verify compilation**

Run: `cargo check -p neo-tui 2>&1 | head -50`
Expected: PASS

Run: `cargo check --workspace 2>&1 | head -50`
Expected: PASS

- [ ] **Step 9: Run clippy**

Run: `cargo clippy -p neo-tui -- -D warnings 2>&1 | head -30`
Expected: PASS

- [ ] **Step 10: Commit**

```bash
git add crates/neo-tui/src/
git commit -m "refactor(neo-tui): delete 17 dead pub fns, structs, and trait method

Removed: StyledLine, ChromeLayout+chrome_layout(), ParsedKitty.event_type
+ KeyEventType + parse_event_type(), Component::invalidate(),
append_progress(), into_state(), open_model_selector(), open_text_input(),
trust_dialog_result() (borrow variant), clear_filter(),
promote_oldest_follow_up_to_steer(), with_selected(), visible_items(),
force_clear(), status_severity(), tool_header(), RawInputParser::clear()."
```

---

## Phase 4: Delete Dead Code in `neo-agent`

**Crate:** `crates/neo-agent/`

### Task 4.1: Delete dead methods and fix `#[allow(dead_code)]` items

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive.rs` — handle 6 `#[allow(dead_code)]` items
- Modify: `crates/neo-agent/src/modes/run.rs` — delete 3 confirmed dead fields on `PromptApprovalRequest`

- [ ] **Step 1: Handle test-only helpers**

In `crates/neo-agent/src/modes/interactive.rs`, for each of these:
- `type_text()` (~line 985) — **keep** but move inside `#[cfg(test)]` impl block, or add `#[cfg(test)]` attribute to the method
- `submit_prompt()` (~line 1291) — same: add `#[cfg(test)]`
- `empty_session_loader()` (~line 6159) — add `#[cfg(test)]`
- `empty_session_forker()` (~line 6168) — add `#[cfg(test)]`

For each, remove the `#[allow(dead_code)]` and replace with `#[cfg(test)]`.

- [ ] **Step 2: Delete truly dead methods**

In `crates/neo-agent/src/modes/interactive.rs`:
- `app()` (~line 4814) — delete entirely, remove `#[allow(dead_code)]` and `#[must_use]`
- `leave()` (~line 5935) — delete entirely, remove `#[allow(dead_code)]`

- [ ] **Step 3: Delete dead `PromptApprovalRequest` fields**

In `crates/neo-agent/src/modes/run.rs`, delete these 3 fields from the struct (~lines 989, 1006, 1009):
```rust
#[allow(dead_code)]
pub operation: PermissionOperation,
```
```rust
#[allow(dead_code)]
pub prefix_rule: Option<neo_agent_core::PrefixApprovalRule>,
```
```rust
#[allow(dead_code)]
pub session_scope: Option<neo_agent_core::SessionApprovalScope>,
```

**Keep** `session_option_label` and `prefix_option_label` — they ARE read by `register_pending_approval()` in interactive.rs:3458-3459.

- [ ] **Step 4: Fix construction sites of `PromptApprovalRequest`**

Search for `PromptApprovalRequest {` construction sites. At each, remove the deleted fields (`operation:`, `prefix_rule:`, `session_scope:`) from the literal. If any test destructures these fields as `_operation`, `_prefix_rule`, `_session_scope`, remove them from the destructure pattern.

- [ ] **Step 5: Verify**

Run: `cargo check -p neo-agent 2>&1 | head -50`
Expected: PASS

Run: `cargo check --workspace 2>&1 | head -50`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/neo-agent/src/modes/
git commit -m "refactor(neo-agent): clean up dead code and test-only helpers

Deleted: app(), leave() (zero callers).
Moved test-only helpers to #[cfg(test)]: type_text, submit_prompt,
empty_session_loader, empty_session_forker.
Removed 3 dead PromptApprovalRequest fields: operation, prefix_rule,
session_scope (written, never read by consumer)."
```

---

## Phase 5: Fix Stale TODO Comments

### Task 5.1: Remove stale "unused" TODOs from `stdio.rs`

**Files:**
- Modify: `crates/neo-agent-core/src/tools/mcp/stdio.rs` — lines ~11-12, ~26-27

- [ ] **Step 1: Read the file**

Read `crates/neo-agent-core/src/tools/mcp/stdio.rs`.

- [ ] **Step 2: Remove the stale comments**

Delete or update the TODO comments that say `StdioConfig` / `build_stdio_client` are "currently unused while the rmcp migration is in progress." They ARE used — by `mcp_manager.rs:850` and `run.rs:1909-1911`.

Replace the module doc comment with an accurate one, e.g.:
```rust
//! Stdio MCP client builder.
```

Remove the `// TODO:` lines.

- [ ] **Step 3: Verify**

Run: `cargo check -p neo-agent-core 2>&1 | head -20`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/neo-agent-core/src/tools/mcp/stdio.rs
git commit -m "docs(neo-agent-core): remove stale 'unused' TODOs from stdio.rs

The rmcp migration is complete; StdioConfig and build_stdio_client are
actively used by mcp_manager.rs and run.rs."
```

---

## Phase 6: Delete Low-Value Tests

### Task 6.1: Delete `fake_provider.rs` integration test

**Files:**
- Delete: `crates/neo-ai/tests/fake_provider.rs`

- [ ] **Step 1: Delete the file**

```bash
rm crates/neo-ai/tests/fake_provider.rs
```

- [ ] **Step 2: Verify**

Run: `cargo check -p neo-ai --tests 2>&1 | head -20`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add -A crates/neo-ai/tests/
git commit -m "test(neo-ai): delete fake_provider.rs — FakeModelClient self-referential test

The test feeds events to FakeModelClient and asserts the same events come
back. This tests the test double against itself, providing zero production
coverage."
```

### Task 6.2: Delete tautology test `request_options_default_to_short_cache`

**Files:**
- Modify: `crates/neo-ai/tests/env_and_options.rs` — delete function at ~line 180

- [ ] **Step 1: Delete the test function**

Delete:
```rust
#[test]
fn request_options_default_to_short_cache_without_transport_overrides() {
    let options = RequestOptions::default();
    assert_eq!(options.temperature, None);
    // ... all tautological assertions ...
}
```

- [ ] **Step 2: Verify**

Run: `cargo check -p neo-ai --tests 2>&1 | head -20`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add crates/neo-ai/tests/env_and_options.rs
git commit -m "test(neo-ai): delete tautology test request_options_default_to_short_cache

Asserting Default::default() equals Default::default() — zero specification value."
```

### Task 6.3: Delete `"hosted"` string-absence assertions

**Files:**
- Modify: `crates/neo-agent/tests/cli_commands.rs` — delete `assert!(!stdout.contains("hosted"));` at ~line 659
- Modify: `crates/neo-agent/tests/rpc_mode.rs` — delete same assertion at ~line 645

- [ ] **Step 1: Delete in cli_commands.rs**

In `crates/neo-agent/tests/cli_commands.rs`, find line ~659 in function `sessions_export_json_returns_sanitized_replayed_session_artifact`. Delete:
```rust
assert!(!stdout.contains("hosted"));
```
Keep the `assert!(!stdout.contains("share_url"));` line above it — that's a meaningful assertion.

- [ ] **Step 2: Delete in rpc_mode.rs**

In `crates/neo-agent/tests/rpc_mode.rs`, find line ~645. Delete:
```rust
assert!(!stdout.contains("hosted"));
```

- [ ] **Step 3: Verify**

Run: `cargo check -p neo-agent --tests 2>&1 | head -20`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/neo-agent/tests/cli_commands.rs crates/neo-agent/tests/rpc_mode.rs
git commit -m "test(neo-agent): remove brittle 'hosted' string-absence assertions

Checking that the word 'hosted' doesn't appear in JSON output is neither
a specification nor a contract — it's a fragile English-word check."
```

### Task 6.4: Delete trivial `bash_default_timeout` test

**Files:**
- Modify: `crates/neo-agent-core/tests/tool_bash.rs` — delete function at ~line 5

- [ ] **Step 1: Delete the test function**

Delete:
```rust
#[test]
fn bash_default_timeout_allows_long_workspace_commands() {
    let workspace = tempfile::tempdir().expect("workspace");
    let context = ToolContext::new(workspace.path()).expect("context");
    assert_eq!(context.bash_timeout, DEFAULT_BASH_TIMEOUT);
    assert_eq!(context.bash_timeout, std::time::Duration::from_secs(10 * 60));
}
```

- [ ] **Step 2: Check if the file has other tests**

Read the rest of `crates/neo-agent-core/tests/tool_bash.rs`. If this was the only test, delete the entire file.

- [ ] **Step 3: Verify**

Run: `cargo check -p neo-agent-core --tests 2>&1 | head -20`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/neo-agent-core/tests/tool_bash.rs
git commit -m "test(neo-agent-core): delete trivial bash_default_timeout test

Asserts a const equals itself and the constructor default. No behavioral logic."
```

---

## Phase 7: Fix Shared-cwd Tests

### Task 7.1: Fix `tools/todo.rs` inline tests — use `tempfile::tempdir()`

**Files:**
- Modify: `crates/neo-agent-core/src/tools/todo.rs` — lines ~378, ~398, ~418, ~452, ~528, ~553

- [ ] **Step 1: Read the test module**

Read `crates/neo-agent-core/src/tools/todo.rs` from line ~370 to the end.

- [ ] **Step 2: Replace each `current_dir()` usage**

At each test function, replace:
```rust
let ctx = ToolContext::new(std::env::current_dir().unwrap()).unwrap();
```
With:
```rust
let dir = tempfile::tempdir().unwrap();
let ctx = ToolContext::new(dir.path()).unwrap();
```

Ensure `tempfile` is available as a dev-dependency (check `Cargo.toml`).

- [ ] **Step 3: Verify**

Run: `cargo nextest run -p neo-agent-core --lib todo`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/neo-agent-core/src/tools/todo.rs
git commit -m "test(neo-agent-core): fix shared-cwd anti-pattern in todo tests

Replace std::env::current_dir() with tempfile::tempdir() to comply
with AGENTS.md rule against shared-cwd test dependencies."
```

### Task 7.2: Fix `tools/background_tasks.rs` inline tests — use `tempfile::tempdir()`

**Files:**
- Modify: `crates/neo-agent-core/src/tools/background_tasks.rs` — lines ~1026, ~1048, ~1073, ~1099

- [ ] **Step 1: Replace each `current_dir()` usage**

Same pattern as Task 7.1 — replace all `std::env::current_dir()` with `tempfile::tempdir()`.

- [ ] **Step 2: Verify**

Run: `cargo nextest run -p neo-agent-core --lib background_tasks`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add crates/neo-agent-core/src/tools/background_tasks.rs
git commit -m "test(neo-agent-core): fix shared-cwd anti-pattern in background_tasks tests"
```

### Task 7.3: Fix remaining shared-cwd test occurrences

**Files:**
- `crates/neo-agent-core/src/tools/skills_manager.rs` — ~line 473
- `crates/neo-agent-core/src/tools/sessions.rs` — ~line 260
- `crates/neo-agent-core/src/tools/ask_user.rs` — ~line 325
- `crates/neo-agent-core/tests/runtime_turn.rs` — ~line 3462

- [ ] **Step 1: Fix each file**

At each cited line, replace `std::env::current_dir()` with `tempfile::tempdir()`. Use the same pattern:
```rust
let dir = tempfile::tempdir().unwrap();
// ... use dir.path() ...
```

- [ ] **Step 2: Verify**

Run: `cargo nextest run -p neo-agent-core --lib skills_manager`
Run: `cargo nextest run -p neo-agent-core --lib sessions`
Run: `cargo nextest run -p neo-agent-core --lib ask_user`
Expected: PASS for all.

- [ ] **Step 3: Commit**

```bash
git add crates/neo-agent-core/src/tools/ crates/neo-agent-core/tests/runtime_turn.rs
git commit -m "test(neo-agent-core): fix remaining shared-cwd test anti-patterns

skills_manager, sessions, ask_user inline tests, and runtime_turn integration
test now use tempfile::tempdir() instead of std::env::current_dir()."
```

---

## Phase 8: Remove Unused Test Helpers from `interactive.rs`

### Task 8.1: Mark test-only helpers as `#[cfg(test)]`

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive.rs`

This was partially done in Task 4.1. Verify completion and clean up any remaining `#[allow(dead_code)]` in the file.

- [ ] **Step 1: Grep for remaining `#[allow(dead_code)]` in production code**

Run: `grep -n '#\[allow(dead_code)\]' crates/neo-agent/src/modes/interactive.rs | head -20`

For each hit, determine if it's on a test-only helper (move to `#[cfg(test)]`), or truly dead (delete), or a legitimate fixture (keep with justification).

- [ ] **Step 2: Fix each remaining item**

Any `#[allow(dead_code)]` on production code that isn't a justified fixture should be removed. The underlying dead code should be deleted or gated behind `#[cfg(test)]`.

- [ ] **Step 3: Verify**

Run: `cargo check -p neo-agent 2>&1 | head -30`
Expected: PASS — no dead_code warnings.

Run: `grep -c '#\[allow(dead_code)\]' crates/neo-agent/src/modes/interactive.rs`
Expected: `0` (or only test-fixture items with clear justification).

- [ ] **Step 4: Commit**

```bash
git add crates/neo-agent/src/modes/interactive.rs
git commit -m "refactor(neo-agent): eliminate all #[allow(dead_code)] in interactive.rs

Test-only helpers moved to #[cfg(test)], truly dead methods deleted."
```

---

## Phase 9: Consolidate Triplicated Path Expansion Utilities

### Task 9.1: Unify `expand_user_path` into a single `pub(crate)` function

**Files:**
- Modify: `crates/neo-agent/src/config.rs` — make existing functions `pub(crate)` (they already exist at ~line 946-967)
- Modify: `crates/neo-agent/src/resources.rs` — delete local copies (~line 222-239), import from `config`
- Modify: `crates/neo-agent/src/themes.rs` — delete local copies (~line 139-160), import from `config`

- [ ] **Step 1: Make config.rs functions `pub(crate)`**

In `crates/neo-agent/src/config.rs`, change:
```rust
fn expand_user_path(path: PathBuf) -> PathBuf {
```
To:
```rust
pub(crate) fn expand_user_path(path: PathBuf) -> PathBuf {
```

Also make `expand_user_path_with_home` and `user_home` `pub(crate)`.

- [ ] **Step 2: Delete duplicated functions in resources.rs**

In `crates/neo-agent/src/resources.rs`, delete the private `expand_user_path`, `expand_user_path_with_home`, and `user_home` functions (~lines 222-239).

Add import at top of file:
```rust
use crate::config::{expand_user_path, user_home};
```
(Only import what's actually used in this file. Check with grep.)

- [ ] **Step 3: Delete duplicated functions in themes.rs**

In `crates/neo-agent/src/themes.rs`, delete the private `expand_user_path`, `expand_user_path_with_home`, and `user_home` functions (~lines 139-160).

Add import:
```rust
use crate::config::{expand_user_path, user_home};
```

- [ ] **Step 4: Verify**

Run: `cargo check -p neo-agent 2>&1 | head -30`
Expected: PASS

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/neo-agent/src/config.rs crates/neo-agent/src/resources.rs crates/neo-agent/src/themes.rs
git commit -m "refactor(neo-agent): deduplicate path expansion utilities into config.rs

Three identical copies of expand_user_path/expand_user_path_with_home/
user_home existed in config.rs, resources.rs, and themes.rs.
Consolidated into pub(crate) functions in config.rs."
```

---

## Phase 10: Consolidate `message_text` into `AgentMessage` method

### Task 10.1: Add `text()` method on `AgentMessage` and delete 4 copies

**Files:**
- Modify: `crates/neo-agent-core/src/messages.rs` — add `pub fn text(&self) -> String` method on `AgentMessage`
- Modify: `crates/neo-agent/src/modes/sessions.rs` — delete local `message_text` (~line 332), use method
- Modify: `crates/neo-agent/src/modes/run.rs` — delete local `message_text` (~line 2325), use method
- Modify: `crates/neo-agent-core/src/sidecar.rs` — delete local `message_text` (~line 50), use method
- Modify: `crates/neo-tui/src/chrome.rs` — delete local `message_text` (~line 428), use method

- [ ] **Step 1: Add `text()` method on `AgentMessage`**

In `crates/neo-agent-core/src/messages.rs`, find `impl AgentMessage`. Add:
```rust
/// Extract and concatenate all text content from this message.
pub fn text(&self) -> String {
    let parts = match self {
        Self::User { content, .. } | Self::Assistant { content, .. } => content,
        _ => return String::new(),
    };
    parts
        .iter()
        .filter_map(|c| match c {
            Content::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}
```

Check the actual variant names and `Content` variants by reading the existing `message_text` implementations — match the exact structure.

- [ ] **Step 2: Replace callers in sessions.rs**

In `crates/neo-agent/src/modes/sessions.rs`, replace all calls to `message_text(&msg)` with `msg.text()`. Delete the local `message_text` function.

- [ ] **Step 3: Replace callers in run.rs**

Same — replace `message_text(&msg)` with `msg.text()`. Delete local function.

- [ ] **Step 4: Replace callers in sidecar.rs**

Same. This is in `#[cfg(test)]` — replace and delete.

- [ ] **Step 5: Replace callers in chrome.rs**

Same — replace `message_text(message)` with `message.text()`. Delete local function.

- [ ] **Step 6: Verify**

Run: `cargo check --workspace 2>&1 | head -30`
Expected: PASS

Run: `cargo nextest run -p neo-agent-core --lib`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add crates/neo-agent-core/src/messages.rs crates/neo-agent/src/modes/ crates/neo-agent-core/src/sidecar.rs crates/neo-tui/src/chrome.rs
git commit -m "refactor: consolidate 4 message_text copies into AgentMessage::text()

Added pub fn text() on AgentMessage; deleted duplicated free functions
in sessions.rs, run.rs, sidecar.rs, and chrome.rs."
```

---

## Phase 11: Consolidate `validate_mcp_server` duplication

### Task 11.1: Unify MCP server validation into `mcp_ops::validate_mcp_server_config`

**Files:**
- Modify: `crates/neo-agent/src/config.rs` — delete `validate_mcp_server` (~line 676), import from `mcp_ops`
- Verify: `crates/neo-agent/src/mcp_ops.rs:114-160` — this is the stricter, canonical version

- [ ] **Step 1: Read both functions**

Read `crates/neo-agent/src/config.rs:676-710` and `crates/neo-agent/src/mcp_ops.rs:114-160`. Confirm that `mcp_ops::validate_mcp_server_config` is the stricter version (it adds mutual-exclusion validation).

- [ ] **Step 2: Delete `validate_mcp_server` from config.rs**

In `crates/neo-agent/src/config.rs`, delete the `validate_mcp_server` function (~line 676-710).

- [ ] **Step 3: Update callers**

Search for calls to `validate_mcp_server(` in config.rs and surrounding code. Replace with `crate::mcp_ops::validate_mcp_server_config(`.

- [ ] **Step 4: Verify**

Run: `cargo check -p neo-agent 2>&1 | head -30`
Expected: PASS

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/neo-agent/src/config.rs
git commit -m "refactor(neo-agent): consolidate validate_mcp_server into mcp_ops

Deleted weaker config.rs copy; all callers now use the stricter
mcp_ops::validate_mcp_server_config which adds mutual-exclusion validation."
```

---

## Phase 12: Consolidate `parse_mcp_kind` / `display_mcp_kind` / `parse_command_string`

### Task 12.1: Delete duplicated functions in `run.rs`, import from `mcp_ops`

**Files:**
- Modify: `crates/neo-agent/src/modes/run.rs` — delete private copies (~lines 775-800), import from `mcp_ops`
- Verify: `crates/neo-agent/src/mcp_ops.rs:66-94` — canonical public versions

- [ ] **Step 1: Read both sets**

Read `crates/neo-agent/src/mcp_ops.rs:66-94` and `crates/neo-agent/src/modes/run.rs:775-800`. Note the difference: mcp_ops accepts `"stdio"`/`"http"`/`"sse"` as aliases, run.rs only accepts CLI forms (`"studio"`, `"remote-http"`, `"remote-sse"`). The mcp_ops version is a superset.

- [ ] **Step 2: Delete private copies in run.rs**

Delete the three private functions in `crates/neo-agent/src/modes/run.rs` (~lines 775-800).

- [ ] **Step 3: Add import**

At the top of `run.rs`, add or extend the import:
```rust
use crate::mcp_ops::{parse_mcp_kind, display_mcp_kind, parse_command_string};
```

- [ ] **Step 4: Verify alias compatibility**

If any caller in run.rs passes `"studio"` / `"remote-http"` / `"remote-sse"` to `parse_mcp_kind`, verify these are accepted by the mcp_ops version. If not, add them as aliases in mcp_ops before proceeding. Read the mcp_ops function arms to confirm.

- [ ] **Step 5: Verify**

Run: `cargo check -p neo-agent 2>&1 | head -30`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/neo-agent/src/modes/run.rs crates/neo-agent/src/mcp_ops.rs
git commit -m "refactor(neo-agent): deduplicate parse_mcp_kind/display_mcp_kind/parse_command_string

Deleted private copies in run.rs; all callers now use the canonical
pub versions from mcp_ops."
```

---

## Phase 13: Introduce `McpTransport` Enum (stringly-typed fix)

### Task 13.1: Create `McpTransport` enum and replace `transport: String`

**Files:**
- Create: `crates/neo-agent/src/config.rs` — add `McpTransport` enum (or in a shared location)
- Modify: `crates/neo-agent/src/config.rs` — change `McpServerConfig.transport` from `String` to `McpTransport`
- Modify: all callers that match on `transport.as_str()` — `mcp_ops.rs`, `run.rs`, `config.rs`, `chrome.rs`/`dialogs/`

> **Note:** This is the highest-risk consolidation task. The serde representation must remain backward-compatible (`"stdio"`, `"http"`, `"sse"` in TOML). Use `#[serde(rename_all = "lowercase")]`.

- [ ] **Step 1: Define the enum**

In `crates/neo-agent/src/config.rs`, near the `McpServerConfig` definition:
```rust
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    Stdio,
    Http,
    Sse,
}

impl McpTransport {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
            Self::Sse => "sse",
        }
    }
}
```

- [ ] **Step 2: Change the field type**

In `McpServerConfig`, change:
```rust
pub transport: String,
```
To:
```rust
pub transport: McpTransport,
```

- [ ] **Step 3: Fix all construction sites**

Search for `transport:` in struct literals building `McpServerConfig`. Change `"stdio".to_string()` → `McpTransport::Stdio`, etc.

- [ ] **Step 4: Fix match sites**

Replace `match server.transport.as_str() { "stdio" => … }` with `match server.transport { McpTransport::Stdio => … }` at all locations. Key sites:
- `config.rs:685` — validation
- `mcp_ops.rs:120, 164, 177, 189, 216-236` — conversion/probe
- `run.rs:868, 881, 893, 896, 940, 1903-1925` — CLI paths
- Any dialog/form code that reads transport

- [ ] **Step 5: Fix form/dialog code**

In `crates/neo-tui/src/dialogs/mcp_add_form.rs` and `mcp_manager.rs`, update any string-to-transport conversions to use the enum.

- [ ] **Step 6: Verify TOML backward compatibility**

Check that existing config files with `transport = "stdio"` still parse. The `#[serde(rename_all = "lowercase")]` should handle this.

Run: `cargo check --workspace 2>&1 | head -50`
Expected: PASS

- [ ] **Step 7: Run tests**

Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add crates/neo-agent/src/ crates/neo-tui/src/
git commit -m "refactor: introduce McpTransport enum, replace stringly-typed transport field

McpServerConfig.transport is now McpTransport::Stdio|Http|Sse instead of
String. Eliminates 86+ raw string comparisons ('stdio'/'http'/'sse') across
config.rs, mcp_ops.rs, run.rs, and dialog code. Serde backward-compatible
via #[serde(rename_all = \"lowercase\")]."
```

---

## Phase 14: Remove Single-Implementor Trait Abstractions

### Task 14.1: Remove `DynamicInjector` trait + `InjectionManager` indirection

**Files:**
- Modify: `crates/neo-agent-core/src/injection/mod.rs`
- Modify: `crates/neo-agent-core/src/injection/injector.rs` — delete trait
- Modify: `crates/neo-agent-core/src/injection/manager.rs` — simplify or delete
- Modify: `crates/neo-agent-core/src/injection/plan_mode.rs` — inline the logic

> **Note:** This task requires understanding the injection flow. Read all injection files first.

- [ ] **Step 1: Read the injection module**

Read all files in `crates/neo-agent-core/src/injection/`:
- `mod.rs` — module exports
- `injector.rs` — `DynamicInjector` trait
- `manager.rs` — `InjectionManager` that wraps a list of injectors
- `plan_mode.rs` — `PlanModeInjector` (the only implementor)

- [ ] **Step 2: Understand the call flow**

Grep for `InjectionManager` and `DynamicInjector` usage across the workspace. Understand how `inject()` is called and what it does.

- [ ] **Step 3: Replace trait with concrete type**

If the injection flow is `manager.inject(context)` → calls `PlanModeInjector::inject()`, replace the manager+trait with a direct `PlanModeInjector` struct and a concrete `inject()` method. Delete `DynamicInjector` trait and `InjectionManager`.

If the manager has meaningful orchestration logic (batch injection, ordering), keep a simplified version that directly calls `PlanModeInjector` without the trait indirection.

- [ ] **Step 4: Fix all references**

Update all code that imports or references `DynamicInjector`, `InjectionManager` to use the concrete types.

- [ ] **Step 5: Verify**

Run: `cargo check --workspace 2>&1 | head -30`
Expected: PASS

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/neo-agent-core/src/injection/
git commit -m "refactor(neo-agent-core): remove single-implementor DynamicInjector trait

InjectionManager + DynamicInjector abstraction had exactly one implementor
(PlanModeInjector). Replaced with direct concrete type — eliminates
indirection without changing behavior."
```

---

## Post-Cleanup Verification

### Final Task: Full workspace verification

- [ ] **Step 1: Full workspace check**

Run: `cargo check --workspace --all-features 2>&1 | head -50`
Expected: PASS — zero errors, zero warnings.

- [ ] **Step 2: Clippy clean**

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings 2>&1 | head -50`
Expected: PASS — zero warnings.

- [ ] **Step 3: Fmt check**

Run: `cargo fmt --all --check 2>&1 | head -20`
Expected: PASS

- [ ] **Step 4: Dead code audit — grep for remaining `#[allow(dead_code)]`**

Run: `grep -rn '#\[allow(dead_code)\]' crates/ --include='*.rs' | grep -v '#\[cfg(test)\]' | grep -v '/tests/'`
Expected: Zero hits in production code (non-test).

- [ ] **Step 5: Focused test run**

Run: `cargo nextest run --workspace --all-features`
Expected: PASS — all tests green.

- [ ] **Step 6: Final commit**

```bash
git add -A
git commit -m "chore: final cleanup verification — zero dead code, zero allow(dead_code)

Full workspace check, clippy, fmt, and test suite pass after codebase cleanup."
```

---

## Self-Review Checklist

After completing all tasks, verify:

- [ ] No `#[allow(dead_code)]` in production code under `crates/*/src/`
- [ ] No `#[ignore]` tests
- [ ] No `std::env::current_dir()` in inline tests
- [ ] No `assert!(!stdout.contains("hosted"))` type assertions
- [ ] No triplicated `expand_user_path` functions
- [ ] No quadruplicated `message_text` free functions
- [ ] No duplicated `validate_mcp_server` / `parse_mcp_kind`
- [ ] `McpServerConfig.transport` is an enum, not String
- [ ] No single-implementor traits (`DynamicInjector`)
- [ ] `stdio.rs` has no stale TODO comments
- [ ] `AgentMode` enum deleted
- [ ] `PreparedToolCall` has no dead fields
- [ ] All phases compile and tests pass
