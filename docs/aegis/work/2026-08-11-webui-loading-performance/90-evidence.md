# WebUI loading performance and loading screen - Evidence

- Backend: the exact `fresh_projection_bootstrap_does_not_publish_history_to_observer` regression passed. History above the observer queue limit was restored only through the snapshot, and the next live sequence remained contiguous.
- Frontend: `npm test -- test/unit/session.test.tsx -t "loading page"` passed 3 focused cases.
- Build: `npm run build` completed successfully.
- Browser: the loading gate filled `1440x900` and `390x844` viewports, stayed centered without overflow, used the `loading-breathe` animation at `1.6s`, and reported no page errors.
- Scope limit: verification used the local mock server, not every native browser or a production-sized JSONL replay through the full binary.

## EvidenceBundleDraft

- Artifact key: backend-regression
- Type: test
- Source: cargo test --package neo-agent --bin neo -- modes::webui::session::projection::fresh_projection_bootstrap_does_not_publish_history_to_observer --exact --nocapture --include-ignored
- Summary: One exact backend regression passed; bootstrap history over the live queue limit stayed out of the observer queue and the next live sequence remained contiguous.
- Verifier: cargo test

## EvidenceBundleDraft

- Artifact key: frontend-regression
- Type: test
- Source: npm test -- test/unit/session.test.tsx -t loading page
- Summary: Three focused loading-page tests passed for initial retry and unloaded-session switching.
- Verifier: vitest

## EvidenceBundleDraft

- Artifact key: web-build
- Type: build
- Source: npm run build
- Summary: Production WebUI build completed successfully.
- Verifier: vite

## EvidenceBundleDraft

- Artifact key: browser-check
- Type: visual
- Source: agent-browser desktop 1440x900 and mobile 390x844
- Summary: Loading gate filled both viewports, stayed centered without overflow, used loading-breathe at 1.6s, and reported no browser errors.
- Verifier: agent-browser
