/**
 * HTTP/WebSocket client for the neo-webui same-origin API. Only knows
 * relative paths, fixed JSON headers, typed error codes and the long
 * connection. It never touches the filesystem, model keys, session JSONL or
 * legacy RPC. Tokens are never stored, logged or rendered.
 */

import type {
  ToolOutputRange,
  WebUiBootstrap,
  WebUiCursor,
  WebUiErrorBody,
  WebUiErrorCode,
  WebUiInputAccepted,
  WebUiInputDelivery,
  WebUiCancelling,
  WebUiQuestionAnswer,
  WebUiServerMessage,
  WebUiSessionMetadata,
  WebUiSessionPage,
  WebUiSessionStarted,
  WebUiComposer,
  WebUiSnapshot,
  WebUiWatchRequest,
} from "./protocol";

export class ApiError extends Error {
  readonly status: number;
  readonly code: WebUiErrorCode | null;

  constructor(status: number, code: WebUiErrorCode | null) {
    super(`request failed (${status})`);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
  }
}

const JSON_HEADERS: Record<string, string> = {
  "content-type": "application/json",
};

async function parseError(response: Response): Promise<ApiError> {
  let code: WebUiErrorCode | null = null;
  try {
    const body = (await response.json()) as WebUiErrorBody;
    if (body && typeof body.code === "string") {
      code = body.code;
    }
  } catch {
    // Error responses are minimal; ignore parse failures.
  }
  return new ApiError(response.status, code);
}

async function request<T>(
  method: string,
  path: string,
  body?: unknown,
): Promise<T> {
  let response: Response;
  try {
    response = await fetch(path, {
      method,
      headers: body === undefined ? undefined : JSON_HEADERS,
      body: body === undefined ? undefined : JSON.stringify(body),
      credentials: "same-origin",
    });
  } catch {
    throw new ApiError(0, null);
  }
  if (!response.ok) {
    throw await parseError(response);
  }
  // 204 No Content (claim, approval/question resolve) has no JSON body.
  if (response.status === 204) {
    return undefined as T;
  }
  return (await response.json()) as T;
}

/**
 * One-time token claim. The token is read from location.hash into a local
 * variable, exchanged once, and the fragment is cleared with
 * history.replaceState before bootstrap starts. Returns false when the token
 * is missing or the claim failed; the caller shows a non-sensitive prompt.
 */
export async function claimAccessToken(): Promise<boolean> {
  const hash = window.location.hash;
  const match = /(?:^|#|&)access=([^&]+)/.exec(hash);
  if (!match || !match[1]) {
    return false;
  }
  const token = decodeURIComponent(match[1]);
  let ok = false;
  try {
    await request<Record<string, never>>("POST", "/api/auth/claim", {
      token,
    });
    ok = true;
  } catch {
    ok = false;
  }
  // Always clear the fragment so the one-time token never lingers in the
  // address bar, history entries or screenshots.
  window.history.replaceState(null, "", window.location.pathname + window.location.search);
  return ok;
}

export function fetchBootstrap(): Promise<WebUiBootstrap> {
  return request<WebUiBootstrap>("GET", "/api/bootstrap");
}

export function listSessions(params: {
  scope: "active" | "archived";
  query?: string;
  cursor?: string;
  limit?: number;
}): Promise<WebUiSessionPage> {
  const search = new URLSearchParams();
  search.set("scope", params.scope);
  if (params.query) search.set("query", params.query);
  if (params.cursor) search.set("cursor", params.cursor);
  if (params.limit !== undefined) search.set("limit", String(params.limit));
  return request<WebUiSessionPage>("GET", `/api/sessions?${search.toString()}`);
}

export function fetchSnapshot(sessionId: string): Promise<WebUiSnapshot> {
  return request<WebUiSnapshot>(
    "GET",
    `/api/sessions/${encodeURIComponent(sessionId)}/snapshot`,
  );
}

export function createSession(
  message: string,
  composer?: WebUiComposer,
): Promise<WebUiSessionStarted> {
  return request<WebUiSessionStarted>("POST", "/api/sessions", {
    message,
    ...(composer ? { composer } : {}),
  });
}

export function startTurn(
  sessionId: string,
  message: string,
  composer?: WebUiComposer,
): Promise<WebUiSessionStarted> {
  return request<WebUiSessionStarted>(
    "POST",
    `/api/sessions/${encodeURIComponent(sessionId)}/turns`,
    { message, ...(composer ? { composer } : {}) },
  );
}

export function sendInput(
  sessionId: string,
  turnId: string,
  delivery: WebUiInputDelivery,
  message: string,
): Promise<WebUiInputAccepted> {
  return request<WebUiInputAccepted>(
    "POST",
    `/api/sessions/${encodeURIComponent(sessionId)}/input`,
    { turn_id: turnId, delivery, message },
  );
}

export function cancelTurn(
  sessionId: string,
  turnId: string,
): Promise<WebUiCancelling> {
  return request<WebUiCancelling>(
    "POST",
    `/api/sessions/${encodeURIComponent(sessionId)}/cancel`,
    { turn_id: turnId },
  );
}

export function resolveApproval(
  sessionId: string,
  turnId: string,
  requestId: string,
  action: unknown,
  feedback?: string,
): Promise<void> {
  const body: Record<string, unknown> = {
    turn_id: turnId,
    request_id: requestId,
    action,
  };
  if (feedback !== undefined) body.feedback = feedback;
  return request<void>(
    "POST",
    `/api/sessions/${encodeURIComponent(sessionId)}/approval`,
    body,
  );
}

export function resolveQuestion(
  sessionId: string,
  turnId: string,
  questionId: string,
  answer: WebUiQuestionAnswer,
): Promise<void> {
  return request<void>(
    "POST",
    `/api/sessions/${encodeURIComponent(sessionId)}/question`,
    { turn_id: turnId, question_id: questionId, answer },
  );
}

/** PATCH metadata; only the fields the user actually changed are sent. */
export function updateMetadata(
  sessionId: string,
  change: { title?: string; pinned?: boolean; archived?: boolean },
): Promise<WebUiSessionMetadata> {
  return request<WebUiSessionMetadata>(
    "PATCH",
    `/api/sessions/${encodeURIComponent(sessionId)}`,
    change,
  );
}

/** Read full tool/terminal output. `outputRef` is the opaque event
 * `output.id`, passed back verbatim — never encoded, decoded or guessed. */
export function readToolOutput(
  sessionId: string,
  outputRef: string,
  startLine = 0,
  maxLines = 400,
): Promise<ToolOutputRange> {
  const search = new URLSearchParams({
    start_line: String(startLine),
    max_lines: String(maxLines),
  });
  return request<ToolOutputRange>(
    "GET",
    `/api/sessions/${encodeURIComponent(sessionId)}/tool-output/${encodeURIComponent(outputRef)}?${search.toString()}`,
  );
}

// ---------------------------------------------------------------------------
// Long connection (one socket, two subscriptions).
// ---------------------------------------------------------------------------

export interface EventsSocket {
  send(request: WebUiWatchRequest): void;
  close(): void;
}

export interface EventsSocketHandlers {
  onMessage(message: WebUiServerMessage): void;
  /** code 1013 = slow consumer; the client must reconnect without a cursor. */
  onClose(code: number): void;
  /** Fired once the connection is actually established (after the WebSocket
   * handshake), not when the constructor returns. */
  onOpen(): void;
}

export function openEventsSocket(handlers: EventsSocketHandlers): EventsSocket {
  const scheme = window.location.protocol === "https:" ? "wss:" : "ws:";
  const url = `${scheme}//${window.location.host}/api/events`;
  const socket = new WebSocket(url);
  socket.onmessage = (event: MessageEvent) => {
    if (typeof event.data !== "string") return;
    let message: WebUiServerMessage;
    try {
      message = JSON.parse(event.data) as WebUiServerMessage;
    } catch {
      return;
    }
    handlers.onMessage(message);
  };
  socket.onclose = (event: CloseEvent) => {
    handlers.onClose(event.code);
  };
  if (socket.readyState === WebSocket.OPEN) {
    queueMicrotask(() => handlers.onOpen());
  } else {
    socket.onopen = () => handlers.onOpen();
  }
  return {
    send(request: WebUiWatchRequest) {
      if (socket.readyState === WebSocket.OPEN) {
        socket.send(JSON.stringify(request));
      } else {
        socket.addEventListener(
          "open",
          () => socket.send(JSON.stringify(request)),
          { once: true },
        );
      }
    },
    close() {
      socket.close();
    },
  };
}

export type { WebUiCursor };
