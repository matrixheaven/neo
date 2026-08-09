/**
 * Test-only mock server (never part of the production build): serves the
 * built dist/, answers the API from the fixed sample, and replays the two
 * subscription tiers over WebSocket. The production index.html contains no
 * sample switch, test data or auth bypass — this server exists only for
 * browser verification.
 */

import { createServer } from "node:http";
import { readFileSync, existsSync } from "node:fs";
import { resolve, dirname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import { WebSocketServer } from "ws";

const here = dirname(fileURLToPath(import.meta.url));
const webRoot = resolve(here, "..", "..");
const distRoot = join(webRoot, "dist");
const fixture = JSON.parse(
  readFileSync(resolve(webRoot, "..", "fixtures", "webui-events.json"), "utf8"),
);

const PORT = Number(process.env.MOCK_PORT ?? 47921);

if (!existsSync(join(distRoot, "index.html"))) {
  console.error("dist/ missing: run `npm run build` first");
  process.exit(1);
}

function json(res, body, status = 200, headers = {}) {
  res.writeHead(status, {
    "content-type": "application/json",
    "cache-control": "no-store",
    ...headers,
  });
  res.end(JSON.stringify(body));
}

function readBody(req) {
  return new Promise((resolveBody) => {
    let data = "";
    req.on("data", (chunk) => {
      data += chunk;
    });
    req.on("end", () => {
      try {
        resolveBody(JSON.parse(data || "{}"));
      } catch {
        resolveBody({});
      }
    });
  });
}

const workspaceSessions = fixture.long_connection.workspace_snapshot.sessions;

const server = createServer(async (req, res) => {
  const url = new URL(req.url ?? "/", "http://127.0.0.1");
  const path = url.pathname;
  const method = req.method ?? "GET";

  if (path === "/api/auth/claim" && method === "POST") {
    json(res, {}, 200, { "set-cookie": "neo_webui=mock; HttpOnly; SameSite=Strict; Path=/" });
    return;
  }
  if (path === "/api/bootstrap") {
    json(res, {
      workspace_label: "neo",
      models: ["gpt-5-codex", "claude-sonnet"],
      permission_modes: ["ask", "auto", "yolo"],
      development_modes: ["normal", "plan", "goal"],
      sessions: workspaceSessions,
    });
    return;
  }
  if (path === "/api/sessions" && method === "GET") {
    const needle = (url.searchParams.get("query") ?? "").toLowerCase();
    const scope = url.searchParams.get("scope") ?? "active";
    const items = workspaceSessions.filter((entry) => {
      const inScope = scope === "archived" ? entry.archived === true : entry.archived !== true;
      return inScope && (needle === "" || String(entry.title ?? "").toLowerCase().includes(needle));
    });
    json(res, { items });
    return;
  }
  const snapshotMatch = /^\/api\/sessions\/([^/]+)\/snapshot$/.exec(path);
  if (snapshotMatch) {
    const session = fixture.sessions.find((entry) => entry.session_id === snapshotMatch[1]);
    if (session) json(res, session.snapshot);
    else json(res, { code: "not_found" }, 404);
    return;
  }
  if (path === "/api/sessions" && method === "POST") {
    const body = await readBody(req);
    if (!body.message || String(body.message).trim() === "") {
      json(res, { code: "invalid_request" }, 400);
      return;
    }
    json(
      res,
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
    );
    return;
  }
  const turnsMatch = /^\/api\/sessions\/([^/]+)\/turns$/.exec(path);
  if (turnsMatch && method === "POST") {
    json(
      res,
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
    );
    return;
  }
  if (path.endsWith("/input") && method === "POST") {
    const body = await readBody(req);
    json(res, { turn_id: body.turn_id ?? "turn_01" }, 202);
    return;
  }
  if (path.endsWith("/cancel") && method === "POST") {
    const body = await readBody(req);
    json(res, { turn_id: body.turn_id ?? "turn_01" }, 202);
    return;
  }
  if ((path.endsWith("/approval") || path.endsWith("/question")) && method === "POST") {
    json(res, {}, 202);
    return;
  }
  const toolOutputMatch = /^\/api\/sessions\/([^/]+)\/tool-output\/(.+)$/.exec(path);
  if (toolOutputMatch) {
    json(res, {
      text: "服务端返回的完整输出内容\ntest result: ok. 42 passed",
      start_line: 0,
      next_line: 2,
      reached_end: true,
    });
    return;
  }
  const patchMatch = /^\/api\/sessions\/([^/]+)$/.exec(path);
  if (patchMatch && method === "PATCH") {
    json(res, { title: null, pinned: false, archived: false });
    return;
  }

  // Static assets (built dist only).
  const relative = path === "/" ? "index.html" : normalize(path).replace(/^[/\\]+/, "");
  const file = join(distRoot, relative);
  if (!file.startsWith(distRoot) || !existsSync(file)) {
    json(res, { code: "not_found" }, 404);
    return;
  }
  const contentType = file.endsWith(".html")
    ? "text/html; charset=utf-8"
    : file.endsWith(".js")
      ? "text/javascript"
      : file.endsWith(".css")
        ? "text/css"
        : "application/octet-stream";
  res.writeHead(200, { "content-type": contentType, "cache-control": "no-store" });
  res.end(readFileSync(file));
});

const wss = new WebSocketServer({ noServer: true });

server.on("upgrade", (req, socket, head) => {
  const url = new URL(req.url ?? "/", "http://127.0.0.1");
  if (url.pathname !== "/api/events") {
    socket.destroy();
    return;
  }
  wss.handleUpgrade(req, socket, head, (ws) => {
    ws.on("message", (data) => {
      let message;
      try {
        message = JSON.parse(String(data));
      } catch {
        return;
      }
      if (message.type === "watch_workspace") {
        ws.send(JSON.stringify(fixture.long_connection.workspace_snapshot));
        return;
      }
      if (message.type === "watch_session") {
        const session = fixture.sessions.find(
          (entry) => entry.session_id === message.session_id,
        );
        if (!session) {
          ws.send(JSON.stringify({ code: "not_found" }));
          return;
        }
        ws.send(JSON.stringify({ type: "session_snapshot", snapshot: session.snapshot }));
        for (const envelope of session.after_snapshot) {
          ws.send(JSON.stringify(envelope));
        }
      }
    });
  });
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`mock server on http://127.0.0.1:${PORT}`);
});
