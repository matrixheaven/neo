# WebUI loading performance and loading screen - Intent

## TaskIntentDraft

- Requested outcome: Stop persisted history bootstrap from overflowing the live relay queue and show a centered Neo loading screen while the workspace or selected session snapshot is loading.
- Goal: Stop persisted history bootstrap from overflowing the live relay queue and show a centered Neo loading screen while the workspace or selected session snapshot is loading.
- Success evidence:
- A focused backend regression proves bootstrap history is not published live; focused frontend tests prove the full-screen loading state; browser checks show the centered animated mark on desktop and mobile.
- Stop condition: Stop when focused backend and frontend checks pass, browser loading state is visually verified, and only task-owned hunks are staged.
- Non-goals:
- History pagination, transcript virtualization, session metadata indexing, or unrelated Review panel work.
- Scope: crates/neo-agent/src/modes/webui, crates/neo-webui/web/src/app.tsx, loading styles, focused tests
- Change kinds:
- bugfix and bounded UI state change
- Risk hints:
- relay sequence continuity, snapshot watermark, reduced-motion behavior, dirty overlapping frontend files

## BaselineReadSetHint

- docs/aegis/specs/2026-08-09-neo-webui-design.md

## BaselineUsageDraft

- Required baseline refs:
- docs/aegis/specs/2026-08-09-neo-webui-design.md
- Acknowledged before plan:
- docs/aegis/specs/2026-08-09-neo-webui-design.md
- Cited in plan:
- docs/aegis/specs/2026-08-09-neo-webui-design.md
- Missing refs:
- none
- Advisory decision: continue

## ImpactStatementDraft

- Compatibility boundary: Existing session snapshots and live event ordering remain unchanged after bootstrap.
- Affected layers:
- WebSessionHost persisted projection, relay sequence initialization, WebUI loading presentation
- Owners:
- WebSessionHost for history restore; App for loading presentation
- Invariants:
- Canonical JSONL remains append-only; historical restore never republishes old events as live traffic; loading UI never cancels or changes a turn.
- Non-goals:
- History pagination, transcript virtualization, session metadata indexing, or unrelated Review panel work.

These records are Method Pack drafts / hints, not authoritative runtime decisions.

## BaselineUsageDraft

- Required baseline refs:
- docs/aegis/specs/2026-08-09-neo-webui-design.md
- Delivered context refs:
- none
- Acknowledged before plan:
- docs/aegis/specs/2026-08-09-neo-webui-design.md
- Cited in plan:
- docs/aegis/specs/2026-08-09-neo-webui-design.md
- Missing refs:
- none
- Advisory decision: continue
