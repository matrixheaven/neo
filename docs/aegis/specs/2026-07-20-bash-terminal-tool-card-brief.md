# Bash and Terminal Tool Call Presentation Spec Brief

Status: Approved
Date: 2026-07-20

## Goal

Make every model-issued Bash command and Terminal operation meaningfully
inspectable in Neo's transcript without allowing long arguments to exceed the
terminal width or silently hiding the dangerous end of a command.

## Problem

The generic tool header currently treats a shell command as a key argument.
Non-path arguments are shortened before the header is rendered, and the final
header is constrained to one terminal row. Users therefore often see only the
beginning of a Bash or Terminal command.

Delegate-family cards receive a separately bounded child tool summary. They
must remain compact and must not gain nested shell cards or duplicate complete
child tool arguments in parent progress state.

## Decisions

1. `ToolCallComponent` remains the canonical owner of tool status, lifecycle,
   queue metadata, result data, replay, and global `Ctrl+O` expansion state.
2. Bash and Terminal receive a dedicated render-only presentation path. No new
   tool-card state type, runtime event, dependency, or persistence field is
   introduced.
3. The header contains status and compact metadata. Command text moves into a
   separate body region and is never placed back in the header.
4. Shell source uses Neo's existing syntect Bash grammar and theme conversion.
   Highlighting is optional presentation: failure falls back to plain text and
   cannot change the displayed characters.
5. Delegate, DelegateGroup, and DelegateSwarm retain their existing card
   chrome, ordering, progress, row budgets, output previews, and expansion
   behavior. Only bounded Bash/Terminal summary elision changes from
   prefix-only to head-and-tail within the existing character budget.

## Common Visual Language

The presentation stays unframed and uses the existing transcript vocabulary:

- `● Preparing`, `● Queued`, `● Using`, and `● Used` retain their existing
  status-specific colors.
- `✗ Failed` and `⊘ Cancelled` retain their existing symbols and colors.
- `$` uses `theme.shell_mode`.
- Commands use Bash syntax highlighting. Working-directory labels, overflow
  hints, handles, queue positions, and secondary metadata use muted text.
- Output keeps the existing tool-result rendering. Arbitrary stdout/stderr is
  not syntax-highlighted because its language is unknown and it may already
  carry meaningful result styling.
- Status remains understandable without color.

## Bash Presentation

### Preparing and queued

```text
● Preparing Bash
  $ cargo tes▌

● Queued Bash · #2 · waiting 18s
  cwd crates/neo-tui
  $ cargo test --package neo-tui --test tool_cards
    -- tool_call_renders_running_header --exact --nocapture
```

Partial streamed arguments may be highlighted when a command can be extracted.
Incomplete JSON, an incomplete quote, or a syntax-highlighting error must fall
back to width-safe plain text without hiding the partial command.

### Running and successful

```text
● Using Bash
  cwd crates/neo-tui
  $ cargo test --package neo-tui --test tool_cards
    -- tool_call_renders_running_header --exact --nocapture
  Compiling neo-tui v0.1.1
  Running tests/tool_cards.rs

● Used Bash · 14 lines
  $ cargo test --package neo-tui --test tool_cards
    -- tool_call_renders_running_header --exact --nocapture
  test tool_call_renders_running_header ... ok
  test result: ok. 1 passed; 0 failed
```

The command region is stable across Preparing, Queued, Running, and terminal
states. Live or final output appears after it and continues to use the existing
output limits and global expansion behavior.

### Failure, cancellation, timeout, and background execution

```text
✗ Failed Bash · 8 lines
  $ cargo test --package neo-tui --test tool_cards
  error[E0308]: mismatched types
  command exited with code 101

⊘ Cancelled Bash
  $ cargo nextest run -p neo-tui
  Cancelled.

✗ Failed Bash
  $ cargo nextest run -p neo-tui
  Timed out after 30m. Increase timeout_secs or omit it.

● Used Bash · background
  $ npm run dev
  task task_42 · Running development server
```

Existing failure reconstruction remains authoritative when stdout/stderr are
empty. This feature must not turn timeout, cancellation, resource-limit, or
background results into blank successful-looking cards.

## Command Layout

### Wrapping

- Preserve logical newlines and shell text exactly after display sanitization.
- Prefix only the first logical command row with `$ `.
- Indent source continuations and width-generated continuations by four spaces.
- Wrap styled spans by visible terminal width. Long tokens with no whitespace
  must hard-wrap rather than exceed the width invariant.
- Recompute wrapping when terminal width changes; never rewrite stored tool
  arguments.

### Collapsed preview

After highlighting and width-safe wrapping:

- Four or fewer visual command rows are shown completely.
- More than four rows show the first three rows, one explicit omission row, and
  the final row.
- The omission row states the number of hidden command characters and includes
  `ctrl+o to expand`.
- The final row is retained so suffixes such as output paths, chained commands,
  destructive targets, and `--exact --nocapture` remain visible.

```text
● Using Bash
  $ cargo nextest run --package neo-agent-core --test runtime_turn
    --features integration,providers --no-fail-fast
    ... 214 characters hidden · ctrl+o to expand
    runtime::tests::exact_case --exact --nocapture && git status
```

Expanded mode renders the complete command with the same wrapping and syntax
highlighting. The existing global `Ctrl+O` state expands both the command and
tool output; no shell-only expansion key or state is added.

### Multiline commands

```text
● Using Bash
  $ for crate in neo-ai neo-agent-core neo-tui; do
      cargo test --package "$crate" --lib
    done
```

```text
● Used Bash
  $ cat <<'EOF' > report.txt
    first line
    second line
    EOF
  wrote report.txt
```

Highlighter state may span logical lines, but no rendered line may contain an
embedded newline character.

## Syntax Highlighting

The render-only shell path reuses `highlight_code_lines` with a synthetic
`.sh` path so `lang_from_path` selects the existing Bash grammar. It reuses the
current syntect theme-to-`Span` conversion and highlight cache.

The highlighter must receive display-sanitized command text. ANSI escapes and
non-printing control bytes are rendered visibly or removed according to the
existing transcript sanitization policy; they must never control the user's
terminal. Highlighting and wrapping operate on a display projection only. The
runtime command, tool arguments, replay data, and model-visible result remain
unchanged.

## Working Directory

When a typed `cwd` argument is present, render its workspace-relative display
form on a muted row before the command. Do not parse `cd` from command text and
do not invent a cwd when the typed field is absent.

```text
● Using Bash
  cwd crates/neo-tui
  $ cargo test --package neo-tui --lib
```

## Terminal Presentation

Terminal keeps one normal tool card per operation. It does not gain a separate
session card or aggregate all operations by handle.

### `start`

```text
● Using Terminal · start
  cwd crates/neo-tui
  $ cargo nextest run --package neo-tui --test tool_cards

● Used Terminal · start · term_7
  cwd crates/neo-tui
  $ cargo nextest run --package neo-tui --test tool_cards
  Terminal started.
```

`start.command` uses the same Bash highlighting, wrapping, preview, cwd, and
sanitization rules as Bash.

### `write`

```text
● Used Terminal · write · term_7
  stdin › q\r
```

Input is not parsed as a shell command. Newlines, carriage returns, escape,
control-C, and other control bytes use an escaped visible form so the user can
audit what was sent without executing terminal control sequences.

### `read`, `resize`, and `stop`

```text
● Used Terminal · read · term_7
  test result: ok. 35 passed
  neo@workspace %

● Used Terminal · resize · term_7
  size 120 × 40

● Used Terminal · stop · term_7
  Process tree stopped.
```

`read` uses the existing output preview and expansion path. `resize` shows the
requested dimensions. `stop` shows the existing terminal result. Missing
structured metadata is omitted rather than reconstructed from human-readable
result text.

## Delegate-Family Boundary

Delegate, DelegateGroup, and DelegateSwarm keep their current card structure
and behavior exactly. The only authorized text-level change is the bounded
shell-summary elision described below:

- no nested Bash/Terminal cards;
- no extra child rows or wrapped command bodies;
- no changes to progress, ordering, visible tool-row count, output preview, or
  expansion semantics;
- no complete child arguments copied into parent snapshots or progress events.

Within the existing 96-character shell summary budget, Bash/Terminal summaries
retain both the beginning and end with a middle ellipsis:

```text
  • Using Bash (cargo nextest run -p neo-agent-core … --exact --nocapture)
  • Used Read (crates/neo-tui/src/transcript/tool_call.rs)
```

Swarm child summaries use the same bounded shell projection:

```text
  nova · running · waiting on Bash
    cargo nextest run -p neo-tui … --exact --nocapture
```

Nested summaries remain plain, single-line text. Syntax highlighting is limited
to top-level Bash and Terminal command bodies.

## Ownership and File Boundary

- `ToolCallComponent` remains the stateful card owner and routes Bash/Terminal
  body presentation to a pure render helper.
- The shell presentation helper owns structured argument extraction, display
  sanitization, syntax highlighting, wrapping, collapsed command selection,
  and Terminal mode-specific rows.
- The helper should live in a focused transcript module rather than adding a
  new responsibility to the already large generic `tool_renderers.rs`.
- Multi-agent runtime remains the owner of bounded child activity summaries.
  It may make summary truncation shell-aware but must not expand the snapshot
  schema or persisted progress payload.

This boundary adds one render module, not a second card architecture.

## Replay and Resize

- Persist raw existing tool arguments and results only; never persist ANSI or
  highlighted spans.
- Replay reconstructs the same command body from existing arguments.
- Old sessions with missing or partial arguments retain the generic fallback.
- Terminal resize reflows command rows from the original display text.
- The final line-width invariant remains enabled and must not be weakened to
  accommodate shell content.

## Non-Goals

- Changing Bash or Terminal schemas, execution, admission, timeout, guardian,
  background-task, permission, or approval behavior.
- Adding a shell parser, cargo-specific parser, custom syntax theme, or new
  highlighting dependency.
- Syntax-highlighting arbitrary command output.
- Grouping consecutive Bash or Terminal calls into generic tool groups.
- Adding a Terminal session card, command-copy shortcut, horizontal scrolling,
  or shell-only expansion mode.
- Persisting full child tool arguments in Delegate-family progress state.
- Redesigning Delegate, DelegateGroup, or DelegateSwarm cards.

## Acceptance

1. A normal Bash command is fully visible in collapsed mode and is no longer
   silently shortened to the generic key-argument limit.
2. Bash commands and Terminal `start.command` use existing Bash syntax
   highlighting with character-identical plain-text fallback.
3. Long commands show their beginning, explicit omission, and final row; global
   `Ctrl+O` reveals the complete command.
4. Multiline commands, heredocs, quoted strings, variables, operators, Unicode,
   and unbroken long tokens remain within terminal width.
5. Explicit typed cwd appears separately; command text is never inspected to
   infer cwd.
6. Preparing, queued, running, successful, failed, cancelled, timed-out,
   resource-limited, and background Bash states retain meaningful visible
   results.
7. Terminal `start`, `write`, `read`, `resize`, and `stop` have distinct,
   inspectable presentations, and control input cannot inject terminal escapes.
8. Replay and resize reproduce command content without persisted styling.
9. Delegate-family card layout and expansion remain unchanged; bounded shell
   summaries preserve both command head and tail without growing persisted
   progress state.
10. Focused rendering checks cover narrow and wide terminals and verify that no
    rendered row exceeds its width.

## Planning Notes

### TaskIntentDraft

- Outcome: model-issued shell operations are visibly auditable without terminal
  width failures.
- Success evidence: focused Bash, Terminal, long-command, syntax-fallback,
  replay, and Delegate-summary rendering checks.
- Stop condition: the presentation contract above is implemented without shell
  runtime or Delegate-card redesign.

### BaselineUsageDraft

- Required baseline refs: current transcript tool renderer, markdown syntax
  highlighter, Terminal input contract, multi-agent activity summary, and
  transcript overflow/viewport specifications.
- Missing refs: none.
- Decision: continue.

### ImpactStatementDraft

- Affected layers: `neo-tui` transcript presentation and bounded child activity
  summary generation in `neo-agent-core`.
- Canonical owner: existing tool-call state plus one render-only shell module.
- Compatibility: existing events, schemas, persisted sessions, shortcuts, and
  Delegate-family cards remain valid.
- ArchitectureReviewRequired: yes, because the work crosses TUI presentation
  and multi-agent summary boundaries.
- ADR signal: no; the spec reuses existing owners and contracts rather than
  creating a durable public architecture surface.
