# Handoff Prompt: Implement DeepSeek Anthropic Cache Probe

Copy everything below the separator into the implementation task unchanged.

---

You are implementing the approved DeepSeek Anthropic-compatible cache probe in:

```text
/Users/chenyuanhao/Workspace/neo
```

The design and implementation plan are approved and closed. Execute the plan
completely and in order. Do not restart product discovery, repeat a
whole-repository survey, propose a different proxy architecture, or broaden the
tool to other providers.

## 1. Read Authority In This Order

1. `AGENTS.md`
2. `~/.codex/RTK.md`
3. `~/.codex/CX.md`
4. `docs/aegis/specs/2026-08-01-deepseek-cache-probe-design.md`
5. `docs/aegis/plans/2026-08-01-deepseek-cache-probe.md`
6. this handoff
7. `docs/aegis/specs/2026-07-08-prompt-cache-hit-rate-design.md`
8. only when a plan step needs confirmation:
   `crates/neo-ai/src/providers/anthropic.rs` and
   `crates/neo-agent-core/src/runtime/chat_request.rs`

Authority rules:

- the approved design owns behavior, scope, privacy, non-goals, and acceptance;
- the implementation plan owns task order, exact files, checks, and commit
  boundaries;
- this handoff owns execution discipline and final evidence;
- current source is evidence, not permission to weaken or expand the design;
- historical reports and `.references/` are not implementation authority;
- if the three authority documents disagree, stop and report the exact
  conflict instead of choosing a new direction.

Known planning commits:

```text
59f3d2a3 docs: design DeepSeek cache probe
14fd4cb7 docs: plan DeepSeek cache probe
```

The branch has advanced beyond these commits. Confirm they are ancestors and
read the files at current `HEAD`; do not reset or switch branches to those
commits.

Before edits, run:

```bash
icm recall-context "DeepSeek Anthropic cache probe local forwarding prefix stability report" --limit 5
git status --short
git log -8 --oneline --decorate
git merge-base --is-ancestor 14fd4cb7 HEAD
```

Expected ancestor check: exit `0`.

At handoff creation, these unrelated uncommitted paths existed:

```text
 M .gitignore
 M docs/aegis/INDEX.md
?? docs/aegis/plans/2026-08-01-workflow-ai-usability-repair.md
?? docs/aegis/specs/2026-08-01-workflow-ai-usability-repair-design.md
```

Re-check status because the shared worktree may have advanced. Every
pre-existing dirty or untracked path belongs to the user or another task.
Never revert, restore, stash, clean, overwrite, stage, or commit unrelated
paths.

Forbidden Git operations:

```text
reset
checkout --
restore
stash
clean
rebase
rm
commit --amend
force push
branch switching
worktree mutation
```

Do not push. Create exactly the three implementation commits required by the
plan, staging only the named tool files for each task.

## 2. Outcome And Stop Condition

Build one opt-in local test utility that observes the real request Neo sends to
DeepSeek's Anthropic-compatible Messages API, relays the real streamed response
back to Neo without buffering it to completion, and produces both a live web
view and a machine-readable report.

The maintained implementation is exactly:

```text
tools/cache_probe.py
tools/cache_probe.html
```

Generated evidence is exactly under:

```text
target/cache-probe/<run-id>/
├── events.jsonl
├── report.json
└── requests/
```

Stop only when:

- all three plan tasks are implemented and committed separately;
- the deterministic self-test passes from the final tree;
- credential scans are clean;
- desktop and mobile browser checks pass;
- only the two approved maintained files were added;
- final evidence distinguishes deterministic local proof from any optional
  live-provider observation.

Stop and ask for direction before changing the design if any task requires:

- a Neo source edit;
- a third maintained implementation file;
- a Python or JavaScript dependency;
- a database, package, daemon wrapper, or second analyzer;
- complete-response buffering;
- fuzzy request matching;
- support for another provider protocol;
- remote listening or telemetry.

## 3. Root Facts Already Established

Do not spend tokens rediscovering these facts:

1. Neo's Anthropic client posts to `<base_url>/messages`.
2. Authentication uses `x-api-key`.
3. The request body contains `model`, `system`, `tools`, `messages`, optional
   thinking settings, and `metadata.user_id` when available.
4. Streamed usage can be split between message-start and message-delta events.
5. Relevant usage fields are:

   ```text
   input_tokens
   output_tokens
   cache_read_input_tokens
   cache_creation_input_tokens
   ```

6. Child-agent identifiers are not serialized into this provider request, so
   global request order and `metadata.user_id` alone cannot safely identify a
   request predecessor.
7. The first-message anchor rule in the approved design resolves only an
   unambiguous existing sequence; ambiguity must remain unknown.
8. `target` is already ignored by Git.
9. Existing session JSONL and terminal usage totals cannot reconstruct the
   final provider body plus streamed provider result for each request.
10. A local forwarding proxy is therefore the smallest sufficient owner.

Use CodeGraph only if a named current-source fact has changed. Do not perform
another broad architecture review.

## 4. Closed Architecture

Keep this single path:

```text
Neo request
  -> local POST /messages
  -> persist request body
  -> analyze request lineage and historical prefix
  -> forward to configured DeepSeek upstream
  -> relay each response chunk immediately
  -> parse a side copy of streamed usage
  -> atomically rewrite report.json
  -> serve report and cache_probe.html
```

Ownership is fixed:

- `tools/cache_probe.py` owns command-line parsing, forwarding, persistence,
  lineage, comparison, attribution, usage merging, statistics, reports, web
  serving, and deterministic self-test;
- `tools/cache_probe.html` owns presentation only;
- `report.json` is the one complete analyzed result;
- `events.jsonl` is append-only evidence;
- the page must not independently decide prefix stability, merge usage, assign
  tools, calculate variance, or detect spikes.

Use Python `3.11+` standard library only. The approved implementation uses
`ThreadingHTTPServer`, `http.client`, atomic `os.replace`, plain HTML, plain
browser JavaScript, and native canvas.

## 5. Request Lineage And Prefix Rules

Apply this order exactly:

1. Filter predecessor candidates by route, model, and metadata user identifier.
2. Prefer candidates whose entire historical `messages` array is an exact
   prefix of the current `messages` array.
3. Choose the longest such prefix, then the newest request.
4. If no exact prefix exists, use the first message only as a conservative
   sequence anchor.
5. Compare as a historical mutation only when exactly one established sequence
   under the same identity has the same non-null first-message anchor.
6. If zero or multiple sequences match, create a new sequence and report the
   prefix as unknown.
7. Never treat the first message itself as a safe mutation anchor.
8. Never fall back to the previous global request, fuzzy similarity, rendered
   text, token estimates, or regular expressions.

For an exact predecessor with `N` messages:

1. truncate the current messages to the first `N` items;
2. canonicalize JSON objects by sorted keys;
3. preserve arrays and string bytes exactly;
4. compare the complete reduced body with the complete predecessor body;
5. include model, system, tools, messages, thinking, metadata, cache markers,
   and every other request field;
6. report stable only on complete equality;
7. otherwise report changed with the first changed path, bounded changed paths,
   and both hashes.

Stable means only that the observable Neo request prefix was preserved. It is
not proof of DeepSeek's internal cache choice.

## 6. Usage, Attribution, And Statistics Rules

Keep raw usage fields and derive only:

```text
cache_hit_tokens = cache_read_input_tokens
new_cache_tokens = cache_creation_input_tokens
non_hit_tokens = input_tokens + cache_creation_input_tokens
observed_input_tokens = input_tokens
                      + cache_read_input_tokens
                      + cache_creation_input_tokens
```

Missing fields remain null. Do not invent zero values.

The request increment is the current messages tail after the matched
predecessor length. Record appended messages, canonical byte count, block
types, user text, assistant text, thinking, tool-use identifiers, tool names,
and tool-result sizes.

Resolve tool results by typed tool-use identifiers already present in the
current history. An unresolved identifier stays unresolved. When one increment
contains multiple tools, list all of them and do not divide provider token
usage among them.

Calculate statistics within one matched sequence only. Emit a numeric spike
only after five earlier usable samples:

```text
current non_hit_tokens > previous mean + 3 * previous standard deviation
```

When previous standard deviation is zero, any value greater than the previous
mean is a spike. Structural change and numeric spike remain independent.

## 7. Forwarding And Security Rules

The proxy must:

- bind only to `127.0.0.1`;
- accept provider traffic only at `POST /messages`;
- reject missing, invalid, or over-limit content lengths before unbounded
  allocation;
- cap request bodies at `64 MiB`;
- forward the end-to-end method, path, JSON body, status, and safe headers;
- replace upstream host and content length;
- strip hop-by-hop headers;
- forward authentication in memory but never persist any request headers;
- never retry;
- read upstream response chunks at no more than `16_384` bytes;
- write and flush every non-empty chunk to Neo immediately;
- parse only a side copy of streamed bytes;
- keep forwarding when event parsing fails;
- record malformed events as warnings without fabricating usage;
- finalize reports on success, upstream failure, downstream disconnect, or
  malformed stream data;
- use downstream connection close when response length is unknown rather than
  buffering a complete body to invent a length.

Generated files, standard output, browser output, reports, and event logs must
not contain authentication values, authorization headers, cookies, or proxy
credentials.

Full request bodies are intentionally stored locally because exact comparison
is the tool's purpose. Print a startup warning that they may contain private
source code, prompts, and tool output.

## 8. Execute The Three Tasks In Order

Do not parallelize these tasks. They modify the same script sequentially.

### Task 1: Analysis And Artifacts

Read the complete Task 1 section in the plan, then create only:

```text
tools/cache_probe.py
```

Implement the named pure helpers, `RunStore`, canonical hashing, sequence
selection, prefix comparison, increment summaries, tool resolution, usage
merging, statistics, atomic report writes, and the analysis-only self-test.

Required check:

```bash
python3 tools/cache_probe.py --self-test
git diff --check -- tools/cache_probe.py
```

Expected final self-test line:

```text
cache probe self-test: analysis ok
```

Before committing, review the exact diff and confirm that no other path is
staged. Commit:

```bash
git add tools/cache_probe.py
git commit -m "feat(dev): add cache probe analysis"
```

### Task 2: Transparent Streaming Proxy

Read the complete Task 2 section in the plan, then modify only:

```text
tools/cache_probe.py
```

Add the threaded local server, upstream forwarding, immediate response flush,
incremental event parser, failure recording, startup output, and in-process
streaming fixture. The fixture must prove first-byte delivery before upstream
completion and credential forwarding without persistence.

Required checks:

```bash
python3 tools/cache_probe.py --self-test
rg -n "self-test-secret|x-api-key|authorization" target/cache-probe/self-test
git diff --check -- tools/cache_probe.py
```

Expected:

- self-test ends with `cache probe self-test: proxy ok`;
- credential scan exits `1` with no matches;
- diff check exits `0` with no output.

Commit only the script:

```bash
git add tools/cache_probe.py
git commit -m "feat(dev): proxy DeepSeek cache traffic"
```

### Task 3: Dashboard And Full Regression

Read the complete Task 3 section in the plan, then modify only:

```text
tools/cache_probe.py
tools/cache_probe.html
```

Serve `/` and `/report.json`, add the test-only fixture server mode, and build
the operational dashboard. The page polls the report, keeps selection across
refreshes, filters by sequence, renders request details, and draws both native
canvas charts. Use DOM text nodes for report content; never use `innerHTML`.

Required checks:

```bash
python3 tools/cache_probe.py --self-test
python3 tools/cache_probe.py --self-test-server --port 8787
```

Expected final self-test line:

```text
cache probe self-test: ok
```

While the fixture server runs, use the available browser automation skill or
Playwright at:

```text
http://127.0.0.1:8787/
```

Capture and review:

```text
1440x1000
390x844
```

Verify no overlap, no page-level horizontal overflow, stable row selection,
readable status without color alone, and non-background pixels in both
canvases. Stop the server normally after the checks.

Then run:

```bash
rg -n "https?://|innerHTML|<script[^>]+src=|<link[^>]+stylesheet" tools/cache_probe.html
git diff --check -- tools/cache_probe.py tools/cache_probe.html
```

Expected scans: no matches; diff check: exit `0`.

Commit only the two tool files:

```bash
git add tools/cache_probe.py tools/cache_probe.html
git commit -m "feat(dev): display cache probe report"
```

## 9. Required Report And Page Surface

`report.json` must contain these top-level fields in deterministic order:

```text
run
summary
sequences
requests
tool_summary
warnings
```

Each request must expose sequence and predecessor identifiers, timestamps,
duration, model, metadata user identifier, request path, hashes, prefix status,
changed paths, increment summary, tool attribution, raw and derived usage,
variance, spike state, and forwarding result.

The page must contain these stable identifiers:

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

The page has no external assets, fonts, analytics, storage, dependencies, or
secondary data owner. It must pause polling while hidden and refresh when
visible. A report fetch failure must keep the last valid report visible.

## 10. Final Verification

After the third commit, run fresh checks from the final tree:

```bash
python3 tools/cache_probe.py --self-test
python3 tools/cache_probe.py --help
rg -n "self-test-secret|x-api-key|authorization" target/cache-probe/self-test
git diff --check
git status --short
git log -3 --stat --oneline -- tools/cache_probe.py tools/cache_probe.html
```

Required conclusions:

- final self-test ends with `cache probe self-test: ok`;
- help lists only approved runtime settings and test-only fixture settings;
- credential scan exits `1` with no matches;
- diff check exits `0`;
- the three tool commits are present;
- maintained implementation scope is exactly the two tool files;
- pre-existing unrelated worktree changes remain untouched and uncommitted by
  this task.

Do not run Cargo tests. No Rust source, Cargo manifest, Neo runtime,
configuration format, or session format is changed.

## 11. Optional Live DeepSeek Experiment

A live provider experiment is not required for deterministic repository
completion. Run it only when the user explicitly authorizes use of their real
provider and credentials.

For an authorized live run:

1. start the proxy with the real DeepSeek Anthropic-compatible base URL;
2. point a temporary Neo provider entry at `http://127.0.0.1:8787`;
3. run one multi-turn, tool-heavy session;
4. stop the proxy normally;
5. preserve the printed `report.json` path;
6. report the exact model, request count, sequence count, stable-prefix rate,
   changed paths, cache-hit trend, non-hit spikes, and tool attribution;
7. never commit credentials, temporary provider configuration, or generated
   run artifacts.

One live run is provider evidence for that exact run only. Do not claim it
proves other providers, models, endpoints, operating systems, or remote CI.

## 12. Drift And Failure Rules

After each task, answer:

- Did only the planned files change?
- Did one script remain the analysis owner?
- Did the page remain presentation-only?
- Did streaming remain immediate?
- Did any credential reach disk or browser output?
- Did ambiguous lineage remain unknown?
- Did a dependency, fallback, alias, provider abstraction, or second path
  appear?
- Does fresh evidence support the next task?

If any answer is wrong, stop and repair the current task before continuing.

If a self-test fails, diagnose that focused failure. Do not weaken assertions,
delete required coverage, broaden to Cargo tests, or modify unrelated files to
make it pass.

If standard-library streaming is proven inadequate, stop and return the exact
evidence to the user. Do not silently add a dependency or buffer full
responses.

If DeepSeek sends unknown usage fields, forward them unchanged, record a
warning, leave unsupported derived values null, and report the residual risk.

## 13. Final Response

Lead with the implemented outcome. Include:

- the three commit hashes and messages;
- exact maintained files;
- self-test result;
- credential-scan result;
- browser viewport results and screenshot paths;
- whether any live provider run was performed;
- the final deterministic report path;
- pre-existing dirty paths preserved;
- residual risk, especially live-provider and cross-platform coverage.

Do not claim remote CI, live DeepSeek behavior, Windows behavior, or Linux
behavior unless those exact checks were run. Do not push.
