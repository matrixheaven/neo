# Delegate Tool Activity Summary And Theme - Intent

## TaskIntentDraft

- Requested outcome: Show representative Edit/Write file data in collapsed DelegateSwarm rows and restore semantic theme colors across Delegate-family tool activity
- Goal: Show representative Edit/Write file data in collapsed DelegateSwarm rows and restore semantic theme colors across Delegate-family tool activity
- Success evidence:
- Exact TUI regression proves text and custom-theme spans for Delegate and collapsed/expanded DelegateSwarm
- Stop condition: Done when planned TUI files pass exact verification; stop on core contract, card-layout, or theme-schema expansion
- Non-goals:
- All-path collapsed display or new theme/config surface
- Scope: child_activity.rs, swarm_card.rs, multi_agent_transcript.rs, scoped Aegis records
- Change kinds:
- ui-fix
- Risk hints:
- Misleading aggregate stats or duplicate expanded paths

## BaselineReadSetHint

- docs/aegis/specs/2026-07-25-delegate-tool-activity-summary-theme-brief.md

## BaselineUsageDraft

- Required baseline refs:
- docs/aegis/specs/2026-07-25-delegate-tool-activity-summary-theme-brief.md
- docs/aegis/specs/2026-07-24-delegate-edit-write-file-activity-brief.md
- Acknowledged before plan:
- docs/aegis/specs/2026-07-25-delegate-tool-activity-summary-theme-brief.md
- docs/aegis/specs/2026-07-24-delegate-edit-write-file-activity-brief.md
- Cited in plan:
- docs/aegis/plans/2026-07-25-delegate-tool-activity-summary-theme.md
- Missing refs:
- none
- Advisory decision: continue

## ImpactStatementDraft

- Compatibility boundary: No core/runtime/schema/event/card-layout changes
- Affected layers:
- neo-tui transcript presentation
- Owners:
- crates/neo-tui/src/transcript/child_activity.rs
- Invariants:
- One semantic activity renderer; collapsed Swarm remains one line
- Non-goals:
- All-path collapsed display or new theme/config surface

These records are Method Pack drafts / hints, not authoritative runtime decisions.

## BaselineUsageDraft

- Required baseline refs:
- docs/aegis/specs/2026-07-25-delegate-tool-activity-summary-theme-brief.md
- docs/aegis/specs/2026-07-24-delegate-edit-write-file-activity-brief.md
- Delivered context refs:
- none
- Acknowledged before plan:
- docs/aegis/specs/2026-07-25-delegate-tool-activity-summary-theme-brief.md
- docs/aegis/specs/2026-07-24-delegate-edit-write-file-activity-brief.md
- Cited in plan:
- docs/aegis/plans/2026-07-25-delegate-tool-activity-summary-theme.md
- Missing refs:
- none
- Advisory decision: continue
