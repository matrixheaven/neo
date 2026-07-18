# Transcript Boundary Semantics Design

## Problem

Delegate, swarm, and workflow events may update cards that already exist earlier in the transcript. Treating every update as a visible boundary prematurely completes an active thinking block. Later thinking deltas then resume with an artificial separator, and terminal scrollback can preserve transient completed-state bullets.

## Design

`TranscriptStore` owns transcript boundary decisions because it knows whether an upsert inserts a new entry or mutates an existing one.

- Inserting a new card uses `TranscriptStore::push`, which completes active text blocks through the existing visible-boundary behavior.
- Updating, grouping, or refreshing an existing card does not complete active thinking or assistant text.
- Event routing forwards delegate, swarm, workflow, and progress events without preemptively finishing active text blocks.
- Provider and runtime thinking lifecycle events remain unchanged.

## Verification

Regression tests interleave thinking deltas with delegate and swarm progress updates. They assert that the thinking content remains one block with byte-for-byte continuous text while the existing cards still receive their progress updates.
