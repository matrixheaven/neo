# DeepSeek Anthropic Cache Probe Implementation Plan

Date: `2026-08-01`

Status: `approved design; ready for implementation`

## Goal

Implement the approved local forwarding proxy and dashboard that measure
DeepSeek Anthropic-compatible cache-prefix stability, cache usage, request
increments, and tool attribution without changing Neo runtime code.

The finished utility consists of exactly two committed files:

```text
tools/cache_probe.py
tools/cache_probe.html
```

All generated evidence stays under `target/cache-probe/<run-id>/`.

## Architecture

Keep one process and one analysis owner:

```text
Neo Anthropic request
  -> ThreadingHTTPServer POST /messages
  -> persist request body
  -> analyze lineage and historical prefix
  -> forward to configured DeepSeek upstream
  -> relay response chunks immediately
  -> parse a side copy of streamed usage events
  -> atomically rewrite report.json
  -> GET /report.json
  -> cache_probe.html presentation
```

`tools/cache_probe.py` owns forwarding, persistence, sequence matching,
comparison, statistics, and report generation. `tools/cache_probe.html` only
renders values from `report.json`.

## Tech Stack

- Python `3.11+` standard library only;
- `argparse`, `dataclasses`, `hashlib`, `http.client`, `http.server`, `json`,
  `math`, `pathlib`, `statistics`, `threading`, `time`, `urllib.parse`, and
  `uuid`;
- plain HTML, CSS, and browser JavaScript;
- native canvas for the two charts;
- no Python package, JavaScript package, Rust crate, database, build step, or
  remote service.

## Baseline And Authority Refs

- approved design:
  `docs/aegis/specs/2026-08-01-deepseek-cache-probe-design.md`;
- cache behavior baseline:
  `docs/aegis/specs/2026-07-08-prompt-cache-hit-rate-design.md`;
- current provider implementation:
  `crates/neo-ai/src/providers/anthropic.rs`;
- current request identity construction:
  `crates/neo-agent-core/src/runtime/chat_request.rs`;
- user approval on `2026-08-01` for the local proxy and the DeepSeek
  Anthropic-compatible first target.

## Compatibility Boundary

- do not modify Neo source, configuration formats, sessions, provider clients,
  terminal behavior, or tool behavior;
- do not add a second report analyzer in the web page;
- do not retry provider requests;
- do not buffer the complete provider response before forwarding;
- do not persist headers or credentials;
- do not infer a request predecessor when identity and the conservative message
  anchor are ambiguous;
- do not claim support for other provider protocols;
- keep generated output inside the already ignored `target` directory;
- use `Path` operations and standard-library networking without Unix-only
  process or signal assumptions.

## TDD Route

- Mode: `off`
- Decision: `skipped`
- Strict authority: `not applicable`
- Test posture: post-change deterministic self-test
- Reason: strict test-first work was not requested; the approved design already
  fixes behavior, ownership, and acceptance boundaries.
- Verification: `python3 tools/cache_probe.py --self-test` exercises analysis,
  forwarding, streaming, redaction, report generation, and page serving against
  an in-process local fixture.

## Verification

Focused evidence must prove:

1. an append-only request matches its correct predecessor and reports a stable
   prefix;
2. system, tools, metadata, thinking, and non-anchor historical-message changes
   report changed paths;
3. interleaved request sequences do not compare against global arrival order;
4. ambiguous predecessors remain unknown;
5. appended tool-use and tool-result blocks identify their tool names;
6. split streamed usage fields merge without invented zero values;
7. variance and spikes are calculated within one sequence only;
8. upstream authentication is forwarded but absent from all files;
9. the first streamed response bytes reach the client before upstream
   completion;
10. browser data comes only from `report.json`;
11. the dashboard remains usable at narrow and desktop widths;
12. normal Neo code and Cargo manifests remain untouched.

## Scope Check

### Plan basis

- Fact: Neo posts Anthropic requests to `<base_url>/messages` and uses
  `x-api-key`.
- Fact: request bodies contain all prompt-relevant fields needed for structural
  comparison.
- Fact: the current child-agent identifier is not serialized into the provider
  body, so `metadata.user_id` alone cannot separate concurrent child sequences.
- Fact: `target` is already ignored by Git.
- Assumption: DeepSeek keeps the documented Anthropic-compatible event field
  names used by the approved design.
- Unknown: a live upstream may add headers or event fields not present in the
  deterministic fixture; unknown fields must be forwarded and recorded as
  warnings rather than guessed.

### Requirement Ready Check

- Requirement source refs: approved design and current user instruction.
- Goals and scope refs: design sections 1 through 4.
- User and scenario refs: repeated Neo turns, tool loops, and interleaved child
  requests.
- Requirement item refs: forwarding, prefix comparison, usage, variance,
  attribution, artifacts, and dashboard sections.
- Acceptance refs: design section 17 and verification boundary section 18.
- Open blocker questions: none.
- Decision: `ready`.

### BaselineUsageDraft

- Required baseline refs: approved cache-probe design and cache behavior
  baseline.
- Acknowledged before plan refs: both baseline specs and current Anthropic
  request construction.
- Cited in plan refs: all required refs.
- Missing refs: no blocking reference; live DeepSeek behavior remains later
  provider evidence.
- Decision: `continue`.

### Change Necessity

- User-visible need: measure actual outgoing requests and cache results during
  real Neo runs.
- No-change option: existing session JSONL and TUI usage totals do not contain
  the final provider body or request-by-request structural comparison.
- Why code is necessary: only a forwarding process on the actual HTTP path can
  capture both request shape and streamed provider usage without changing Neo.
- Minimum boundary: the two approved files under `tools/`.
- Decision: `code-change`.

### Existence Check

- Proposed surface: one opt-in local cache probe.
- Existing reuse candidate: Neo session logs and provider tests.
- Why insufficient: they cannot observe arbitrary real requests and streamed
  upstream usage together.
- Creation proof: the user explicitly approved the local forwarding approach.
- Entropy impact: two dependency-free files; generated data retires with its
  run directory.
- Decision: `add-with-proof`.

### Architecture Integrity Lens

- Invariant: analysis must describe the same request and response that Neo used.
- Canonical owner: `cache_probe.py` owns all analyzed values.
- Responsibility overlap: the page performs formatting only.
- Higher-level simplification: no Neo hook, library crate, or database is
  required.
- Retirement falsifier: if standard-library forwarding cannot remain truly
  streamed, stop and return to design before adding a dependency.
- Verdict: `proceed`.

### Plan Pressure Test

- Owner and retirement: one script owner, no old implementation to retain.
- Architecture integrity: external forwarding remains the lowest-coupling path.
- Verification scope: in-process fixture covers deterministic behavior; a live
  call remains separate provider evidence.
- Task executability: three implementation tasks, each with one focused commit.
- Pressure result: `proceed`.

### Complexity Budget

- Artifact class: repository test utility.
- Target files: one Python script and one HTML page.
- Current pressure: no existing top-level tool implementation.
- Projected pressure: the script combines server and analyzer but keeps one
  runtime owner and named pure functions.
- Budget result: `within-budget`.
- Planned governance: no framework, plugin system, class hierarchy, or provider
  abstraction.

### Execution Readiness View

- Intent Lock: measure DeepSeek Anthropic-compatible cache behavior only.
- Scope Fence: two committed files; no Neo runtime edits.
- Baseline Lock: approved design and existing cache behavior baseline.
- Approved Behavior: transparent local forwarding plus local reports and page.
- Owner Constraints: script analyzes; page presents.
- Compatibility Boundary: preserve streaming, headers, status, and Neo behavior.
- Retirement Boundary: no fallback, alias, or previous utility.
- Task Batches: analysis, forwarding, presentation.
- Test Obligations: deterministic self-test plus browser layout check.
- Review Gates: review each task diff before its commit.
- Drift Rules: stop if a dependency, Neo edit, complete-response buffering, or
  guessed predecessor becomes necessary.
- Evidence Required: self-test output, credential scan, browser screenshots,
  `git diff --check`, and final file-scope check.

## File Map

### Create `tools/cache_probe.py`

Responsibilities:

- command-line parsing and startup output;
- run directory and atomic artifact writes;
- canonical JSON comparison and hashes;
- conservative request sequence matching;
- message increment and tool attribution;
- streamed usage parsing and statistics;
- transparent request and response forwarding;
- serving the report and committed HTML page;
- deterministic in-process self-test.

### Create `tools/cache_probe.html`

Responsibilities:

- polling `/report.json` once per second;
- summary counters and request table;
- selected-request detail panel;
- sequence filter;
- cache trend and per-tool variance charts;
- responsive, accessible presentation.

## Task 1: Build The Analysis And Artifact Owner

### Files

- Create: `tools/cache_probe.py`

### Why

Prefix stability, request lineage, usage, and tool attribution must be correct
before networking and the page consume them.

### Change Necessity

Session logs cannot reconstruct the final provider request. Add one script with
pure analysis functions and an append-only run store; do not add a package or
test file.

### Impact And Compatibility

- no network traffic in this task;
- no Neo files touched;
- generated files stay under `target/cache-probe`;
- missing usage stays `null`;
- ambiguous lineage stays unknown.

### Required code shape

Create these constants and functions with the stated behavior:

```python
MAX_REQUEST_BYTES = 64 * 1024 * 1024
MAX_DIFF_PATHS = 100

def canonical_bytes(value: object) -> bytes:
    return json.dumps(
        value,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")

def json_hash(value: object) -> str:
    return hashlib.sha256(canonical_bytes(value)).hexdigest()

def identity_key(body: dict[str, object], route: str) -> tuple[object, ...]:
    metadata = body.get("metadata")
    user_id = metadata.get("user_id") if isinstance(metadata, dict) else None
    return route, body.get("model"), user_id

def first_message_anchor(body: dict[str, object]) -> bytes | None:
    messages = body.get("messages")
    if not isinstance(messages, list) or not messages:
        return None
    return canonical_bytes(messages[0])
```

Add named pure helpers for:

```text
common_message_prefix_length
select_predecessor
truncate_to_predecessor
diff_json_paths
summarize_increment
resolve_tool_names
merge_usage_event
derive_usage
population_stats
detect_spike
analyze_request
```

`select_predecessor` must apply this exact order:

1. filter by route, model, and metadata user identifier;
2. choose the longest exact historical-message prefix, then the newest request;
3. when no exact prefix exists, allow a mutation candidate only when exactly
   one established sequence has the same non-null first-message anchor;
4. otherwise create a new sequence and return no predecessor.

Use one `RunStore` class guarded by `threading.RLock`:

```python
class RunStore:
    def __init__(self, output_root: Path) -> None: ...
    def append_event(self, event: dict[str, object]) -> None: ...
    def save_request(self, request_id: int, body: object) -> Path: ...
    def begin_request(self, route: str, body: dict[str, object]) -> dict[str, object]: ...
    def finish_request(self, request_id: int, result: dict[str, object]) -> None: ...
    def report_snapshot(self) -> dict[str, object]: ...
    def write_report(self) -> None: ...
```

`write_report` writes a sibling temporary file, flushes it, and replaces
`report.json` with `os.replace`. Event records are one compact JSON object per
line. Request files are pretty-printed UTF-8 JSON.

Add `--self-test` and a temporary analysis-only self-test that creates a
temporary directory under `target/cache-probe/self-test`, runs assertions, and
prints:

```text
cache probe self-test: analysis ok
```

The assertions must cover:

- canonical object-key ordering but strict array ordering;
- append-only stable prefix;
- system and tool changes;
- one non-anchor historical-message mutation;
- two interleaved sequences;
- ambiguous identical anchors remaining unknown;
- tool identifier to tool-name resolution;
- split usage merging with missing values preserved;
- five-sample spike threshold;
- atomic report creation and expected top-level keys.

### Verification

Run:

```bash
python3 tools/cache_probe.py --self-test
```

Expected final line:

```text
cache probe self-test: analysis ok
```

Run:

```bash
git diff --check -- tools/cache_probe.py
```

Expected: exit `0`, no output.

Review that only `tools/cache_probe.py` is changed, then commit:

```bash
git add tools/cache_probe.py
git commit -m "feat(dev): add cache probe analysis"
```

## Task 2: Add Transparent Streaming Forwarding

### Files

- Modify: `tools/cache_probe.py`

### Why

The analyzer must observe the actual body Neo sends and the same response Neo
receives. Buffering or retrying would invalidate the measurement.

### Change Necessity

Pure offline analysis cannot capture real provider traffic. Add forwarding to
the same script; do not add a daemon wrapper or provider-specific Neo hook.

### Impact And Compatibility

- bind only to `127.0.0.1`;
- accept only `POST /messages` as provider traffic;
- preserve `x-api-key` upstream without persisting it;
- strip hop-by-hop headers;
- do not retry;
- force downstream connection close when response length is unknown so chunks
  can be flushed without inventing a buffered content length;
- malformed event parsing must not interrupt forwarding.

### Required code shape

Add:

```python
HOP_BY_HOP_HEADERS = {
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
}

class ProbeServer(ThreadingHTTPServer):
    daemon_threads = True

class ProbeHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def do_POST(self) -> None: ...
    def do_GET(self) -> None: ...
```

Forwarding steps in `do_POST`:

1. reject any path other than `/messages` with `404`;
2. require a numeric content length no greater than `MAX_REQUEST_BYTES`;
3. decode one JSON object and persist it;
4. build the upstream URL from `--upstream-base` plus the received path;
5. use `http.client.HTTPSConnection` or `HTTPConnection` according to the
   parsed scheme;
6. forward all end-to-end headers, replacing `Host` and recalculating
   `Content-Length`;
7. store no header values in events or reports;
8. forward the upstream status and safe headers;
9. read at most `16_384` bytes per iteration, immediately write and flush each
   non-empty chunk to Neo;
10. feed the same bytes to an incremental line buffer that recognizes
    `event:` and `data:` fields;
11. merge usage objects from message-start and message-delta events;
12. finalize report state on success, upstream failure, downstream disconnect,
    or malformed event data.

CLI arguments after this task:

```text
--upstream-base URL    required outside self-test
--port PORT            default 8787
--output-root PATH     default target/cache-probe
--self-test            run deterministic fixture checks and exit
```

Startup output must include the local base URL, dashboard URL, report path, raw
request directory, and a warning that request bodies can contain private source
code and prompts.

Extend the self-test with an in-process fixture upstream that:

- records the received `x-api-key` value in memory only;
- emits message-start usage, one delayed content event, and message-delta usage;
- flushes the first event before waiting on a synchronization event;
- returns a deliberate non-success response for a separate request.

The self-test client must prove:

- the first event is received before the fixture releases the remainder;
- all successful response bytes are unchanged and ordered;
- status and content type are preserved;
- the fixture received the authentication value;
- generated files contain neither the authentication value nor header names;
- split usage produces the expected merged and derived values;
- upstream failure becomes a recorded forwarding error without retry.

Replace the temporary final line with:

```text
cache probe self-test: proxy ok
```

### Verification

Run:

```bash
python3 tools/cache_probe.py --self-test
```

Expected final line:

```text
cache probe self-test: proxy ok
```

Run a credential scan over the deterministic output:

```bash
rg -n "self-test-secret|x-api-key|authorization" target/cache-probe/self-test
```

Expected: exit `1`, no matches.

Run:

```bash
git diff --check -- tools/cache_probe.py
```

Expected: exit `0`, no output.

Commit:

```bash
git add tools/cache_probe.py
git commit -m "feat(dev): proxy DeepSeek cache traffic"
```

## Task 3: Add The Live Dashboard And Full Regression

### Files

- Modify: `tools/cache_probe.py`
- Create: `tools/cache_probe.html`

### Why

The user needs a live view during experiments, while `report.json` remains the
complete machine-readable result for later evaluation.

### Change Necessity

Raw JSON alone is sufficient for another coding agent but inefficient for live
human monitoring. Add one dependency-free page and serve it from the existing
proxy; do not duplicate analysis in JavaScript.

### Impact And Compatibility

- page reads only `/report.json`;
- no remote assets, fonts, analytics, storage, or network calls;
- no nested cards or marketing layout;
- the table remains the primary operational surface;
- charts display report values without recalculation beyond screen scaling;
- controls and text must fit mobile and desktop widths.

### Required page structure

Create these stable element identifiers:

```text
run-status
summary-grid
sequence-filter
request-table
request-detail
cache-chart
tool-chart
last-updated
error-banner
```

The page must:

- poll `/report.json` every second with `cache: "no-store"`;
- retain the selected request when the report refreshes;
- filter rows by sequence without altering report data;
- show stable, changed, unknown, missing-usage, and spike states with text and
  color rather than color alone;
- render summary counters, request rows, changed paths, hashes, appended block
  counts, tool names, usage values, warnings, and request file path;
- draw cache-hit versus non-hit lines from per-request report fields;
- draw per-tool average and variance bars from `tool_summary`;
- escape all report text through DOM text nodes, never `innerHTML`;
- pause polling while the page is hidden and refresh immediately when visible;
- show a compact error banner without clearing the last valid report.

`ProbeHandler.do_GET` must serve:

```text
/             -> tools/cache_probe.html
/report.json  -> current atomic report snapshot
```

All other paths return `404`. The report response uses no-store cache headers.

Add the test-only `--self-test-server` option. It runs the deterministic
fixture population, then keeps the completed report and dashboard available
until interrupted normally. It must not contact the real upstream.

Extend `--self-test` to verify:

- the page and report endpoints return `200` and correct content types;
- all required element identifiers exist in the committed HTML;
- the HTML contains no external URL, script source, stylesheet link, or
  `innerHTML` assignment;
- the fixture-generated report contains at least two sequences, one stable
  request, one changed request, one unknown request, tool summary data, and
  merged usage;
- the page bytes contain no provider request content or credential.

The final self-test line becomes:

```text
cache probe self-test: ok
```

### Verification

Run:

```bash
python3 tools/cache_probe.py --self-test
```

Expected final line:

```text
cache probe self-test: ok
```

Run the persistent fixture view:

```bash
python3 tools/cache_probe.py --self-test-server --port 8787
```

Expected startup lines include:

```text
Dashboard: http://127.0.0.1:8787/
Report: <absolute target/cache-probe path>/report.json
```

Use the in-app browser or Playwright against:

```text
http://127.0.0.1:8787/
```

Capture and review screenshots at:

```text
1440x1000
390x844
```

Verify:

- the request table is visible without horizontal page overflow;
- summary text does not overlap;
- both canvases contain non-background pixels;
- selecting a row updates details without layout shift;
- changed and spike states remain understandable without relying only on color;
- no request body or credential appears in the page source.

Stop the fixture server normally after the browser checks.

Run:

```bash
rg -n "https?://|innerHTML|<script[^>]+src=|<link[^>]+stylesheet" tools/cache_probe.html
```

Expected: exit `1`, no matches.

Run:

```bash
git diff --check -- tools/cache_probe.py tools/cache_probe.html
```

Expected: exit `0`, no output.

Commit:

```bash
git add tools/cache_probe.py tools/cache_probe.html
git commit -m "feat(dev): display cache probe report"
```

## Final Verification

After all three task commits, run fresh evidence from the final tree:

```bash
python3 tools/cache_probe.py --self-test
```

Expected final line:

```text
cache probe self-test: ok
```

Run:

```bash
python3 tools/cache_probe.py --help
```

Expected: usage lists only the approved runtime settings and test-only fixture
settings.

Run:

```bash
rg -n "self-test-secret|x-api-key|authorization" target/cache-probe/self-test
```

Expected: exit `1`, no matches.

Run:

```bash
git diff --check
```

Expected: exit `0`, no output.

Run:

```bash
git log -3 --stat --oneline -- tools/cache_probe.py tools/cache_probe.html
```

Expected: the three task commits are visible and their maintained
implementation scope is limited to:

```text
tools/cache_probe.py
tools/cache_probe.html
```

Do not run Cargo tests: no Rust source, manifest, configuration, or Neo runtime
behavior changes.

## Live DeepSeek Check

The deterministic self-test is the completion evidence for the repository
implementation. A live provider run is a separate manual experiment:

1. start the probe with the real DeepSeek Anthropic-compatible base URL;
2. create a temporary Neo provider entry whose base URL is
   `http://127.0.0.1:8787`;
3. run one multi-turn tool-heavy session;
4. stop the probe normally;
5. retain the printed `report.json` path;
6. evaluate stable-prefix rate, changed paths, cache-hit slope, non-hit spikes,
   and per-tool attribution;
7. report exact model, request count, sequence count, and artifact path.

Do not commit credentials, temporary provider configuration, or generated run
artifacts. Do not present one live run as proof for other providers.

## Risks And Stop Conditions

- If the standard library buffers complete responses, stop and return to the
  approved design before adding a dependency.
- If DeepSeek emits undocumented usage shapes, forward them unchanged, record a
  warning, and leave derived fields missing.
- If request lineage is ambiguous, report unknown; do not add fuzzy matching.
- If the page needs analyzed values absent from `report.json`, add them to the
  script report owner rather than calculating them in JavaScript.
- If implementation requires Neo source edits, stop; that exceeds the approved
  scope.
- If request bodies or credentials appear in HTML, logs, events, or report
  summaries unintentionally, treat it as a blocking privacy failure.

## Retirement

- No old utility or compatibility path exists.
- No fallback proxy, alternate report shape, or provider abstraction is added.
- Self-test fixture support remains inside the script because it is the only
  deterministic proof for streaming and credential redaction.
- Generated run directories can be deleted independently without changing the
  repository.

## Plan Self-Review

- Spec coverage: every acceptance item maps to one task or final verification.
- Placeholder scan: no deferred implementation language remains.
- Type consistency: request, report, and page field ownership stays in the
  script.
- Compatibility: Neo code and normal provider behavior remain untouched.
- Change necessity: only the approved two-file utility is added.
- Existence check: the new surface has explicit user approval and no existing
  substitute.
- Architecture integrity: no duplicate analyzer or fallback path.
- Verification: exact self-test, security scan, browser sizes, and file-scope
  checks are specified.
- Retirement: no legacy path is retained or created.
- Residual risk: live DeepSeek behavior remains manual provider evidence after
  deterministic local completion.
