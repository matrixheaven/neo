# /btw Sidecar Dialog

The `/btw` sidecar dialog lets you ask temporary side questions without polluting the current turn or the main session transcript.

## Opening the sidecar

- `/btw` opens an empty sidecar panel.
- `/btw <question>` opens the sidecar and immediately sends the question.

The panel appears between the transcript and the composer. Its title shows `BTW ─ Esc close`, or `BTW ─ Esc close · ↑↓ scroll` when the content overflows.

## Composer behavior while the sidecar is open

While the sidecar is focused, the bottom composer becomes the side-question composer:

- **Enter** sends the side question.
- **Shift+Enter**, **Alt+Enter**, and **Ctrl+J** insert a newline.
- **Esc** clears the side-composer text if it is non-empty; if the composer is empty, Esc closes the panel.
- **Up/Down** scroll the panel when the composer is empty; otherwise they move the caret.

## What the sidecar can and cannot do

The sidecar inherits a projected snapshot of the active session's messages. Incomplete trailing tool turns are trimmed, and a system reminder is appended that tool calls are disabled.

- Tool calls are rejected with: `Tool calls are disabled for side questions. Answer with text only.`
- The sidecar does **not** write to the main session JSONL.
- The sidecar does **not** append to the main transcript.
- Closing the sidecar does **not** resume or fork the main session.
- Sending a new side question while one is still running cancels the previous request and replaces it.
