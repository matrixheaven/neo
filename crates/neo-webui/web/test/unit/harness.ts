/**
 * Shared test harness: fetch mock routed by (method, path) and a fake
 * WebSocket that records subscriptions and replays server messages.
 */

import type { WebUiServerMessage } from "../../src/protocol";
import { loadFixture, type FixtureEnvelope } from "./fixture";

export const fixture = loadFixture();

export interface RecordedRequest {
  method: string;
  url: string;
  body: unknown;
}

export const recordedRequests: RecordedRequest[] = [];

export class FakeWebSocket {
  static OPEN = 1;
  static CONNECTING = 0;
  static instances: FakeWebSocket[] = [];
  /** When false, new sockets stay CONNECTING and never fire onopen — used to
   * simulate an unreachable service for backoff tests. */
  static autoOpen = true;

  readonly url: string;
  readyState: number;
  sent: string[] = [];
  closed = false;
  onmessage: ((event: { data: string }) => void) | null = null;
  onclose: ((event: { code: number }) => void) | null = null;
  onopen: (() => void) | null = null;

  constructor(url: string) {
    this.url = url;
    this.readyState = FakeWebSocket.autoOpen ? FakeWebSocket.OPEN : FakeWebSocket.CONNECTING;
    FakeWebSocket.instances.push(this);
  }

  send(data: string): void {
    this.sent.push(data);
  }

  close(): void {
    this.closed = true;
  }

  addEventListener(): void {}

  emit(message: WebUiServerMessage | FixtureEnvelope): void {
    this.onmessage?.({ data: JSON.stringify(message) });
  }

  closeWith(code: number): void {
    this.onclose?.({ code });
  }

  watchSessionIds(): string[] {
    return this.sent
      .map((data) => JSON.parse(data) as { type: string; session_id?: string })
      .filter((entry) => entry.type === "watch_session")
      .map((entry) => entry.session_id ?? "");
  }
}

export function resetHarness(): void {
  recordedRequests.length = 0;
  FakeWebSocket.instances = [];
  FakeWebSocket.autoOpen = true;
  window.localStorage.clear();
}

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

export const workspaceSessions = (
  fixture.long_connection.workspace_snapshot.workspaces ?? []
).flatMap((group) => group.sessions) as Array<Record<string, unknown>>;

export function bootstrapBody(): Record<string, unknown> {
  return {
    workspace_label: "neo",
    models: [
      { alias: "gpt-5-codex", provider: "openai", capabilities: [] },
      { alias: "claude-sonnet", provider: "anthropic", capabilities: [] },
    ],
    permission_modes: ["ask", "auto", "yolo"],
    development_modes: ["normal", "plan", "goal"],
    sessions: workspaceSessions,
  };
}

/** Fixture-style child-agent wire history for the drill-down panel (R4):
 * the same projection inputs as a main snapshot — user + assistant canonical
 * appends with contiguous sequences. */
export function agentHistoryBody(agentId: string): Record<string, unknown> {
  return {
    agent_id: agentId,
    watermark: 2,
    history: [
      {
        sequence: 1,
        event: {
          MessageAppended: {
            message: { User: { content: [{ Text: { text: "检查 relay 测试覆盖" } }] } },
          },
        },
      },
      {
        sequence: 2,
        event: {
          MessageAppended: {
            message: {
              Assistant: {
                content: [{ Text: { text: "子代理结论：relay 覆盖达标。" } }],
                tool_calls: [],
                stop_reason: "EndTurn",
              },
            },
          },
        },
      },
    ],
  };
}

export function mockFetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
  const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
  const method = init?.method ?? "GET";
  const body = init?.body ? JSON.parse(String(init.body)) : undefined;
  recordedRequests.push({ method, url, body });

  const path = url.startsWith("http") ? new URL(url).pathname + new URL(url).search : url;

  if (path === "/api/auth/claim" && method === "POST") {
    return Promise.resolve(jsonResponse({}, 200));
  }
  if (path === "/api/bootstrap") {
    return Promise.resolve(jsonResponse(bootstrapBody()));
  }
  if (path.startsWith("/api/sessions?")) {
    const query = new URLSearchParams(path.split("?")[1] ?? "");
    const needle = (query.get("query") ?? "").toLowerCase();
    const scope = query.get("scope") ?? "active";
    const items = workspaceSessions.filter((entry) => {
      const archived = entry.archived === true;
      const inScope = scope === "archived" ? archived : !archived;
      const title = String(entry.title ?? "").toLowerCase();
      return inScope && (needle === "" || title.includes(needle));
    });
    return Promise.resolve(jsonResponse({ items }));
  }
  const snapshotMatch = /^\/api\/sessions\/([^/]+)\/snapshot$/.exec(path);
  if (snapshotMatch) {
    const session = fixture.sessions.find((entry) => entry.session_id === snapshotMatch[1]);
    if (session) return Promise.resolve(jsonResponse(session.snapshot));
    return Promise.resolve(jsonResponse({ code: "not_found" }, 404));
  }
  if (path === "/api/sessions" && method === "POST") {
    return Promise.resolve(
      jsonResponse(
        {
          session_id: "session_0001",
          turn_id: "turn_01",
          state: {
            phase: "running",
            waiting_approval: false,
            waiting_question: false,
            current_turn_id: "turn_01",
          },
          stream_id: fixture.stream_id,
          sequence: 0,
        },
        201,
      ),
    );
  }
  const turnsMatch = /^\/api\/sessions\/([^/]+)\/turns$/.exec(path);
  if (turnsMatch && method === "POST") {
    return Promise.resolve(
      jsonResponse(
        {
          session_id: turnsMatch[1],
          turn_id: "turn_09",
          state: {
            phase: "running",
            waiting_approval: false,
            waiting_question: false,
            current_turn_id: "turn_09",
          },
          stream_id: fixture.stream_id,
          sequence: 99,
        },
        201,
      ),
    );
  }
  if (path.endsWith("/input") && method === "POST") {
    return Promise.resolve(jsonResponse({ turn_id: body?.turn_id ?? "turn_01" }, 202));
  }
  if (path.endsWith("/cancel") && method === "POST") {
    return Promise.resolve(jsonResponse({ turn_id: body?.turn_id ?? "turn_01" }, 202));
  }
  if (path.endsWith("/approval") && method === "POST") {
    return Promise.resolve(jsonResponse({}, 202));
  }
  if (path.endsWith("/question") && method === "POST") {
    return Promise.resolve(jsonResponse({}, 202));
  }
  const agentHistoryMatch = /^\/api\/sessions\/([^/]+)\/agents\/([^/]+)\/history$/.exec(path);
  if (agentHistoryMatch) {
    const agentId = decodeURIComponent(agentHistoryMatch[2]);
    if (agentId === "agent_missing") {
      return Promise.resolve(jsonResponse({ code: "not_found" }, 404));
    }
    return Promise.resolve(jsonResponse(agentHistoryBody(agentId)));
  }
  const toolOutputMatch = /^\/api\/sessions\/([^/]+)\/tool-output\/([^?]+)/.exec(path);
  if (toolOutputMatch) {
    return Promise.resolve(
      jsonResponse({
        text: "服务端返回的完整输出内容\ntest result: ok. 42 passed",
        start_line: 0,
        next_line: 2,
        reached_end: true,
      }),
    );
  }
  const patchMatch = /^\/api\/sessions\/([^/]+)$/.exec(path);
  if (patchMatch && method === "PATCH") {
    return Promise.resolve(
      jsonResponse({ title: body?.title ?? null, pinned: body?.pinned ?? false, archived: body?.archived ?? false }),
    );
  }
  return Promise.resolve(jsonResponse({ code: "not_found" }, 404));
}
