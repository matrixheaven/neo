# Neo WebUI Composer Completion - Intent

## TaskIntentDraft

- Requested outcome: Add slash command and workspace file completion with transcript-aware popup placement
- Goal: Add slash command and workspace file completion with transcript-aware popup placement
- Success evidence:
- Focused Rust and WebUI tests pass; browser confirms below placement on empty transcript and above placement after transcript
- Stop condition: Done after focused verification and scoped commit; otherwise stop on blocker, missing verification, or scope expansion
- Non-goals:
- Execute TUI-only slash dialogs in the browser
- Scope: Existing completion owner, WebUI typed query, composer listbox, focused tests and embedded assets
- Change kinds:
- feature
- Risk hints:
- Do not duplicate completion discovery or expose absolute paths; preserve dirty WebUI edits

## BaselineReadSetHint

- docs/aegis/specs/2026-08-09-neo-webui-design.md
- docs/aegis/plans/2026-08-11-webui-composer-completion.md

## BaselineUsageDraft

- Required baseline refs:
- docs/aegis/specs/2026-08-09-neo-webui-design.md
- docs/aegis/plans/2026-08-11-webui-composer-completion.md
- Acknowledged before plan:
- none
- Cited in plan:
- none
- Missing refs:
- docs/aegis/specs/2026-08-09-neo-webui-design.md
- docs/aegis/plans/2026-08-11-webui-composer-completion.md
- Advisory decision: needs-baseline-readback

## ImpactStatementDraft

- Compatibility boundary: Preserve transcript and session behavior plus pre-existing dirty changes
- Affected layers:
- neo-agent
- neo-webui
- web
- Owners:
- prompt_completion and WebSessionHost
- Invariants:
- Browser receives only structured relative candidates
- Non-goals:
- Execute TUI-only slash dialogs in the browser

These records are Method Pack drafts / hints, not authoritative runtime decisions.
