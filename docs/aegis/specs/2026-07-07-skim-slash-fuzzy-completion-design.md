# Skim Slash Fuzzy Completion Design

## Goal

Upgrade Neo's slash command completion so users get command-palette quality fuzzy matches while preserving Neo's existing slash command sources and stable ordering. The implementation will use `skim` as the matching engine and raise Neo's documented minimum Rust version to Rust 1.96.1.

## Background

Neo currently builds slash completion candidates in `crates/neo-agent/src/modes/interactive/prompt_completion.rs`. Slash matching is prefix-only: candidates are retained with `item.value.starts_with(prefix)`, then sorted by completion source and value. This is predictable, but it misses common command-palette inputs such as `/mdl` for `/model`, `/prv` for `/provider`, and skill-name queries that omit the `/skill:` prefix.

Kimi's reference implementation points in two useful directions: keep candidate collection separate from matching/ranking, and use fuzzy ranking for picker-like experiences. For Neo, `skim` is the preferred third-party engine because it is actively maintained, MIT licensed, and exposes library APIs such as `skim::fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2}`. `skim` 5.0.0 requires Rust 1.91, and the project will intentionally move all documentation and workspace metadata to Rust 1.96.1.

## Scope

In scope:

- Add `skim = { version = "5.0.0", default-features = false }`.
- Set the workspace Rust version to 1.96.1.
- Update user-facing and agent-facing Rust version documentation.
- Replace slash command prefix-only filtering with a Neo-local ranking adapter backed by `SkimMatcherV2`.
- Apply fuzzy slash matching to built-in commands, slash prompt templates, prompt package commands, and `/skill:<name>` session skill commands.
- Add focused tests for fuzzy slash ranking behavior.

Out of scope:

- File path completion fuzzy matching.
- Provider model completion fuzzy matching.
- Session picker, model picker, help panel, or other searchable list behavior.
- Rendering matched character highlights in the candidate list.
- Using skim's full interactive finder UI.

## Architecture

The implementation will keep Neo's existing completion pipeline. `completion_source_candidates` will still decide whether the active prefix is slash, model, or filesystem completion. Only the slash branch changes.

`slash_source_candidates` will collect slash candidates from `CompletionCatalog` without prefix filtering. Each candidate will be scored by a new local adapter that computes:

- The original candidate.
- The completion source rank.
- The candidate's original collection order.
- The best slash match tier.
- The best skim score for fuzzy matches.

The adapter will use `SkimMatcherV2::default().smart_case()` and the `FuzzyMatcher` trait's `fuzzy_match` method. `skim` is only used to compute fuzzy scores; Neo retains its own match tiers and tie-breakers so exact and prefix matches stay intuitive.

## Matching Model

The user query is normalized only for slash matching:

- Strip one leading `/`.
- Trim surrounding whitespace.
- Treat an empty query as "show all slash candidates".

Each candidate produces one or more search keys:

- `/model` produces `model`.
- `/permissions` produces `permissions`.
- `/skill:code-simplifier` produces both `skill:code-simplifier` and `code-simplifier`.
- `/review-code` produces `review-code`.

The best match across all keys is used for ranking. Match tiers are:

1. `Exact`: query exactly equals a key.
2. `Prefix`: a key starts with the query.
3. `SegmentPrefix`: any segment after `:`, `-`, `_`, `.`, or `/` starts with the query.
4. `Fuzzy`: `SkimMatcherV2` returns a score for a key/query pair.

Description text is not searched in the first version. Keeping the match surface limited to command names avoids surprising results where a command appears only because of prose in its description.

## Ordering

Slash results are sorted by:

1. Match tier: `Exact`, `Prefix`, `SegmentPrefix`, then `Fuzzy`.
2. Skim score descending.
3. Completion source rank, preserving Neo's existing source priorities.
4. Original collection order, so built-in commands keep their curated order.
5. Candidate value as a final stable tie-breaker.

For an empty slash query, no fuzzy scoring is needed. The existing source ordering and collection order should be preserved.

## Expected Behavior

- `/m` shows `/model` and `/mcp` near the top with stable command-list ordering.
- `/mdl` matches `/model`.
- `/prv` matches `/provider`.
- `/perm` matches `/permissions`.
- `/code` can match `/skill:code-simplifier` without requiring `/skill:`.
- `/rvw` can match `/review` prompt templates.
- `/zzzznotacommand` returns no slash candidates.

## Rust Version And Docs

The implementation will update:

- `Cargo.toml` workspace `rust-version` to `1.96.1`.
- `AGENTS.md` project summary to min Rust 1.96.1.
- `README.md` prerequisites to Rust 1.96.1+.
- `README.zh-CN.md` prerequisites to Rust 1.96.1+.
- `docs/en/quickstart.md` Rust prerequisite table to 1.96.1+.
- `docs/zh/quickstart.md` Rust prerequisite table to 1.96.1+.

`rust-toolchain.toml` currently pins `stable`, and stable has been updated locally to `rustc 1.96.1 (31fca3adb 2026-06-26)`. This design keeps `channel = "stable"` unless implementation discovers the project requires an exact `1.96.1` channel pin for reproducibility.

`skim` is intentionally used as the maintained upstream fuzzy matcher even though `default-features = false` still brings some finder/runtime dependencies into the lockfile. This cost is accepted for the slash completion upgrade so Neo can use the current `SkimMatcherV2` implementation instead of the archived `fuzzy-matcher` crate or a local hand-rolled scorer.

## Error Handling

Slash completion remains synchronous and fallible only at the existing catalog-building boundaries. A candidate with no match is filtered out. `skim` scoring failures are represented by `None` and do not produce user-visible errors.

Filesystem and model completion paths keep their current error behavior.

## Testing

Focused tests will be added to `crates/neo-agent/src/modes/interactive/tests.rs` around `completion_source_candidates` and `CompletionCatalog`.

Required cases:

- Empty slash query preserves slash candidate availability and stable ordering.
- Prefix matches beat fuzzy matches.
- `/mdl` returns `/model`.
- `/prv` returns `/provider`.
- `/code` returns `/skill:code-simplifier`.
- `/rvw` returns a `/review` prompt candidate.
- A nonsense query returns an empty slash result set.

Verification should use narrow commands only, following this repo's testing policy:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::<exact_test_name> --exact --nocapture --include-ignored
```

For dependency and MSRV/doc edits, run the narrowest useful build/check after the focused tests:

```bash
cargo check -p neo-agent --bin neo
```

## Non-Goals

This feature is not a general fuzzy search framework. It introduces a narrow adapter for slash command completion and avoids compatibility branches or duplicate old/new completion paths.

## Self-Review

- Placeholder scan: no placeholders remain.
- Consistency check: the scope, matching model, ordering, tests, and Rust version requirements all align on `skim` plus Neo-local ranking.
- Scope check: the spec is focused on slash completion and MSRV/documentation updates; other completion systems are explicitly out of scope.
- Ambiguity check: description matching is explicitly excluded in the first version, and `rust-toolchain.toml` behavior is explicit.
