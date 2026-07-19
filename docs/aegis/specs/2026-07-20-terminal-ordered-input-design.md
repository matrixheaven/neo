# Terminal Ordered Input Sequence

## Status

Direction approved on 2026-07-20 after inspection of the failing Neo session.
Written review is required before implementation planning.

This design supersedes only the string-shaped `Terminal` write-input contract
and control-byte examples in
`2026-07-19-terminal-bounded-yield-design.md`. Raw PTY output, bounded yield,
admission, command lifetime, guardian ownership, and process cleanup remain
unchanged.

## Evidence

The latest Neo session for the workspace was
`session_5961d31e-39f2-464e-bfd8-a8f3c6b25ddd`.

The session established an interactive SSH connection with:

```json
{"command":"ssh root@10.211.55.18","mode":"start","yield_time_ms":3000}
```

Normal commands, prompts, and Vim worked through the remote PTY. The model then
started `sleep 60` and attempted Ctrl+C with raw tool arguments equivalent to:

```json
{"mode":"write","handle":"...","input":"\\u0003"}
```

After JSON parsing, `ToolExecutionStarted.arguments.input` was the six printable
characters `\u0003`, not the single U+0003 control character. Terminal correctly
preserved and wrote those characters. Existing Unix real-PTY coverage proves
that an actual byte `0x03` interrupts an interruptible foreground command and
keeps the session usable.

The causal chain is therefore:

```text
ambiguous string-only input contract
  -> model emits a double-escaped printable sequence
  -> JSON parser correctly preserves one literal backslash
  -> Terminal correctly writes six printable characters
  -> remote sleep receives no Ctrl+C byte
```

Fedora, SSH PTY allocation, guardian framing, and the PTY writer are not the
failure owners.

## Decision

Keep `mode=write`, but replace its scalar string `input` with one ordered array
of typed input parts. One tool call can mix ordinary text and exact control
bytes without textual escape interpretation.

Canonical shape:

```json
{
  "mode": "write",
  "handle": "terminal-handle",
  "input": [
    {"text": "ordinary text"},
    {"control": 3},
    {"text": "more text"}
  ],
  "yield_time_ms": 1000
}
```

Each array element contains exactly one field:

| Part | Value | Semantics |
| --- | --- | --- |
| `text` | UTF-8 string | Preserve existing text input behavior. Normalize LF and CRLF to CR before writing. Other characters remain literal. |
| `control` | integer | Append one exact C0 control byte `0..=31` or DEL `127` without text escape parsing or newline normalization. |

Common values are Ctrl+C `3`, Ctrl+D `4`, Tab `9`, Enter `13`, Ctrl+Z `26`,
Escape `27`, and Delete `127`.

The array must be non-empty. Parts are concatenated in array order and passed
once through the existing `GuardianClient::write_terminal` path. Existing
guardian chunking may split a large payload but must preserve byte order.

## First-Principles Boundary

The irreducible outcome is exact ordered terminal input in one tool call. The
non-negotiable constraints are:

- no additional LLM round trip for known mixed text/control input;
- no inference from printable escape syntax;
- literal text remains literal;
- nested PTY and SSH input remains byte-oriented;
- no change to admission, command lifetime, output yield, or handle ownership;
- one canonical write-input contract with no compatibility branch.

The design drops the assumption that Ctrl+C should be modeled as an operating
system signal. In an interactive SSH session, a local process-group signal may
terminate the local `ssh` client rather than deliver a terminal key to the
remote foreground process. Ordered PTY input is the correct owner for keyboard
semantics.

## Rejected Alternatives

### Separate `interrupt` or `signal` mode

Rejected as the primary repair. It does not solve mixed text/control input in
one call, introduces separate process-control semantics, and can target the
wrong process boundary for nested SSH. A signal operation requires independent
future evidence and design.

### Parse `\uXXXX`, `\xNN`, or caret notation inside text

Rejected. It makes raw text ambiguous, prevents exact entry of those printable
sequences, adds an escape language, and fixes the observed sample rather than
the input-representation class.

### Base64 or a raw byte-array-only input

Rejected as the ordinary interface. It is exact but expensive for models,
opaque in tool-call presentation, and unnecessarily replaces readable text.

### Keep scalar `input` plus add a second structured field

Rejected. Two active input owners would force precedence and exclusivity rules
and preserve the ambiguous path that caused the failure.

## Product Contract

`input` is required only for `mode=write` and is invalid for other modes. Its
type is a non-empty ordered list of typed parts. The old scalar string form is
invalid input.

Validation fails before guardian I/O when:

- `input` is absent or empty for `mode=write`;
- a part contains neither or both of `text` and `control`;
- `control` is outside `0..=31` and is not `127`;
- `input` appears in a non-write mode.

One successful call returns the existing single write result and one bounded
post-write observation. It does not yield once per part and does not create
multiple guardian requests solely because the payload contains multiple parts.

The tool schema and English/Chinese documentation must explicitly show a mixed
text/control example. The TUI/tool-call presentation continues to display the
structured arguments so AI-issued control input remains inspectable.

## SSH and Signal Semantics

Control parts guarantee exact PTY input bytes, not a portable process signal.
The receiving terminal stack remains authoritative:

- an interactive local or remote PTY commonly maps byte `3` to VINTR/SIGINT;
- an SSH command without a remote PTY may deliver byte `3` to process stdin,
  which a program such as `sleep` does not read;
- interactive remote control should allocate a remote PTY, for example with
  `ssh -tt` when automatic allocation is uncertain;
- Windows ConPTY behavior remains program- and console-mode-dependent.

Neo must not silently replace a control part with local process-group signaling.

## Architecture

The existing ownership remains:

```text
TerminalTool input validation and byte assembly
  -> GuardianClient raw write/chunking
  -> guardian raw frame payload
  -> PTY writer
```

Only `TerminalTool` gains typed input-part parsing and ordered byte assembly.
The guardian protocol already transports arbitrary `Vec<u8>` and requires no
new frame, request kind, signal adapter, or platform branch.

No new module is required unless the maintained `terminal.rs` complexity check
shows the input-part encoder cannot remain a small local owner.

## Compatibility and Retirement

This is a hard contract replacement:

- retire scalar string `input`;
- do not accept both strings and arrays through an untagged compatibility enum;
- do not add `input_v2`, `sequence`, `keys`, `interrupt`, or `signal` aliases;
- remove schema/docs claims that a model should express controls through JSON
  string escapes;
- update existing Terminal tests and examples to the ordered array.

Historical tool calls remain transcript data and are not replayed, so no
session migration is required. The model receives the current tool schema on
subsequent requests.

## Verification

Use focused existing targets and tests that prove the contract rather than
Serde or container round trips:

1. One encoder test proves ordered mixed input produces the exact bytes for
   text, newline normalization, C0 controls, Escape, and DEL.
2. Printable text resembling `\u0003` remains printable text when supplied in
   a `text` part.
3. Empty input, malformed parts, and invalid control values fail before
   guardian I/O.
4. The existing Unix real-PTY Ctrl+C test uses `{"control":3}`, interrupts the
   command, and leaves the session usable.
5. One real-PTY test sends text followed by Ctrl+D in a single write call and
   proves ordering without a second write.
6. The existing incremental write/yield test still returns one post-write
   observation and advances one shared offset.
7. Windows coverage proves the input contract and session usability without
   claiming portable Ctrl+C signal behavior.
8. Generated tool schema inspection confirms one canonical array input and no
   scalar-string compatibility branch.
9. English and Chinese docs contain the same mixed-input and SSH/PTTY boundary.

Entity verification remains required on macOS, Fedora Linux, and Windows 11
ConPTY because the public Terminal contract is cross-platform.

## Scope and Governance

Task intent:

- Outcome: let models express exact ordered text and control input in one
  Terminal call without double escaping or added LLM round trips.
- Success evidence: the focused contract and real-PTY tests pass on the three
  supported platforms.
- Stop condition: scalar input is retired, ordered input is canonical, docs are
  synchronized, and no signal path or escape parser is added.

Baseline alignment:

- Product / requirement: the previous claim that raw JSON strings were an
  adequate AI-facing control-input contract is a Design Defect exposed by the
  recorded session.
- Architecture / runtime: aligned; `TerminalTool` remains the input owner and
  the guardian remains a raw byte transport.
- Result: Design Defect, scope requirements. No runtime-owner drift was found.

Impact statement:

- Affected surfaces: Terminal input schema, local byte assembly, focused tests,
  and English/Chinese tool documentation.
- Unchanged surfaces: guardian protocol, PTY backend, scheduler, command
  lifetime, bounded yield, output offsets, Bash, runtime dispatch, and TUI
  rendering.
- Architecture review required: yes, because the public tool input contract is
  replaced.
- ADR signal: no separate ADR unless implementation introduces process-signal
  semantics or a new owner beyond `TerminalTool`.

Minimality decision:

- Reuse the existing write path and raw byte protocol.
- Add no operation, process-control owner, parser language, fallback, or
  compatibility carrier.
- Prefer one local encoder plus the smallest tests that fail if byte order or
  contract shape regresses.
