# Multi-Agent Cache and Token Display Design

## Goal

Reduce fixed input overhead for Neo Multi-Agent and Swarm child turns, preserve provider prompt-cache affinity across child requests, and make cached-token usage visible in the chat transcript.

## Root Cause

Neo child agents already start with a fresh `AgentContext`, so the observed `40k tok` rows are not caused by parent conversation replay. The large count comes from the repeated request prefix each child sends: the base system prompt, workspace context, role profile, tool definitions, and model-reported input usage. Neo currently records only `input_tokens + output_tokens`, so cached input looks the same as newly processed input.

Kimi-code handles the same class of problem with three useful patterns:

- subagents start with zero parent context unless explicitly resumed;
- the session id is passed to the provider as prompt-cache affinity;
- usage is split into normal input, cache read, cache creation, and output.

## Scope

This design fixes the parts that directly benefit Neo now:

- remove the duplicate system-message tool schema catalog from chat requests;
- derive a stable prompt-cache key from the active session directory;
- map that cache key to providers that support it;
- retain cache read/write token fields in Neo usage events;
- show cache information in Delegate/Swarm transcript rows only when providers report it.

This design does not rewrite the entire prompt system, change subagent scheduling, or remove user/MCP tools from child profiles. Tool narrowing is a separate policy decision because MCP tools are user-configured and can be task-critical.

## Request Prefix Design

Neo will rely on provider-native tool schemas instead of duplicating the full tool schema JSON inside a system message. The system prompt remains first, followed by compact workspace context and conversation messages. The request still includes `ChatRequest.tools`, so tool-capable providers receive the full schema through the intended API field.

Before:

```text
system: base system prompt
system: <available_tools_schema>[large JSON copy]</available_tools_schema>
system: <environment_context>...</environment_context>
messages: turn history
tools: [same large schema again]
```

After:

```text
system: base system prompt
system: <environment_context>...</environment_context>
messages: turn history
tools: [single provider-native schema]
```

This is the largest low-risk token reduction because it removes repeated schema bytes from every parent and child request without changing the tool registry.

## Cache Affinity Design

`AgentConfig.session_directory` already points at a session directory such as:

```text
~/.neo/sessions/wd_neo_hash/session_00000000-0000-4000-8000-000000000001/
```

`chat_request` will derive the basename as the request cache key:

```text
session_00000000-0000-4000-8000-000000000001
```

Provider mapping:

- OpenAI Responses and OpenAI-compatible: `prompt_cache_key = session_id`, existing provider code already supports this.
- Anthropic: `metadata.user_id = session_id`, matching kimi-code's session-affinity mapping.
- Providers without cache affinity support ignore the value.

Child agents inherit the parent `AgentConfig`, so swarm siblings use the same session-level key. This favors cache hits because their static prefix is shared and only the final child task prompt differs.

## Token Usage Model

Neo keeps `input_tokens` and `output_tokens`, then adds:

```rust
input_cache_read_tokens: u32
input_cache_write_tokens: u32
```

The fields default to zero for providers that do not report cache details. Provider adapters parse common cache shapes:

- Anthropic: `cache_read_input_tokens`, `cache_creation_input_tokens`;
- OpenAI/Kimi-compatible style: nested `*_tokens_details.cached_tokens` when available.

The existing total token count remains `input_tokens + output_tokens` so legacy provider totals are not double-counted. Cache read/write counts are displayed as a breakdown, not added again into the main `tok` number.

## UI Design

When cache information is absent, rows remain unchanged:

```text
├─ Newton  [Explorer] ✓ [■■■■■■■■]  done · 0 tools · 5s · 40k tok · summary...
```

When cache information exists, rows add a compact cache segment:

```text
├─ Newton  [Explorer] ✓ [■■■■■■■■]  done · 0 tools · 5s · 40k tok · cache 37k read / 1k write · summary...
```

For single Delegate cards and grouped delegate lists the same compact segment is used:

```text
● Huygens [Explorer] done · 0 tools · 4s · 40k tok · cache 38k read
```

The UI never invents a cache hit rate when the provider does not report cache tokens. It only renders concrete read/write counts.

## Tests

Narrow verification should cover:

- `chat_request` no longer injects `<available_tools_schema>` but still sends `request.tools`;
- `chat_request` derives `RequestOptions.session_id` from `session_directory`;
- Anthropic request bodies map `session_id` to `metadata.user_id`;
- provider usage parsing preserves cache read/write fields;
- multi-agent snapshots accumulate cache read/write counts;
- Swarm transcript rows render the cache segment when present.

## Risks

Removing the system-message schema catalog relies on models respecting provider-native tool schemas. That is the correct wire contract for tool-capable providers and avoids two sources of truth. If a provider cannot use tools, Neo already has capability validation and should fail before relying on prose schema instructions.

Cache accounting varies by provider. Neo treats cache fields as a visible breakdown rather than billing math, so incorrect double-counting is avoided in the main token total.
