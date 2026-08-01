# DeepSeek Anthropic Cache Probe Design

Date: `2026-08-01`

Status: `approved design`

## 1. Purpose

Add a repository-owned test utility that measures whether consecutive Neo
requests preserve cacheable prompt prefixes and correlates any new uncached
input with the tool activity appended between requests.

The utility is not a Neo feature. It must not change Neo runtime behavior,
session history, provider request construction, or the terminal interface.

The first complete target is DeepSeek's Anthropic-compatible Messages API.

## 2. Existing Baseline

This design builds on
`docs/aegis/specs/2026-07-08-prompt-cache-hit-rate-design.md`:

- provider-reported cache usage is evidence, not a billing guarantee;
- cache success is not equivalent to zero uncached input;
- historical request prefixes should remain stable except for intentional
  append-only growth;
- Google cached-content lifecycle and cache-aware compaction remain separate
  concerns.

Current Neo behavior relevant to the probe:

- the Anthropic client posts to `<base_url>/messages`;
- authentication uses `x-api-key`;
- request bodies contain `model`, `system`, `tools`, `messages`, optional
  thinking settings, and `metadata.user_id` when available;
- streamed usage may be split between message-start and message-delta events;
- cache usage is exposed as input, cache-read, and cache-creation token fields.

## 3. Confirmed Scope

### In scope

- A local HTTP forwarding proxy bound to `127.0.0.1`.
- Transparent forwarding from Neo to a configured DeepSeek
  Anthropic-compatible upstream base URL.
- Unbuffered forwarding of streamed responses.
- Local capture of every Anthropic Messages request body.
- Local extraction of DeepSeek Anthropic-compatible usage events.
- Conservative request-lineage matching when main-agent and child-agent
  requests are interleaved.
- Historical-prefix comparison between a request and its matched predecessor.
- Per-request appended-content and tool attribution.
- Cache increment, variance, and spike reporting.
- A live local web page backed by the same report data written to disk.
- A stable JSON report that can be read directly by another coding agent.

### Out of scope

- Changes to Neo provider or runtime code.
- OpenAI-compatible, OpenAI Responses, Google, or native Anthropic validation.
- Provider dashboard scraping.
- Remote telemetry, uploads, accounts, authentication, or shared storage.
- A database or long-running report service.
- Exact provider tokenization or recreation of DeepSeek's internal cache key.
- Automatic modification of the user's Neo configuration.
- Guessing relationships between requests when no safe predecessor exists.

## 4. Decision Review

### First-principles invariants

- The probe must observe the real request that leaves Neo.
- The probe must not alter Neo's request-building path.
- Cache-prefix claims require request-shape evidence and provider-usage
  evidence to remain separate.
- Authentication material must never be persisted.
- Ambiguous request relationships must start an independent sequence rather
  than being guessed.

### Smallest sufficient path

Use one Python standard-library process as the forwarding proxy, report writer,
and web server. Use one static HTML file for presentation. Do not add a Rust
crate, package manager, database, or front-end build.

### Ownership

- `tools/cache_probe.py` owns capture, forwarding, analysis, and report data.
- `tools/cache_probe.html` owns presentation only.
- `report.json` is the only complete analyzed result.
- `events.jsonl` is append-only raw evidence for recovery and debugging.

No second analysis implementation may be placed in the web page.

## 5. Repository Layout

Committed files:

```text
tools/cache_probe.py
tools/cache_probe.html
```

Run output uses the already ignored Cargo target directory:

```text
target/cache-probe/<run-id>/
├── events.jsonl
├── report.json
└── requests/
    ├── 0001.json
    ├── 0002.json
    └── ...
```

No generated run output is committed.

## 6. Invocation

The script accepts only the settings needed for a run:

```text
python3 tools/cache_probe.py \
  --upstream-base <deepseek-anthropic-base-url> \
  --port 8787
```

Defaults:

- listen address: `127.0.0.1`;
- port: `8787`;
- output root: `target/cache-probe`;
- run identifier: UTC timestamp plus a short random suffix.

The script prints:

- the local proxy base URL to place in the temporary Neo provider entry;
- the local dashboard URL;
- the absolute report path;
- the absolute raw-request directory.

Neo continues to append `/messages`, so the proxy forwards the received path to
the configured upstream base URL without rewriting provider semantics.

## 7. Forwarding Behavior

The proxy uses Python standard-library HTTP server and client modules.

For `POST /messages`:

1. Read and validate a bounded JSON request body.
2. Persist the request body before forwarding.
3. Forward the method, path, body, and required request headers upstream.
4. Exclude hop-by-hop headers and replace the upstream host header.
5. Never log or persist `x-api-key`, authorization headers, cookies, or proxy
   credentials.
6. Relay the upstream status and safe response headers.
7. Relay streamed response bytes immediately and flush after each chunk.
8. Parse a side copy of streamed events without delaying the forwarded bytes.
9. Finish the report entry when the stream ends or fails.

The proxy must not retry requests. Neo and the provider remain the only owners
of retry behavior.

The server uses a threaded request handler so overlapping main-agent and child
requests do not block one another.

## 8. Request Lineage

Global arrival order is insufficient because Delegate-family work can create
interleaved provider requests. The probe therefore finds a predecessor before
performing a prefix comparison.

Candidate requests must have the same:

- provider route;
- model;
- `metadata.user_id`, including both being absent.

Before lineage or prefix comparison, recursively remove every `cache_control`
field from comparison-only copies. DeepSeek ignores these Anthropic markers;
the persisted request body and its raw hash remain unchanged.

Among candidates, the preferred predecessor is the most recent request whose
normalized entire `messages` array is an exact prefix of the normalized current
`messages` array.
When more than one candidate matches, choose the candidate with the longest
messages array, then the most recent candidate.

If no exact predecessor exists, use the first message as a conservative
sequence anchor. A historical-mutation comparison is allowed only when exactly
one established sequence under the same candidate identity has the same first
message. Compare against that sequence's latest request and report the changed
historical paths. The first message itself is never treated as a safe mutation
anchor.

If there is no unique anchored sequence:

- mark the request as `new_sequence`;
- do not claim that the prefix is stable;
- include the nearest common-prefix length as diagnostic data only;
- never compare it to the immediately preceding global request by default.

This prevents child-agent traffic from creating false cache-break reports. A
historical-message rewrite is reported as changed only when the conservative
anchor identifies one sequence; otherwise it remains visible as an unmatched
sequence rather than being incorrectly reported as stable.

## 9. Prefix Comparison

JSON objects are canonicalized by sorting object keys. Arrays and string bytes
retain their original order and content. Recursive `cache_control` removal is
the only comparison normalization and never rewrites stored evidence.

For a matched predecessor:

1. Let `N` be the predecessor's messages length.
2. Confirm that current messages `[0..N]` exactly equal predecessor messages.
3. Replace the current messages array with its first `N` elements.
4. Compare the complete canonicalized current body with the canonicalized
   predecessor body.

After comparison normalization, this comparison includes:

- model;
- system content;
- complete tool definitions and order;
- historical messages and content blocks;
- thinking settings;
- metadata and provider options;
- all other request fields.

No field other than DeepSeek-ignored `cache_control` is silently ignored. A
changed value produces:

- `prefix_status: changed`;
- the first changed JSON path;
- all changed paths up to a fixed reporting limit;
- predecessor and current canonical hashes.

Successful equality produces `prefix_status: stable`. The first request in a
sequence produces `prefix_status: first-req` and is excluded from the stable
rate denominator.

`stable` means Neo preserved the observable Anthropic request prefix. It does
not prove that DeepSeek internally selected the same cache entry.

## 10. Increment And Tool Attribution

For a matched predecessor, the appended increment is always the current
messages tail after the predecessor's message count. Non-message changes such
as `system` or tool definitions may mark the prefix changed, but must not hide
the independently computed message activity.

Record:

- appended message count;
- canonical serialized byte count;
- content-block counts by type;
- tool-use names, identifiers, argument byte counts, and bounded summaries;
- tool-result identifiers, byte counts, and bounded summaries;
- assistant text and thinking byte counts and bounded previews;
- user text byte count.

Tool-result identifiers are resolved to tool names from tool-use blocks already
present in the current message history. Unresolved identifiers remain explicit
and are not assigned to a guessed tool.

If one increment contains multiple tool calls, list every tool. Do not divide
provider token usage among them because the response does not provide that
resolution.

The side-copy stream parser also records bounded response activity summaries
while forwarding bytes unchanged. It counts and previews tool arguments,
thinking, and assistant text without buffering the complete response. Request
history remains the primary activity evidence; the response summary covers the
latest response before it can appear in a later request history.

## 11. DeepSeek Usage Semantics

The streamed event parser extracts these Anthropic-compatible fields wherever
DeepSeek emits them:

```text
input_tokens
output_tokens
cache_read_input_tokens
cache_creation_input_tokens
```

The report keeps the raw event values and the final merged values.

Derived values for this target are:

```text
cache_hit_tokens = cache_read_input_tokens
cache_creation_tokens = cache_creation_input_tokens
uncached_input_tokens = input_tokens
observed_input_tokens = input_tokens
                      + cache_read_input_tokens
                      + cache_creation_input_tokens
```

Missing fields remain null rather than becoming invented zeros. A request with
no usable usage event is marked `usage_status: missing` and is excluded from
numeric variance calculations.

## 12. Variance And Spike Rules

Statistics are calculated per matched request sequence, not across unrelated
agents.

For each usable request, report:

- `cache_creation_tokens`;
- `uncached_input_tokens`;
- running mean;
- population variance;
- population standard deviation.

A numeric spike is reported only when at least five earlier usable samples
exist in the same sequence and:

```text
current uncached_input_tokens > previous mean + 3 * previous standard deviation
```

When the previous standard deviation is zero, any value greater than the
previous mean is a spike.

Structural status and numeric status remain independent:

- stable prefix with expected new input;
- stable prefix with numeric spike;
- changed prefix with or without a numeric spike;
- `first-req` for each independently identified sequence.

## 13. Report Format

`events.jsonl` contains append-only records for:

- run start;
- request received;
- request forwarded;
- usage event observed;
- response completed or failed;
- analyzed request summary.

`report.json` is rewritten atomically after each completed request and contains:

```text
run
summary
sequences[]
requests[]
tool_summary[]
warnings[]
```

Every analyzed request includes:

- sequence and predecessor identifiers;
- timestamps and duration;
- model and metadata user identifier;
- request file path and canonical hashes;
- prefix status and changed paths;
- appended-content summary;
- tool attribution;
- raw and derived usage;
- variance and spike data;
- forwarding status or error.

The report uses stable field names and deterministic ordering so another coding
agent can compare runs without reading the HTML.

## 14. Web Page

The page is served from the same process and polls `report.json` once per
second.

The first view contains:

- request count;
- sequence count;
- stable-prefix rate among comparable requests;
- cache-hit tokens;
- provider cache-creation tokens;
- uncached input tokens;
- total observed input tokens;
- structural-change count;
- numeric-spike count.

The request table contains:

- request number;
- sequence;
- model;
- prefix status;
- attributed tools;
- appended bytes;
- cache-hit tokens;
- provider cache-creation tokens;
- uncached input tokens;
- spike status;
- duration.

Selecting a row shows changed paths, appended block summaries, raw usage,
canonical hashes, warnings, and the request file path.

Two native canvas charts are sufficient:

- cache-hit and uncached input tokens by request within a selected sequence;
- average uncached input tokens and variance grouped by tool name.

The page contains no independent comparison or statistics code beyond display
formatting. All analyzed values come from `report.json`.

## 15. Error Handling

- Invalid request JSON: persist a bounded error record and return a client
  error without forwarding.
- Oversized request: reject before allocating an unbounded buffer.
- Upstream connection failure: return a gateway error and record the failure.
- Mid-stream upstream failure: close the client stream and record an incomplete
  response.
- Malformed streamed event: keep forwarding, record a parse warning, and do not
  fabricate usage.
- Report write failure: keep proxying, write the error to standard error, and
  retain the append-only event log when possible.
- Browser disconnect: do not cancel or corrupt an active provider request.

## 16. Security And Privacy

- Bind only to `127.0.0.1`.
- Never persist request headers.
- Never persist authentication values.
- Do not expose a remote-listen option in the first implementation.
- Store full request bodies locally because exact historical comparison is the
  purpose of the tool.
- Print an explicit startup warning that request bodies may contain source code,
  prompts, and tool output.
- Place all generated files under the selected local output directory.

## 17. Acceptance Criteria

### Forwarding

- A Neo Anthropic-compatible request reaches a local fixture upstream through
  the proxy.
- The client receives streamed bytes before the upstream stream completes.
- Status codes and usable response headers are preserved.
- Authentication reaches the fixture but is absent from every generated file.

### Prefix analysis

- An append-only second request is reported as stable.
- A changed system block, tool schema, historical message, metadata value, or
  thinking setting is reported as changed with the first differing path.
- Interleaved independent request sequences are matched to their own
  predecessors when an exact predecessor exists.
- A non-anchor historical-message mutation is reported as changed when the
  first-message anchor identifies exactly one established sequence.
- A request with no safe predecessor starts an independent sequence and is
  reported as `first-req`, never stable.

### Usage and attribution

- Split DeepSeek Anthropic-compatible usage events merge into one request
  summary.
- Missing usage remains missing.
- Appended tool-use and tool-result blocks name the involved tools.
- Five prior samples are required before a numeric spike can be emitted.

### Artifacts and page

- `events.jsonl`, `report.json`, and numbered request files are produced.
- The page updates without a build step or external network dependency.
- The page and `report.json` show the same analyzed values.
- A coding agent can determine prefix stability, changed paths, tool
  attribution, and cache-token trends from `report.json` alone.

## 18. Verification Boundary

Implementation verification uses a local fixture server, never a required live
DeepSeek call.

One focused script self-test should cover:

- append-only stability;
- a deliberate prefix mutation;
- interleaved sequences;
- split usage events;
- authentication redaction;
- streamed first-byte forwarding.

A later manual live run provides provider evidence but is not required for the
deterministic repository check. Live evidence must state the exact model,
upstream base URL category, request count, and report path. It must not be
presented as proof for other providers.

## 19. Complexity Budget

- Two committed files.
- Python standard library only.
- One runtime process.
- One analyzed report owner.
- No persistent service and no compatibility layer.

If transparent streaming cannot be implemented correctly with the Python
standard library, stop and revise the design before adding a dependency. Do not
silently buffer complete model responses merely to keep the implementation
small.

## 20. Design Records

### Task intent

- Outcome: make Neo cache-prefix stability and cache increments directly
  measurable during real DeepSeek Anthropic-compatible runs.
- Success evidence: deterministic fixture proof plus a readable local report.
- Stop condition: the proxy, analysis report, and page satisfy the acceptance
  criteria without modifying Neo runtime code.
- Non-goal: general provider observability platform.

### Baseline usage

- Required baseline:
  `docs/aegis/specs/2026-07-08-prompt-cache-hit-rate-design.md`.
- Current runtime source checked: Anthropic request construction and endpoint
  behavior.
- Missing authority: DeepSeek may change undocumented usage event details;
  malformed or missing fields therefore remain visible instead of guessed.
- Decision: proceed with the narrow DeepSeek Anthropic-compatible target.

### Impact statement

- Product impact: none; this is an opt-in repository test utility.
- Runtime impact: provider traffic passes through the proxy only when the user
  explicitly points a temporary provider entry at it.
- Persistence impact: local test artifacts under `target/cache-probe`.
- Compatibility impact: none for normal Neo execution.
- Main risk: an incorrect proxy could buffer or alter streamed traffic; the
  first-byte streaming check is therefore mandatory.

## 21. Self-Review

- Placeholder scan: no placeholders remain.
- Scope check: only DeepSeek's Anthropic-compatible path is complete.
- Ownership check: the script is the only analysis owner; the page presents
  report values.
- Ambiguity check: stable, changed, first request, usage missing, and numeric spike
  have explicit meanings.
- Privacy check: headers and credentials are never persisted; request bodies
  are explicitly local and sensitive.
- Failure check: the proxy never retries, guesses usage, or guesses a request
  predecessor.
- Simplicity check: two files, standard library, no build system, no database.
