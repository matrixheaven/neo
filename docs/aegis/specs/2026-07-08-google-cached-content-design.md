# Neo Google Cached Content Design

## Background

Neo's current Google provider uses the Gemini `streamGenerateContent` shape: it sends `contents`, optional `systemInstruction`, optional `tools`, and `generationConfig`, then parses `usageMetadata` into Neo token usage. It does not send `cachedContent`, does not create `cachedContents/*` resources, and currently relies on whatever implicit caching Google applies to repeated prefixes.

Google's documentation changed materially in 2026. The Interactions API is now the recommended API for the latest features and models, but its context-caching guide says that Interactions only supports implicit caching; explicit cached-content resources remain a generateContent API feature. The generateContent caching guide says Gemini supports two mechanisms: implicit caching, enabled by default for Gemini 2.5 and newer models, and explicit caching, where the client creates and manages a `CachedContent` resource. Gemini 3.5 Flash is current, has a 1,048,576-token input limit, supports caching, and has a 4,096-token implicit-cache minimum.

References checked on 2026-07-08:

- Google Interactions context caching: https://ai.google.dev/gemini-api/docs/caching
- Google generateContent context caching: https://ai.google.dev/gemini-api/docs/generate-content/caching
- Google Caching API reference: https://ai.google.dev/api/caching
- Google GenerateContent API reference: https://ai.google.dev/api/generate-content
- Gemini 3.5 Flash model page: https://ai.google.dev/gemini-api/docs/models/gemini-3.5-flash

## Goals

- Make Google cache reporting accurate in Neo by mapping Gemini `usageMetadata.cachedContentTokenCount` into Neo cache-read usage.
- Prefer implicit caching as the default behavior for Gemini 2.5+ and Gemini 3.5+ because it is automatic and requires no new lifecycle state.
- Add an explicit cached-content design that is opt-in, local-only, session-scoped, and safe to disable.
- Reuse stable request parts only: system instruction and tool declarations first; large repeated user-provided context only when Neo can prove it is stable.
- Avoid changing Neo's provider-neutral `ChatRequest` contract unless a small optional extension is clearly necessary.
- Avoid building on Interactions API until Neo explicitly migrates from generateContent.

## Non-Goals

- Do not migrate Neo's Google provider to the Interactions API in this design.
- Do not cache arbitrary conversation history by default.
- Do not upload local repository files into Google Files API as part of this change.
- Do not create global or cross-project cachedContent resources.
- Do not make cached content a way to bypass Neo context-window budgeting; cached tokens still count toward the effective prompt size.
- Do not implement Google Cloud Vertex-specific context-cache APIs in the Gemini Developer API provider.

## Current Code Shape

- `crates/neo-ai/src/providers/google.rs` builds `streamGenerateContent` requests with `contents`, `systemInstruction`, `tools`, and `generationConfig`.
- The same file parses `usageMetadata` using `token_usage_from(v, "promptTokenCount", "candidatesTokenCount")`.
- `token_usage_from` does not currently recognize Gemini's `cachedContentTokenCount`, so Google cache hits can be underreported locally.
- `RequestOptions.cache` exists, but Google does not use it.

## Approach Options

### Option A: Implicit caching only, plus accurate reporting

Map `cachedContentTokenCount` to `input_cache_read_tokens`, keep request shape unchanged, and document that Gemini 2.5+ and 3.5+ implicit caching is automatic.

This is the safest first step. It improves observability and lets Neo benefit from current Google defaults without creating persistent Google resources.

### Option B: Opt-in explicit cache for stable system/tools

Add a local cached-content manager for Google generateContent. When enabled, Neo creates a `cachedContents/*` resource for stable systemInstruction and tool declarations, stores the cache name in the session directory, and sends `cachedContent` on subsequent requests.

This can guarantee savings for repeated stable prefixes, but it adds remote resource lifecycle, TTL, invalidation, and error handling. The likely token volume of system/tools may be below the threshold where explicit caching is worthwhile.

### Option C: Explicit cache for large user/repo context

Cache large repeated user context or repository snapshots through `contents[]` and reference them by cache name.

This is potentially high leverage for repeated repo analysis, but it requires a separate product design: deciding what content is stable, what can be uploaded, how to handle privacy, how to refresh stale context, and how to keep local-only expectations clear.

## Recommended Design

Use Option A first and design Option B as a gated follow-up.

The first implementation should only fix local accounting and add explicit configuration scaffolding without creating remote cache resources. Once Google cache usage is visible and trusted, an explicit cache feature can be enabled behind `runtime.google_cache.explicit = true` or a provider-scoped equivalent.

## Detailed Design

### Phase 1: Reporting-only implicit cache support

Map Gemini usage as follows:

```text
promptTokenCount          -> TokenUsage.input_tokens
candidatesTokenCount      -> TokenUsage.output_tokens
cachedContentTokenCount   -> TokenUsage.input_cache_read_tokens
```

Do not subtract cached tokens from `input_tokens`. Google documents that `promptTokenCount` remains the total effective prompt size when `cachedContent` is set. Neo should preserve that meaning and display cache hits as a secondary field.

This phase changes only `neo-ai` parsing and tests.

### Phase 2: Explicit cache feature gate

Add a disabled-by-default config surface:

```toml
[runtime.google_cache]
explicit = false
ttl_seconds = 3600
scope = "system_tools"
min_estimated_tokens = 4096
```

`scope = "system_tools"` means only Google `systemInstruction`, `tools`, and `toolConfig` are eligible. This avoids caching user conversation history or workspace content by accident.

### Phase 3: Cached content lifecycle

When explicit caching is enabled and the eligible payload exceeds `min_estimated_tokens`:

1. Build a canonical cache key from provider id, model id, systemInstruction bytes, tool declarations, toolConfig, and TTL class.
2. Look for a session-local cache record under the Neo session directory.
3. If a non-expired record exists, send `cachedContent: "cachedContents/..."` and omit the cached system/tools from the generateContent request.
4. If no record exists, call `POST https://generativelanguage.googleapis.com/v1beta/cachedContents` with `model`, `systemInstruction`, `tools`, optional `toolConfig`, and `ttl`.
5. Store the returned `name`, `expireTime`, cache key, model, provider id, and token metadata in session-local JSON.
6. On 404/expired cache during generation, delete the stale local record and retry once without explicit cache or after recreating the cache.

### Request Shape

When no explicit cache is used, keep the existing request shape:

```json
{
  "contents": [...],
  "systemInstruction": {...},
  "tools": [...],
  "generationConfig": {...}
}
```

When explicit cache is used for system/tools:

```json
{
  "cachedContent": "cachedContents/abc123",
  "contents": [...],
  "generationConfig": {...}
}
```

Do not also send the same `systemInstruction` and `tools` if they are inside the cached content, because duplicated context would waste tokens and may confuse the model.

### Error Handling

- Cache create failure: log/emit provider warning if Neo has such a channel; otherwise fall back to normal uncached generation.
- Generate with stale cache returns not found or invalid cache: clear local cache record and retry once uncached.
- Cache patch/delete failure: do not fail the user request.
- Model mismatch: never reuse the cache. Google says cached content can only be used with the model it was created for.

### Privacy and Local-Only Semantics

Explicit cachedContent is remote storage with user-defined TTL. This must be treated differently from implicit in-memory caching. The first explicit scope must avoid arbitrary repo files and user transcript history. Any future `large_user_context` or `workspace_snapshot` scope requires explicit user-facing docs and opt-in configuration.

### Testing

- Unit test Google usage parsing maps `cachedContentTokenCount` to `input_cache_read_tokens`.
- Provider request-body test verifies no `cachedContent` appears by default.
- Explicit-cache disabled test verifies `RequestOptions.cache` alone does not create remote resources.
- Future explicit-cache tests should use a mock server with two endpoints: `POST /cachedContents` and `POST /models/{model}:streamGenerateContent`.
- Test stale-cache recovery with a first generate response returning 404-like provider error, then one retry without cachedContent.

## Review Notes

- The scope is intentionally split. Phase 1 is safe and should be implemented first. Phases 2-3 should not be implemented until the user explicitly asks for explicit Google cache behavior.
- The design does not rely on Anthropic `cache_control`; Google cachedContent is a resource lifecycle and request reference.
- The design respects Gemini 3.5 Flash's current caching support and the Interactions/generateContent split.
- No placeholders remain.
- Ambiguous point resolved: cached tokens remain part of effective context size and must not affect compaction safety thresholds.

