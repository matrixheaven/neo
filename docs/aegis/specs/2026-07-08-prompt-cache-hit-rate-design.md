# Neo Prompt Cache Hit Rate Design

## Background

Two cache audits and live API-console observations point to different layers of the same problem.

The live DeepSeek Anthropic-compatible run is the most important signal: cache eventually works well. The dashboard pattern starts with roughly 1M uncached input tokens, then cache reads accelerate from about `2M hit / 1M miss` toward `10M hit / 1M miss`, and eventually stabilizes near 95% hit rate. A 2026-06-21 run showed a much smaller uncached slope: `7,241,344 hit / 253,660 miss` to `15,841,152 hit / 267,238 miss`, then later `23,980,928 hit / 777,214 miss` after a new session. This means Neo is probably not in a total cache-miss state. The optimization target is to reduce warmup/miss slope, make Neo's local usage reporting trustworthy, and prevent runtime changes from breaking stable prefixes.

The current code still has sharp edges:

- Anthropic stream parsing ignores `message_start.message.usage`, so local cache usage can be missing even when the provider dashboard reports hits.
- Anthropic request construction marks system, tools, and only the last message body with `cache_control`; this underuses the message-side anchor budget during tool loops.
- Runtime reminder and projection behavior can append or rewrite request-time messages; these should not be mistaken for durable user prompts when choosing cache anchors.
- TUI shows raw cache read/write tokens but not a hit-rate signal.
- Google context caching and full compaction tuning require separate design; they are not part of this first repair.

## Goals

- Parse Anthropic-compatible cache usage from real SSE shapes so Neo local usage aligns with API-console data.
- Use Anthropic's limited cache-control budget more deliberately: system, tools, latest real user message, and request tail.
- Keep request prefixes byte-stable across turns except for intentional appends or explicit projection.
- Make cache behavior visible in the TUI footer with a conservative hit-rate label.
- Preserve local-only Neo architecture and current public provider API unless a small internal extension is necessary.
- Avoid compact/micro behavior changes while the compact refactor is in flight.

## Non-Goals

- Do not implement Google `cachedContents` lifecycle in this change.
- Do not make full compaction decisions depend on cache hit rate. Cache hits reduce cost/latency, not model context-window size.
- Do not add hosted telemetry or provider-specific analytics services.
- Do not preserve compatibility aliases or duplicate paths. Replace the old Anthropic cache-control injector.
- Do not change durable session history to chase cache hits.

## Approach Options

### Option A: Metrics-first only

Fix Anthropic usage parsing and TUI hit-rate display, but leave request cache anchors alone.

This is safe and makes the dashboard trustworthy, but it leaves obvious provider-request waste untouched. It is useful as a first commit, not as the full solution.

### Option B: Request-shape repair plus metrics (recommended)

Fix usage parsing, redesign Anthropic cache anchors, add provider golden tests, add TUI hit-rate display, and add prefix-stability tests. Keep compact and Google out of scope.

This directly addresses both live observations and code-level defects with narrow blast radius. It gives us local evidence and provider request evidence without changing runtime compaction semantics.

### Option C: Broad cache platform

Do Option B plus Google cached-content lifecycle, compact threshold changes, and cache-aware runtime policy.

This is too broad for one change. Google cache requires its own resource lifecycle. Cache-aware compaction can be actively wrong because cached tokens still occupy the model context window.

## Recommended Design

Use Option B.

The design has four implementation units.

1. Anthropic usage accumulator
   - Store usage as partial fields during SSE parsing.
   - Merge `message_start.message.usage` for input/cache tokens.
   - Merge `message_delta.usage` for output tokens.
   - Emit `TokenUsage` at `MessageEnd` when enough information exists.

2. Anthropic cache anchor planner
   - Replace `inject_cache_control_on_last_message` with a small planner inside `anthropic.rs`.
   - Keep the public `ChatMessage` API unchanged for now.
   - While building Anthropic message bodies, track origin: `RealUser`, `Assistant`, `ToolResult`.
   - Add cache control to at most two message bodies: latest `RealUser` and latest body, with de-duplication.
   - System and last tool definition keep their existing cache-control slots.

3. Runtime prefix stability guardrails
   - Add tests that demonstrate runtime state changes do not rewrite old request prefixes.
   - Treat injected runtime reminders as non-real-user anchors for Anthropic caching.
   - Document `ContextAppendTransform` as request-local append-only behavior.

4. TUI cache visibility
   - Add a conservative cache hit-rate label derived from cumulative provider-reported usage.
   - Keep raw read/write values.
   - Use language that does not overpromise exact billing semantics across providers.

## Cache Anchor Semantics

Anthropic-compatible request bodies should use no more than four cache-control blocks.

```text
slot 1: system prompt text block, when present
slot 2: last tool definition, when tools are present
slot 3: latest real user message body, when present
slot 4: latest message body, when distinct from slot 3
```

`ToolResult` bodies are Anthropic `role: "user"`, but they are not real user prompts. They may be selected as the tail anchor, but they must not replace the latest real user anchor.

This avoids the naive `second-last + last` strategy. In tool loops, the previous tail can be pushed earlier by the next assistant/tool-result pair. The real user prompt remains the stable intra-turn anchor, while the tail anchor prepares the next request.

## Usage Parsing Semantics

Anthropic-compatible streams can split usage across events.

```text
message_start.message.usage:
  input_tokens
  cache_read_input_tokens
  cache_creation_input_tokens

message_delta.usage:
  output_tokens
```

Neo should merge these fields instead of requiring one event to contain the complete usage object. If input/cache data exists but output data is absent, output defaults to zero. If output exists but input is absent, do not invent input usage.

## Hit-Rate Display Semantics

The footer hit-rate is a provider-reported cache read share, not a billing guarantee.

```text
cache denominator = max(input_tokens, input_cache_read_tokens + input_cache_write_tokens)
hit rate = input_cache_read_tokens / denominator
```

If the denominator is zero, omit the hit-rate label. Preserve the existing raw cache read/write label.

## Handling The 1M Warmup Observation

The 1M uncached plateau is not proof of a Neo total-miss bug. It suggests one or more of these:

- The Anthropic-compatible backend warms or materializes cache entries after enough stable repeated prefixes.
- DeepSeek's dashboard aggregates across requests and may report delayed cache-read visibility.
- Neo's current message anchor is sufficient for eventual high hit rate, but inefficient during warmup or tool-loop phases.
- Neo local usage parsing is currently unable to validate the dashboard signal for Anthropic-compatible streams.

The implementation should therefore optimize miss slope and observability, not chase zero misses. Success is a smaller uncached slope after warmup and locally visible cache usage, while preserving high eventual hit rates.

## Files

- `crates/neo-ai/src/providers/anthropic.rs`
  - Anthropic usage accumulator and cache anchor planner.
- `crates/neo-ai/tests/real_provider_adapters.rs`
  - Realistic Anthropic SSE usage tests and cache-control request-body golden tests.
- `crates/neo-tui/src/shell/context.rs`
  - Cache hit-rate label.
- `crates/neo-agent/src/modes/interactive/tests.rs`
  - Footer replay/render tests.
- `crates/neo-agent-core/src/runtime/config.rs`
  - Documentation for context append transform constraints.
- `crates/neo-agent-core/src/runtime/chat_request.rs`
  - Prefix stability tests or helpers if needed.
- `crates/neo-agent-core/tests/runtime_turn.rs`
  - Runtime prefix stability tests.
- `crates/neo-agent-core/src/multi_agent/runtime.rs`
  - Optional follow-up for subagent system prompt stabilization.
- `crates/neo-agent-core/tests/multi_agent_runtime.rs`
  - Optional follow-up tests for subagent system prompt stability.

## Testing Strategy

Use narrow exact tests only.

- Provider parser tests prove realistic Anthropic SSE usage is emitted as `TokenUsage`.
- Provider request-body tests count `cache_control` blocks and verify anchor placement.
- TUI tests prove cache hit-rate label is stable in footer replay.
- Runtime tests prove old prefixes remain unchanged across live state changes.
- No full cargo test as evidence.

## Deferred Work

- Google context caching: requires separate cached-content lifecycle design.
- Cache-aware compaction: defer until compact refactor settles; do not use cache hit rate as a reason to exceed context safety thresholds.
- Deep provider dashboard experiment harness: useful later, but this plan uses local golden tests plus manual dashboard observation.

## Self-Review

- Placeholder scan: no placeholders remain.
- Scope check: focused on Anthropic-compatible cache correctness and visibility; Google and compact are explicitly deferred.
- Ambiguity check: hit-rate semantics and cache-anchor selection are explicit.
- Consistency check: implementation units map directly to files and tests in the plan.
