/**
 * Loads the fixed, read-only sample (crates/neo-webui/fixtures/webui-events.json)
 * at test runtime so tests never drift from the canonical fixture.
 */

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import type {
  AgentEvent,
  WebUiOutputRef,
  WebUiServerMessage,
  WebUiSnapshot,
} from "../../src/protocol";

export interface FixtureEnvelope {
  type: string;
  stream_id?: string;
  session_id?: string;
  sequence?: number;
  workspace_sequence?: number;
  event?: unknown;
  output?: WebUiOutputRef | null;
  snapshot?: WebUiSnapshot;
  sessions?: unknown[];
  workspaces?: Array<{ label: string; current: boolean; sessions: unknown[] }>;
}

export interface FixtureSession {
  session_id: string;
  snapshot: WebUiSnapshot;
  after_snapshot: FixtureEnvelope[];
  replay_after_cursor?: {
    after: { stream_id: string; sequence: number };
    envelopes: FixtureEnvelope[];
  };
}

export interface Fixture {
  stream_id: string;
  sessions: FixtureSession[];
  snapshot_replacement: {
    stream_id: string;
    session_id: string;
    snapshot: WebUiSnapshot;
    after_snapshot: FixtureEnvelope[];
  };
  long_connection: {
    client_messages: unknown[];
    workspace_snapshot: FixtureEnvelope;
    session_summary_changed: FixtureEnvelope;
  };
  errors: Array<{ code: string }>;
  close_samples: Array<{ code: number; reason: string }>;
}

// Vitest runs with cwd = crates/neo-webui/web; the fixture lives beside it.
const fixturePath = resolve(process.cwd(), "../fixtures/webui-events.json");

export function loadFixture(): Fixture {
  return JSON.parse(readFileSync(fixturePath, "utf8")) as Fixture;
}

export function asServerMessage(envelope: FixtureEnvelope): WebUiServerMessage {
  return envelope as unknown as WebUiServerMessage;
}

export function eventOf(envelope: FixtureEnvelope): AgentEvent {
  return envelope.event as AgentEvent;
}
