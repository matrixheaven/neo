# Provider Model Refresh Implementation Plan

> **For agentic workers:** Execute task-by-task. Do not redesign the approved
> behavior or broaden refresh beyond `models.dev`.

**Goal:** Add `R` refresh to `/provider`, fetch the current selected provider
from `models.dev`, and atomically replace only that provider's configured models.

**Architecture:** Reuse the provider manager, the existing background catalog
fetch task, catalog conversion, and atomic config writer. The TUI emits one
provider id; the interactive controller fetches and routes the result; the
config mutation performs the complete replacement and default transition.

**Tech Stack:** Rust 2024, `neo-tui`, `neo-agent`, `neo-ai`, Tokio, TOML config.

**Baseline/Authority Refs:**

- `AGENTS.md`
- `docs/aegis/specs/2026-08-01-provider-model-refresh-brief.md`
- `crates/neo-tui/src/dialogs/provider_manager.rs`
- `crates/neo-agent/src/modes/interactive/catalog_fetch.rs`
- `crates/neo-agent/src/config/mutations.rs`

**Compatibility Boundary:** Preserve provider settings, unrelated providers and
models, existing add/delete behavior, config atomicity, and existing catalog
import behavior. Remove no public command and add no config field or fallback.

**TDD Route:**

- Mode: off
- Decision: skipped
- Strict authority: not applicable
- Test posture: post-change regression
- Reason: The approved feature is a small extension of three existing owners.
- Verification: Exact unit and controller tests for each changed boundary.

**Verification:** Run only the exact tests listed per task, then scoped formatting
and `git diff --check` over touched files.

## Scope Check

**Aegis Visibility:** Planning prevents refresh behavior from leaking into a
second network path or a second config writer.

**Plan Basis:** The approved spec fixes the source, replacement semantics,
default fallback, failure safety, UI behavior, and non-goals.

**BaselineUsageDraft:**

- Required baseline refs: approved spec and `AGENTS.md`
- Delivered context refs: current provider manager, catalog task, and config mutation owners
- Acknowledged before plan refs: all required refs
- Cited in plan refs: all required refs
- Missing refs: none
- Decision: continue

**Requirement Ready Check:**

- Requirement source refs: approved spec
- Goals and scope refs: approved spec Goal and Approved Behavior
- User / scenario refs: `/provider`, selected provider, `R`
- Requirement item refs: config replacement and failure safety sections
- Acceptance / verification criteria refs: approved spec Acceptance
- Open blocker questions: none
- Decision: ready

**Change Necessity:**

- User-visible need: refresh configured models without re-adding a provider
- No-change / non-code option: manually delete and re-import the provider
- Why code change is necessary: the current provider manager has no refresh action
- Minimum change boundary: provider dialog, catalog completion routing, config mutation
- Decision: code-change

**Existence Check:**

- Proposed new surface: exported automatic reasoning selection helper
- Existing owner / reuse candidate: private `auto_selection` in `neo-ai`
- Why existing surface is insufficient: `neo-agent` cannot reuse a private cross-crate function
- Creation proof: fallback must use Neo's existing automatic rule without copying it
- Entropy / retirement impact: rename and export existing logic; add no second algorithm
- Decision: add-with-proof

**Architecture Integrity Lens:** The config mutation remains the only writer;
catalog fetching remains in the existing interactive task; the provider dialog
only emits intent. No responsibility overlap or higher-level simplification is
available.

**Plan Pressure Test:**

- Owner / retirement: existing owners, no old path to retain
- Architecture integrity / higher-level path: one fetch path and one config writer
- Verification scope: one exact test per boundary, with two config branch tests
- Task executability: exact files, symbols, and commands are named below
- Pressure result: proceed

**Plan-Time Complexity Check:**

- Target files: existing dialog, catalog controller, config mutation, focused tests
- Existing size / shape signals: `mutations.rs` and interactive tests are large but already own these behaviors
- Owner fit: direct
- Add-in-place risk: low when helpers remain narrow
- Better file boundary: no new file; reuse `catalog_fetch.rs`
- Recommendation: edit-in-place

## Files

- Modify `crates/neo-ai/src/reasoning.rs`
  - Expose the existing automatic reasoning selection function without changing behavior.
- Modify `crates/neo-ai/src/lib.rs`
  - Re-export that function for the config mutation owner.
- Modify `crates/neo-agent/src/config/mutations.rs`
  - Add atomic selected-provider model refresh and focused tests.
- Modify `crates/neo-tui/src/dialogs/provider_manager.rs`
  - Add the refresh action, key handling, hint, consuming action access, and unit test.
- Modify `crates/neo-tui/src/shell/input_dispatch.rs`
  - Consume provider-manager actions so refresh can leave the dialog open.
- Modify `crates/neo-agent/src/modes/interactive/dialog_results.rs`
  - Route the consumed refresh action without closing the dialog.
- Modify `crates/neo-agent/src/modes/interactive/catalog_fetch.rs`
  - Add refresh completion to the existing pending fetch operation.
- Modify `crates/neo-agent/src/modes/interactive/tests.rs`
  - Cover controller success, overlap rejection, reload, and open-dialog behavior.

## Task 1: Atomic Config Replacement

**Files:**

- Modify: `crates/neo-ai/src/reasoning.rs`
- Modify: `crates/neo-ai/src/lib.rs`
- Modify: `crates/neo-agent/src/config/mutations.rs`

**Why:** A successful fetch must become one safe config transition, including a
usable fallback when the current default model disappears.

**Change Necessity:** The catalog import helper replaces `ProviderConfig` and
credentials, so it cannot satisfy the approved preservation boundary. Add one
model-only mutation beside the existing config mutations.

**Impact/Compatibility:** Keep `add_provider_from_catalog_entry` unchanged.
Reuse its model alias and conversion rules. No config schema change.

- [ ] Rename private `auto_selection` to public
  `automatic_reasoning_selection` in `neo-ai/src/reasoning.rs`, update
  `ReasoningPolicy::Auto`, and re-export it from `neo-ai/src/lib.rs`. Do not
  change the matching logic or defaults.

- [ ] Add this production signature near `add_provider_from_catalog_entry`:

```rust
pub fn refresh_provider_models_from_catalog_entry(
    config_path: &Path,
    provider_id: &str,
    entry: &neo_ai::catalog::CatalogEntry,
) -> anyhow::Result<String>
```

The function must convert the entry before calling `update_file_config`, reject
an empty converted model list, then inside the closure:

1. Resolve whether `default_model` currently refers to a configured model owned
   by `provider_id`, retaining that model's underlying `model` id.
2. Remove only models owned by `provider_id` through `remove_provider_models`.
3. Insert every converted catalog model through the existing
   `catalog_model_alias` and `catalog_model_config` helpers.
4. If the selected provider owns the default, find the same underlying model id
   in the refreshed list; otherwise choose the refreshed list's first model.
5. Write the chosen canonical alias to `default_model` and synchronize
   `default_provider`.
6. Only on fallback, set `runtime.reasoning` from
   `neo_ai::automatic_reasoning_selection(&chosen.reasoning)`.
7. Return `refreshed provider '<id>' with <count> model(s)\n`.

- [ ] Add
  `refresh_provider_models_preserves_provider_and_surviving_default` to prove
  provider fields and unrelated config are byte-equivalent after parsing, old
  models are gone, refreshed models are present, and the same underlying default
  model maps to its canonical alias without changing reasoning.

- [ ] Add
  `refresh_provider_models_falls_back_with_automatic_reasoning` to prove a
  removed default switches to the first refreshed model, synchronizes the
  provider, and derives reasoning from the selected model capability.

- [ ] Run both exact regressions:

```bash
cargo test --package neo-agent --bin neo -- config::mutations::tests::refresh_provider_models_preserves_provider_and_surviving_default --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- config::mutations::tests::refresh_provider_models_falls_back_with_automatic_reasoning --exact --nocapture --include-ignored
```

Expected: each command runs one test and passes.

- [ ] Commit only Task 1:

```bash
git add crates/neo-ai/src/reasoning.rs crates/neo-ai/src/lib.rs crates/neo-agent/src/config/mutations.rs
git commit -m "feat: refresh provider model config"
```

## Task 2: Provider Manager Refresh Action

**Files:**

- Modify: `crates/neo-tui/src/dialogs/provider_manager.rs`
- Modify: `crates/neo-tui/src/shell/input_dispatch.rs`
- Modify: `crates/neo-agent/src/modes/interactive/dialog_results.rs`

**Why:** The selected provider must produce one refresh request while the dialog
stays visible.

**Change Necessity:** Current provider actions all close the dialog and action
reads are non-consuming. Refresh needs a consuming action so the same request is
not processed again on every terminal-loop tick.

**Impact/Compatibility:** Existing Add, DeleteSource, and Close actions keep
their behavior. Only the internal action read becomes consuming.

- [ ] Add the exact action variant:

```rust
Refresh(String),
```

- [ ] Change the header hint to include `R refresh`. Add a private
  `refresh_selected_provider` that only accepts a `Row::Source` containing one
  provider id, sets `ProviderManagerAction::Refresh(id.clone())`, and returns
  `InputResult::Submitted`. The add row returns `InputResult::Handled` without
  an action. Delete confirmation keeps exclusive input handling.

- [ ] Route both `r` and `R` from `handle_insert` to that helper. Keep `d` and
  `D` unchanged.

- [ ] Replace the cloned `action()` read with:

```rust
pub fn take_action(&mut self) -> Option<ProviderManagerAction> {
    self.action.take()
}
```

Update the shell accessor and controller result processing to consume the action
once. Pass the consumed value into `handle_provider_manager_action`. Close the
overlay for Add, DeleteSource, and Close; for Refresh, leave it open and call the
Task 3 refresh starter.

- [ ] Add one table-style unit test named
  `r_refreshes_selected_provider_but_not_add_or_confirmation` covering lowercase,
  uppercase, add row, and armed delete confirmation. Extend `render_shows_hint`
  to require `R refresh`.

- [ ] Run the exact dialog regression:

```bash
cargo test --package neo-tui --lib -- dialogs::provider_manager::tests::r_refreshes_selected_provider_but_not_add_or_confirmation --exact --nocapture --include-ignored
```

Expected: one test runs and passes.

- [ ] Commit only Task 2:

```bash
git add crates/neo-tui/src/dialogs/provider_manager.rs crates/neo-tui/src/shell/input_dispatch.rs crates/neo-agent/src/modes/interactive/dialog_results.rs
git commit -m "feat(tui): request provider refresh"
```

## Task 3: Reuse Background Catalog Fetch

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive/catalog_fetch.rs`
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`

**Why:** Network work must remain asynchronous, visible, single-owner, and safe
on failure.

**Change Necessity:** The existing pending fetch only distinguishes catalog
browsing from provider addition. It needs one refresh completion outcome rather
than a second task and poll loop.

**Impact/Compatibility:** Known catalog browsing, custom registry browsing, and
provider addition continue through the same fetch and poll path.

- [ ] Replace `pending_add: Option<PendingCatalogAdd>` with one completion enum:

```rust
pub(super) enum CatalogFetchCompletion {
    Browse,
    Add(PendingCatalogAdd),
    Refresh(PendingCatalogRefresh),
}

pub(super) struct PendingCatalogRefresh {
    pub(super) provider_id: String,
    pub(super) config_path: PathBuf,
}
```

Every existing fetch constructor must select either `Browse` or `Add`; do not
retain `pending_add` as a compatibility field.

- [ ] Add `start_provider_model_refresh(&mut self, provider_id: String)`:

1. If `pending_catalog_fetch.is_some()`, report `Provider refresh is already running`
   and keep the existing handle untouched.
2. Resolve `config_path`; on absence report `No config available`.
3. Set footer text to `Refreshing provider <id>...`.
4. Spawn the existing `neo_ai::catalog::fetch_catalog()` call.
5. Store `CatalogFetchCompletion::Refresh` through the existing pending owner.

- [ ] In `poll_pending_catalog_fetch`, match completion after a successful fetch.
For Refresh, look up the exact provider id and call
`refresh_provider_models_from_catalog_entry`. A missing id reports an error and
does not write. On mutation success, push the returned message, call
`refresh_config`, and leave the provider overlay open. Existing network and join
errors retain their current reporting and clear the footer.

- [ ] Add
  `provider_refresh_completion_reloads_config_and_keeps_dialog_open`. Build a
  deterministic finished Tokio handle returning an in-memory catalog, open the
  provider manager, poll completion, and assert the on-disk replacement, loaded
  model catalog, success status, cleared footer, and unchanged overlay kind.

- [ ] In the same test, install a still-pending handle, invoke the refresh
  starter again, and assert the original pending operation remains owned and the
  overlap status is visible. Do not make a live network request.

- [ ] Run the exact controller regression:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::provider_refresh_completion_reloads_config_and_keeps_dialog_open --exact --nocapture --include-ignored
```

Expected: one test runs and passes without network access.

- [ ] Run scoped final checks:

```bash
rustfmt --check --edition 2024 crates/neo-ai/src/reasoning.rs crates/neo-ai/src/lib.rs crates/neo-agent/src/config/mutations.rs crates/neo-tui/src/dialogs/provider_manager.rs crates/neo-tui/src/shell/input_dispatch.rs crates/neo-agent/src/modes/interactive/dialog_results.rs crates/neo-agent/src/modes/interactive/catalog_fetch.rs crates/neo-agent/src/modes/interactive/tests.rs
git diff --check
```

Expected: both commands succeed. This is focused local proof, not remote CI or
native Windows/Linux execution.

- [ ] Commit only Task 3:

```bash
git add crates/neo-agent/src/modes/interactive/catalog_fetch.rs crates/neo-agent/src/modes/interactive/tests.rs
git commit -m "feat: refresh provider models in background"
```

## Risks

- Full replacement intentionally deletes hand-written models owned by the
  refreshed provider. The UI does not add a confirmation because the approved
  behavior is an explicit refresh command.
- Catalog ordering determines the fallback model. The existing catalog
  conversion order remains authoritative; no ranking heuristic is added.
- A provider not found in `models.dev` cannot refresh and remains unchanged.

## Retirement

- No old refresh path exists.
- `pending_add` is replaced, not retained beside the completion enum.
- No source metadata, custom endpoint fallback, retry, cache, or merge path is
  introduced.

## Execution Readiness View

- Intent Lock: selected provider, manual `R`, `models.dev`, full model replacement
- Scope Fence: no endpoint refresh, source persistence, merge, retry, or periodic refresh
- Baseline Lock: approved spec and current repository rules
- Approved Behavior: exact Approved Behavior and Acceptance sections in the spec
- Owner Constraints: TUI emits; controller fetches; config mutation writes
- Compatibility Boundary: provider settings and unrelated config remain unchanged
- Retirement Boundary: replace `pending_add`; retain no parallel completion path
- Task Batches: config mutation, TUI action, background completion
- Test Obligations: exact tests listed in each task
- Review Gates: task-scoped diff and focused verification before each commit
- Drift / Rewind Rules: stop if source provenance or provider endpoint support becomes necessary
- Evidence Required Before Completion: three exact regressions, scoped rustfmt, diff check
- Advisory Boundary: execution guidance only; it does not establish completion
