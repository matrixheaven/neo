/**
 * Test-only mock server (never part of the production build): serves the
 * built dist/, answers the API in the final protocol shape (grouped
 * workspace_snapshot, session_state carrying usage/context, attachments,
 * agent history), and replays the two subscription tiers over WebSocket.
 * The production index.html contains no sample switch, test data or auth
 * bypass — this server exists only for browser verification.
 *
 * Scenario sessions beyond the fixed sample:
 * - session_show1 "重设计走查": one showcase transcript exercising the long
 *   user message clamp, a finished TurnFold with edit/write file changes
 *   (answer footer), a delegate agent-line and a still-running tool line.
 * - session_big "长会话压测": a >100k-character session generated
 *   programmatically for the content-visibility performance check.
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

// ---------------------------------------------------------------------------
// Scenario data (showcase + perf sessions).
// ---------------------------------------------------------------------------

const SHOWCASE_ID = "session_show1";
const BIG_ID = "session_big";
const SHOWCASE_AGENT_ID = "agent_show1";

const LONG_USER_TEXT = [
  "请把这次重设计的验收清单逐条走一遍，重点核对以下几项：",
  "1. 转录区改为行式层级后，思考、工具、终端、工作流都应该是单行折叠；",
  "2. 用户消息是唯一的 bubble，长消息要有渐变折叠和展开按钮；",
  "3. 侧栏按工作区分组，当前工作区展开、其他工作区折叠，置顶集中在一组；",
  "4. composer 下方是 pill 行：附件、模型、权限、模式、推理，右侧是上下文环；",
  "5. 子代理是 agent-line，点击打开右侧详情面板，面板里是只读子转录；",
  "6. swarm 是一块带聚合进度条的成员列表；",
  "7. 完成的回答下方要有文件修改列表和复制按钮；",
  "8. 右键菜单只在会话行上出现，不能切换当前会话；",
  "9. 窄桌面是抽屉，手机是单列；",
  "10. 亮色主题要有一整套对照截图。",
  "以上每一项都要有截图证据，性能项要有 DOM 探测证据。",
].join("\n");

function showcaseAgent(state) {
  return {
    id: SHOWCASE_AGENT_ID,
    display_name: "explorer",
    path: "/root/explorer",
    role: "explorer",
    mode: "background",
    context: "inherit",
    state,
    task: "检查 relay 测试覆盖",
    task_title: "检查测试覆盖",
    created_at_ms: 1723000001000,
    updated_at_ms: 1723000001600,
    started_at_ms: 1723000001100,
    detached_from_foreground: true,
    terminal_reason: state === "completed" ? "completed" : undefined,
    run_count: 1,
    live_messages_received: 6,
    tool_count: 1,
    token_count: 20,
    input_token_count: 480,
    cache_read_token_count: 128,
    elapsed: { secs: 7, nanos: 0 },
    latest_text: "relay 覆盖检查进行中",
  };
}

/** Showcase snapshot: the whole transcript is authoritative history. */
function showcaseSnapshot() {
  const outputRef = {
    id: "bW9jay1vdXRwdXQtc2hvd2Nhc2U",
    byte_len: 12048,
    line_count: 132,
    complete: true,
  };
  const runningOutputRef = {
    id: "bW9jay1vdXRwdXQtcnVubmluZw",
    byte_len: 2048,
    line_count: 31,
    complete: false,
  };
  const history = [
    {
      sequence: 1,
      event: {
        MessageAppended: { message: { User: { content: [{ Text: { text: LONG_USER_TEXT } }] } } },
      },
    },
    {
      sequence: 2,
      event: { ThinkingStarted: { turn: 1, id: "think_s1", kind: "full" } },
    },
    {
      sequence: 3,
      event: {
        ThinkingDelta: {
          turn: 1,
          text: "先核对转录的行式层级：思考、工具、终端都是单行折叠，展开后再看细节。",
        },
      },
    },
    { sequence: 4, event: { ThinkingFinished: { turn: 1, redacted: false } } },
    {
      sequence: 5,
      event: {
        ToolExecutionStarted: {
          turn: 1,
          id: "tool_s1",
          name: "bash",
          arguments: { command: "cargo test -p neo-webui" },
        },
      },
      output: { ...outputRef, complete: false },
    },
    {
      sequence: 6,
      event: {
        ToolExecutionFinished: {
          turn: 1,
          id: "tool_s1",
          name: "bash",
          result: {
            content: "test result: ok. 42 passed",
            is_error: false,
            details: null,
            terminate: false,
          },
        },
      },
      output: outputRef,
    },
    {
      sequence: 7,
      event: {
        ToolExecutionStarted: {
          turn: 1,
          id: "tool_s2",
          name: "edit",
          arguments: {
            path: "web/src/styles.css",
            old: ".line {\n  min-height: 28px;\n}",
            new: ".line {\n  min-height: 24px;\n  gap: 6px;\n}",
          },
        },
      },
    },
    {
      sequence: 8,
      event: {
        ToolExecutionFinished: {
          turn: 1,
          id: "tool_s2",
          name: "edit",
          result: {
            content: "已应用编辑",
            is_error: false,
            details: {
              changes: [
                {
                  path: "web/src/styles.css",
                  status: "committed",
                  added: 2,
                  removed: 1,
                  diff: "--- a/web/src/styles.css\n+++ b/web/src/styles.css\n@@ -1,3 +1,4 @@\n .line {\n-  min-height: 28px;\n+  min-height: 24px;\n+  gap: 6px;\n }\n",
                },
              ],
            },
            terminate: false,
          },
        },
      },
    },
    {
      sequence: 9,
      event: {
        ToolExecutionStarted: {
          turn: 1,
          id: "tool_s3",
          name: "write",
          arguments: {
            path: "web/src/acceptance-notes.md",
            content: "# 验收记录\n- 转录行式层级\n- 侧栏工作区分组",
          },
        },
      },
    },
    {
      sequence: 10,
      event: {
        ToolExecutionFinished: {
          turn: 1,
          id: "tool_s3",
          name: "write",
          result: {
            content: "已写入文件",
            is_error: false,
            details: {
              changes: [
                {
                  path: "web/src/acceptance-notes.md",
                  status: "committed_unsynced",
                  operation: "created",
                  added: 3,
                  removed: 0,
                  content: "# 验收记录\n- 转录行式层级\n- 侧栏工作区分组",
                },
              ],
            },
            terminate: false,
          },
        },
      },
    },
    {
      sequence: 11,
      event: {
        MessageAppended: {
          message: {
            Assistant: {
              content: [
                {
                  Text: {
                    text: "验收改造已完成：转录改为行式层级，结束的回合折叠为工作过程摘要，回答下方列出本轮修改的文件。",
                  },
                },
              ],
              tool_calls: [],
              stop_reason: "EndTurn",
            },
          },
        },
      },
    },
    {
      sequence: 12,
      event: {
        MessageAppended: {
          message: { User: { content: [{ Text: { text: "再把子代理详情面板打开看看。" } }] } },
        },
      },
    },
    {
      sequence: 13,
      event: { ThinkingStarted: { turn: 2, id: "think_s2", kind: "full" } },
    },
    {
      sequence: 14,
      event: {
        ThinkingDelta: {
          turn: 2,
          text: "子代理的历史已经落盘，面板直接按同一条投影线渲染即可。",
        },
      },
    },
    { sequence: 15, event: { ThinkingFinished: { turn: 2, redacted: false } } },
    {
      sequence: 16,
      event: { DelegateStarted: { turn: 2, agent: showcaseAgent("running") } },
    },
    {
      sequence: 17,
      event: {
        DelegateProgressUpdated: {
          turn: 2,
          progress: {
            agent_id: SHOWCASE_AGENT_ID,
            state: "running",
            mode: "background",
            detached_from_foreground: true,
            updated_at_ms: 1723000001500,
            run_count: 1,
            tool_count: 1,
            token_count: 16,
            elapsed_ms: 6500,
            latest_text: "relay 覆盖检查进行中",
          },
        },
      },
    },
    {
      sequence: 18,
      event: { DelegateFinished: { turn: 2, agent: showcaseAgent("completed") } },
    },
    {
      sequence: 19,
      event: {
        ToolExecutionStarted: {
          turn: 2,
          id: "tool_s4",
          name: "bash",
          arguments: { command: "npm run test:browser" },
        },
      },
      output: runningOutputRef,
    },
    {
      sequence: 20,
      event: {
        TokenUsage: {
          turn: 2,
          usage: {
            input_tokens: 48210,
            output_tokens: 1204,
            input_cache_read_tokens: 16384,
            input_cache_write_tokens: 0,
          },
        },
      },
    },
    {
      sequence: 21,
      event: {
        ContextWindowUpdated: {
          turn: 2,
          used_tokens: 49414,
          projected_tokens: 51200,
          max_tokens: 200000,
          remaining_tokens: 150586,
        },
      },
    },
  ];
  return {
    stream_id: fixture.stream_id,
    session_id: SHOWCASE_ID,
    watermark: history.length,
    session: {
      phase: "running",
      waiting_approval: false,
      waiting_question: false,
      current_turn_id: "turn_show2",
      token_usage: {
        input_tokens: 48210,
        output_tokens: 1204,
        input_cache_read_tokens: 16384,
        input_cache_write_tokens: 0,
      },
      context_window: {
        used_tokens: 49414,
        projected_tokens: 51200,
        max_tokens: 200000,
        remaining_tokens: 150586,
      },
    },
    metadata: {
      title: "重设计走查",
      pinned: false,
      archived: false,
      updated_at: "2026-08-09T10:02:00+00:00",
    },
    history,
  };
}

const PARAGRAPH =
  "这一段是压测会话的正文，用来构造十万字符级别的转录。行式层级下每个回合是一个用户 bubble 加一个回答，" +
  "离屏的项目依赖 content-visibility 跳过渲染，滚动和侧栏拖拽都不能出现明显卡顿。";

function bigSnapshot() {
  const history = [];
  let sequence = 0;
  for (let turn = 1; turn <= 200; turn += 1) {
    sequence += 1;
    history.push({
      sequence,
      event: {
        MessageAppended: {
          message: {
            User: { content: [{ Text: { text: `第 ${turn} 轮：${PARAGRAPH}` } }] },
          },
        },
      },
    });
    sequence += 1;
    history.push({
      sequence,
      event: {
        MessageAppended: {
          message: {
            Assistant: {
              content: [
                {
                  Text: {
                    text:
                      `第 ${turn} 轮回答。${PARAGRAPH.repeat(4)}` +
                      (turn === 200 ? "（结尾标记）" : ""),
                  },
                },
              ],
              tool_calls: [],
              stop_reason: "EndTurn",
            },
          },
        },
      },
    });
  }
  return {
    stream_id: fixture.stream_id,
    session_id: BIG_ID,
    watermark: history.length,
    session: {
      phase: "idle",
      waiting_approval: false,
      waiting_question: false,
      current_turn_id: null,
    },
    metadata: {
      title: "长会话压测",
      pinned: false,
      archived: false,
      updated_at: "2026-08-09T09:30:00+00:00",
    },
    history,
  };
}

function bigCharCount() {
  return bigSnapshot().history.reduce(
    (total, entry) => total + JSON.stringify(entry.event).length,
    0,
  );
}

const SHOWCASE_SUMMARY = {
  session_id: SHOWCASE_ID,
  title: "重设计走查",
  updated_at: "2026-08-09T10:02:00+00:00",
  pinned: false,
  archived: false,
  state: "running",
  workspace_label: "neo",
};

const BIG_SUMMARY = {
  session_id: BIG_ID,
  title: "长会话压测",
  updated_at: "2026-08-09T09:30:00+00:00",
  pinned: false,
  archived: false,
  state: "idle",
  workspace_label: "neo",
};

/** Grouped workspace snapshot: fixture groups plus the scenario sessions. */
function workspaceSnapshot() {
  const base = fixture.long_connection.workspace_snapshot;
  return {
    ...base,
    workspaces: base.workspaces.map((group) =>
      group.current
        ? { ...group, sessions: [SHOWCASE_SUMMARY, ...group.sessions, BIG_SUMMARY] }
        : group,
    ),
  };
}

const scenarioSnapshots = new Map([
  [SHOWCASE_ID, showcaseSnapshot()],
  [BIG_ID, bigSnapshot()],
]);

function findSnapshot(sessionId) {
  const scenario = scenarioSnapshots.get(sessionId);
  if (scenario) return { snapshot: scenario, after: [] };
  const entry = fixture.sessions.find((item) => item.session_id === sessionId);
  if (!entry) return null;
  // The fixed sample deliberately stores some envelopes out of order (dedup /
  // gap cases for the reducer unit tests); the live wire is contiguous, so the
  // mock replays them sorted by sequence.
  const after = [...entry.after_snapshot].sort((a, b) => a.sequence - b.sequence);
  return { snapshot: entry.snapshot, after };
}

/** Lazy child-agent history for the drill-down panel. */
function agentHistory(sessionId, agentId) {
  if (agentId !== SHOWCASE_AGENT_ID && !agentId.startsWith("agent_")) return null;
  return {
    agent_id: agentId,
    watermark: 5,
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
        event: { ThinkingStarted: { turn: 1, id: "think_a1", kind: "full" } },
      },
      {
        sequence: 3,
        event: {
          ThinkingDelta: { turn: 1, text: "先列出 relay 模块的行为测试，再核对断言。" },
        },
      },
      { sequence: 4, event: { ThinkingFinished: { turn: 1, redacted: false } } },
      {
        sequence: 5,
        event: {
          MessageAppended: {
            message: {
              Assistant: {
                content: [
                  { Text: { text: "relay 测试覆盖良好：慢连接注销与 1013 关闭都有行为测试。" } },
                ],
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

const allSummaries = () => workspaceSnapshot().workspaces.flatMap((group) => group.sessions);

// ---------------------------------------------------------------------------
// HTTP surface
// ---------------------------------------------------------------------------

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
      models: [
        {
          alias: "gpt-5-codex",
          provider: "openai",
          context_window: 272000,
          capabilities: ["reasoning"],
        },
        {
          alias: "claude-sonnet-4.5",
          provider: "anthropic",
          context_window: 200000,
          capabilities: [],
        },
        {
          alias: "kimi-k2",
          provider: "moonshot",
          context_window: 128000,
          capabilities: [],
        },
      ],
      permission_modes: ["ask", "auto", "yolo"],
      development_modes: ["normal", "plan", "goal"],
      sessions: allSummaries().filter((entry) => entry.workspace_label === "neo"),
    });
    return;
  }
  if (path === "/api/attachments" && method === "POST") {
    const body = await readBody(req);
    if (typeof body.mime !== "string" || typeof body.base64 !== "string") {
      json(res, { code: "invalid_request" }, 400);
      return;
    }
    const byteLen = Buffer.from(body.base64, "base64").length;
    json(res, { id: `att_${byteLen.toString(16)}`, mime: body.mime, byte_len: byteLen }, 201);
    return;
  }
  if (path === "/api/sessions" && method === "GET") {
    const needle = (url.searchParams.get("query") ?? "").toLowerCase();
    const scope = url.searchParams.get("scope") ?? "active";
    const items = allSummaries().filter((entry) => {
      const inScope = scope === "archived" ? entry.archived === true : entry.archived !== true;
      return inScope && (needle === "" || String(entry.title ?? "").toLowerCase().includes(needle));
    });
    json(res, { items });
    return;
  }
  const agentHistoryMatch = /^\/api\/sessions\/([^/]+)\/agents\/([^/]+)\/history$/.exec(path);
  if (agentHistoryMatch) {
    const history = agentHistory(agentHistoryMatch[1], agentHistoryMatch[2]);
    if (history) json(res, history);
    else json(res, { code: "not_found" }, 404);
    return;
  }
  const snapshotMatch = /^\/api\/sessions\/([^/]+)\/snapshot$/.exec(path);
  if (snapshotMatch) {
    const found = findSnapshot(snapshotMatch[1]);
    if (found) json(res, found.snapshot);
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
    const body = await readBody(req);
    json(res, {
      title: body.title ?? null,
      pinned: body.pinned ?? false,
      archived: body.archived ?? false,
    });
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
        ws.send(JSON.stringify(workspaceSnapshot()));
        return;
      }
      if (message.type === "watch_session") {
        const found = findSnapshot(message.session_id);
        if (!found) {
          ws.send(JSON.stringify({ code: "not_found" }));
          return;
        }
        ws.send(JSON.stringify({ type: "session_snapshot", snapshot: found.snapshot }));
        for (const envelope of found.after) {
          ws.send(JSON.stringify(envelope));
        }
      }
    });
  });
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(
    `mock server on http://127.0.0.1:${PORT} (big session ≈ ${bigCharCount()} chars of event payload)`,
  );
});
