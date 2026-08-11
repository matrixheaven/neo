# WebUI loading performance and loading screen - Reflection

- Root cause fixed at bootstrap: persisted events reserve their sequence range and build one snapshot projection instead of entering the live observer queue.
- The loading UI reuses the existing Neo mark and existing connection state; no new state owner, dependency, or compatibility path was added.
- Existing reconnect behavior remains visible over an already loaded session.
- Residual risk is limited to browser and full-process coverage not exercised by the focused local checks.

Method Pack output does not grant completion authority.
