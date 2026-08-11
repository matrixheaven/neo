/**
 * Subagent UI tests (R4 §5): agent-line state pill / pulse / elapsed, swarm
 * aggregate bar + member stagger + connector rows, and the drill-down panel —
 * lazy history load through the shared transcript rendering, Esc close with
 * focus restore, session-switch auto-close and the 404 error state.
 */

import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "../../src/app";
import { AppProvider } from "../../src/state/store";
import {
  FakeWebSocket,
  fixture,
  mockFetch,
  recordedRequests,
  resetHarness,
} from "./harness";
import { asServerMessage } from "./fixture";

function renderApp() {
  return render(React.createElement(AppProvider, null, React.createElement(App)));
}

const session1 = fixture.sessions[0];

async function openSession1() {
  const utils = renderApp();
  await screen.findByLabelText("会话列表");
  await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
  const socket = FakeWebSocket.instances[0];
  socket.emit(asServerMessage(fixture.long_connection.workspace_snapshot));
  (await screen.findByText("有界中继测试")).click();
  await waitFor(() => expect(socket.watchSessionIds()).toEqual(["session_0001"]));
  socket.emit({ type: "session_snapshot", snapshot: session1.snapshot });
  await screen.findByText("检查有界中继的行为测试并修复慢连接。");
  return { ...utils, socket };
}

function emitter(socket: FakeWebSocket) {
  let sequence = 9;
  return (event: unknown) => {
    socket.emit({
      type: "session_event",
      stream_id: fixture.stream_id,
      session_id: "session_0001",
      sequence: sequence++,
      event: event as never,
    });
  };
}

function runningAgent() {
  return {
    id: "agent_02",
    display_name: "explorer",
    state: "running",
    task: "检查 relay 测试覆盖",
    task_title: "检查测试覆盖",
    elapsed: { secs: 5, nanos: 0 },
    latest_text: "relay 覆盖检查进行中",
    token_count: 16,
  };
}

function emitSwarm(emit: (event: unknown) => void) {
  emit({
    DelegateSwarmStarted: {
      turn: 1,
      swarm: {
        swarm_id: "swarm_01",
        description: "并行检查覆盖",
        state: "running",
        max_concurrency: 2,
        aggregate: {
          total: 2,
          queued: 0,
          running: 1,
          completed: 1,
          failed: 0,
          cancelled: 0,
          timed_out: 0,
        },
        children: [
          {
            item_index: 0,
            item: "检查 relay 测试覆盖",
            agent: {
              id: "agent_10",
              display_name: "explorer",
              state: "completed",
              task_title: "relay 覆盖",
              elapsed: { secs: 3, nanos: 0 },
            },
          },
          {
            item_index: 1,
            item: "检查 server 测试覆盖",
            agent: {
              id: "agent_11",
              display_name: "explorer",
              state: "running",
              task_title: "server 覆盖",
              elapsed: { secs: 1, nanos: 0 },
            },
          },
        ],
      },
    },
  });
}

describe("subagent UI (R4)", () => {
  beforeEach(() => {
    resetHarness();
    vi.stubGlobal("fetch", vi.fn(mockFetch));
    vi.stubGlobal("WebSocket", FakeWebSocket);
    window.history.replaceState(null, "", "/");
  });

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it("agent-line shows a pulsing dot, left status icon and elapsed while running", async () => {
    const { socket } = await openSession1();
    const emit = emitter(socket);
    emit({ DelegateStarted: { turn: 1, agent: runningAgent() } });

    const button = await screen.findByRole("button", {
      name: /查看子代理详情：检查测试覆盖/,
    });
    const line = button.closest(".agent-line") as HTMLElement;
    expect(line.className).toContain("state-running");
    expect(line.querySelector(".pulse-dot")).not.toBeNull();
    expect(line.querySelector('.tl-result-ic[data-status-icon="running"]')).not.toBeNull();
    expect(line.querySelector(".agent-pill")).toBeNull();
    expect(line.querySelector(".agent-elapsed")?.textContent).toBe("5s");
    expect(line.querySelector(".tl-mono")?.textContent).toBe("relay 覆盖检查进行中");

    emit({
      DelegateFinished: {
        turn: 1,
        agent: {
          ...runningAgent(),
          state: "completed",
          terminal_reason: "completed",
          elapsed: { secs: 7, nanos: 0 },
          latest_text: null,
        },
      },
    });
    await waitFor(() => {
      const finished = button.closest(".agent-line") as HTMLElement;
      expect(finished.querySelector(".pulse-dot")).toBeNull();
      expect(finished.querySelector('.tl-result-ic[data-status-icon="finished"]')).not.toBeNull();
      expect(finished.querySelector(".agent-pill")).toBeNull();
      expect(finished.querySelector(".agent-elapsed")?.textContent).toBe("7s");
    });
  });

  it("agent-line renders failure and timeout states with a left failure icon", async () => {
    const { socket } = await openSession1();
    const emit = emitter(socket);
    emit({
      DelegateStarted: {
        turn: 1,
        agent: {
          id: "agent_20",
          display_name: "worker",
          state: "failed",
          task_title: "失败的代理",
          elapsed: { secs: 2, nanos: 0 },
        },
      },
    });
    emit({
      DelegateStarted: {
        turn: 1,
        agent: {
          id: "agent_21",
          display_name: "worker",
          state: "timed_out",
          task_title: "超时的代理",
          elapsed: { secs: 30, nanos: 0 },
        },
      },
    });

    const failed = await screen.findByRole("button", { name: /查看子代理详情：失败的代理/ });
    const timedOut = await screen.findByRole("button", { name: /查看子代理详情：超时的代理/ });
    const failedLine = failed.closest(".agent-line") as HTMLElement;
    const timedOutLine = timedOut.closest(".agent-line") as HTMLElement;
    expect(failedLine.querySelector('.tl-result-ic[data-status-icon="failed"]')).not.toBeNull();
    expect(timedOutLine.querySelector('.tl-result-ic[data-status-icon="failed"]')).not.toBeNull();
    expect(failedLine.querySelector(".agent-pill")).toBeNull();
    expect(timedOutLine.querySelector(".agent-pill")).toBeNull();
    expect(timedOutLine.querySelector(".agent-elapsed")?.textContent).toBe("30s");
  });

  it("swarm header carries the aggregate bar and members stagger in with connector rows", async () => {
    const { socket } = await openSession1();
    const emit = emitter(socket);
    emitSwarm(emit);

    const head = await screen.findByRole("button", { name: /并行子代理 并行检查覆盖/ });
    const block = head.closest(".swarm-block") as HTMLElement;
    const tail = block.querySelector(".line-tail");
    expect(tail?.textContent).toContain("完成 1/2");

    const bar = block.querySelector(".swarm-bar") as HTMLElement;
    expect(bar.getAttribute("role")).toBe("progressbar");
    expect(bar.getAttribute("aria-valuenow")).toBe("1");
    expect(bar.getAttribute("aria-valuemax")).toBe("2");
    const fill = block.querySelector(".swarm-bar-fill") as HTMLElement;
    expect(fill.style.width).toBe("50%");

    const members = block.querySelectorAll(".swarm-member");
    expect(members).toHaveLength(2);
    expect((members[0] as HTMLElement).style.getPropertyValue("--stagger")).toBe("0");
    expect((members[1] as HTMLElement).style.getPropertyValue("--stagger")).toBe("1");
    // Each member row is a full agent-line that can open the panel on its own.
    expect(members[0].querySelector(".agent-line")).not.toBeNull();
    expect(members[1].querySelector(".agent-line .pulse-dot")).not.toBeNull();
    expect(members[0].querySelector('.tl-result-ic[data-status-icon="finished"]')).not.toBeNull();
    expect(members[0].querySelector(".agent-pill")).toBeNull();
  });

  it("auto-opens the information column without stealing focus and groups active agents", async () => {
    const { socket } = await openSession1();
    const composer = screen.getByLabelText("输入消息");
    composer.focus();
    const emit = emitter(socket);
    emit({ DelegateStarted: { turn: 1, agent: runningAgent() } });

    const panel = screen.getByLabelText("会话信息区");
    await waitFor(() => expect(panel.classList.contains("open")).toBe(true));
    expect(panel.getAttribute("aria-hidden")).toBe("false");
    expect(document.activeElement).toBe(composer);
    expect(within(panel).getByRole("tab", { name: "Subagents" }).getAttribute("aria-selected"))
      .toBe("true");
    expect(within(panel).getByRole("tab", { name: "Review" }).getAttribute("aria-selected"))
      .toBe("false");
    expect(within(panel).getByRole("heading", { name: "Active 1" })).toBeTruthy();
    expect(within(panel).getByRole("heading", { name: "Done 0" })).toBeTruthy();
    expect(within(panel).getByRole("button", { name: /查看子代理：检查测试覆盖/ })).toBeTruthy();
    expect(screen.queryByRole("dialog", { name: /子代理详情/ })).toBeNull();
  });

  it("opens a fixed summary without blocking the transcript and routes changed files to Review", async () => {
    const user = userEvent.setup();
    const { socket } = await openSession1();
    const emit = emitter(socket);
    emit({ DelegateStarted: { turn: 2, agent: runningAgent() } });
    emit({
      ToolExecutionFinished: {
        turn: 2,
        id: "summary_file",
        name: "edit",
        result: {
          content: "updated",
          is_error: false,
          details: {
            changes: [{
              path: "src/summary.ts",
              status: "committed",
              added: 1,
              removed: 0,
              diff: "@@ -1 +1,2 @@\n existing\n+summary",
            }],
          },
        },
      },
    });
    emit({ MessageStarted: { turn: 2, id: "summary_result" } });
    emit({ TextDelta: { turn: 2, text: "已整理摘要文件。" } });
    emit({ MessageFinished: { turn: 2, id: "summary_result", stop_reason: "EndTurn" } });

    const toggle = screen.getByRole("button", { name: "切换固定摘要" });
    expect(toggle.getAttribute("aria-controls")).toBe("fixed-summary");
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    await user.click(toggle);

    const summary = screen.getByLabelText("固定摘要");
    expect(summary.classList.contains("open")).toBe(true);
    expect(summary.getAttribute("aria-hidden")).toBe("false");
    expect(document.querySelector(".session-view")?.classList.contains("fixed-summary-open")).toBe(true);
    expect(within(summary).getByRole("button", { name: /查看子代理，共 1 个/ })).toBeTruthy();
    expect(within(summary).getByRole("button", { name: "查看 1 个修改文件" })).toBeTruthy();

    await user.click(within(summary).getByRole("button", { name: "关闭固定摘要" }));
    await waitFor(() => expect(summary.getAttribute("aria-hidden")).toBe("true"));
    expect(document.activeElement).toBe(toggle);

    await user.click(toggle);
    await user.click(within(summary).getByRole("button", { name: "查看 1 个修改文件" }));
    const panel = screen.getByLabelText("会话信息区");
    expect(summary.getAttribute("aria-hidden")).toBe("true");
    expect(within(panel).getByRole("tab", { name: "Review" }).getAttribute("aria-selected"))
      .toBe("true");
  });

  it("keeps a user-selected Review tab during later agent activity", async () => {
    const user = userEvent.setup();
    const { socket } = await openSession1();
    const emit = emitter(socket);
    emit({ DelegateStarted: { turn: 1, agent: runningAgent() } });
    const panel = screen.getByLabelText("会话信息区");
    await waitFor(() => expect(panel.classList.contains("open")).toBe(true));
    await user.click(within(panel).getByRole("tab", { name: "Review" }));

    emit({
      DelegateUpdated: {
        turn: 1,
        agent: { ...runningAgent(), latest_text: "继续检查 relay 覆盖" },
      },
    });
    await screen.findByRole("button", { name: /查看子代理详情：检查测试覆盖/ });
    await waitFor(() => expect(
      within(panel).getByRole("tab", { name: "Review" }).getAttribute("aria-selected"),
    ).toBe("true"));
  });

  it("returns focus to the topbar when a roster trigger no longer exists", async () => {
    const user = userEvent.setup();
    const { socket } = await openSession1();
    const emit = emitter(socket);
    emit({ DelegateStarted: { turn: 1, agent: runningAgent() } });
    const panel = screen.getByLabelText("会话信息区");
    const topbarToggle = document.querySelector(".information-toggle") as HTMLElement;
    const rosterButton = await within(panel).findByRole("button", {
      name: /查看子代理：检查测试覆盖/,
    });
    await user.click(rosterButton);
    await within(panel).findByText("子代理结论：relay 覆盖达标。");

    await user.click(within(panel).getByRole("button", { name: "关闭会话信息区" }));
    await waitFor(() => expect(panel.getAttribute("aria-hidden")).toBe("true"));
    expect(document.activeElement).toBe(topbarToggle);
  });

  it("opens shared child history, switches tabs, then closes and restores focus", async () => {
    const user = userEvent.setup();
    const { socket } = await openSession1();
    const emit = emitter(socket);
    emit({ DelegateStarted: { turn: 1, agent: runningAgent() } });

    const lineButton = await screen.findByRole("button", {
      name: /查看子代理详情：检查测试覆盖/,
    });
    await user.click(lineButton);

    const panel = screen.getByLabelText("会话信息区");
    await waitFor(() => expect(panel.classList.contains("open")).toBe(true));
    await waitFor(() =>
      expect(
        recordedRequests.some(
          (entry) =>
            entry.method === "GET" &&
            entry.url === "/api/sessions/session_0001/agents/agent_02/history",
        ),
      ).toBe(true),
    );
    expect(within(panel).getByText("运行中仅显示上次落盘快照；进度状态仍来自当前会话。")).toBeTruthy();
    expect(within(panel).getByText("token 16")).toBeTruthy();
    expect(await within(panel).findByText("子代理结论：relay 覆盖达标。")).toBeTruthy();
    expect(within(panel).getByText("检查 relay 测试覆盖")).toBeTruthy();
    expect(within(panel).getByText("已编辑 1 个文件")).toBeTruthy();
    await user.click(within(panel).getByRole("button", { name: /展开工作过程/ }));
    expect(within(panel).getByRole("button", { name: /展开思考，状态：已完成/ })).toBeTruthy();
    expect(within(panel).getByRole("button", { name: /编辑 src\/relay.ts/ })).toBeTruthy();

    await user.click(within(panel).getByRole("tab", { name: "Review" }));
    expect(within(panel).getByRole("tab", { name: "Review" }).getAttribute("aria-selected"))
      .toBe("true");
    expect(within(panel).getByText("从最终修改文件列表选择一个文件开始 Review。")).toBeTruthy();
    await user.click(within(panel).getByRole("tab", { name: "Subagents" }));
    await user.click(within(panel).getByRole("button", { name: "关闭会话信息区" }));
    await waitFor(() => expect(panel.getAttribute("aria-hidden")).toBe("true"));
    expect(panel.classList.contains("open")).toBe(false);
    expect(document.activeElement).toBe(lineButton);
  });

  it("opens the panel from a swarm member row", async () => {
    const user = userEvent.setup();
    const { socket } = await openSession1();
    const emit = emitter(socket);
    emitSwarm(emit);

    const memberButton = await screen.findByRole("button", {
      name: /查看子代理详情：relay 覆盖/,
    });
    await user.click(memberButton);
    const panel = screen.getByLabelText("会话信息区");
    await waitFor(() => expect(panel.classList.contains("open")).toBe(true));
    await waitFor(() =>
      expect(
        recordedRequests.some(
          (entry) =>
            entry.method === "GET" &&
            entry.url === "/api/sessions/session_0001/agents/agent_10/history",
        ),
      ).toBe(true),
    );
    expect(await within(panel).findByText("子代理结论：relay 覆盖达标。")).toBeTruthy();
  });

  it("closes the panel when switching sessions", async () => {
    const user = userEvent.setup();
    const { socket } = await openSession1();
    const emit = emitter(socket);
    emit({ DelegateStarted: { turn: 1, agent: runningAgent() } });

    await user.click(
      await screen.findByRole("button", { name: /查看子代理详情：检查测试覆盖/ }),
    );
    const panel = screen.getByLabelText("会话信息区");
    await waitFor(() => expect(panel.getAttribute("aria-hidden")).toBe("false"));

    (await screen.findByText("并行格式化")).click();
    await waitFor(() => expect(panel.getAttribute("aria-hidden")).toBe("true"));
    expect(panel.classList.contains("open")).toBe(false);
  });

  it("shows a non-sensitive error when the agent history is missing (404)", async () => {
    const user = userEvent.setup();
    const { socket } = await openSession1();
    const emit = emitter(socket);
    emit({
      DelegateStarted: {
        turn: 1,
        agent: {
          id: "agent_missing",
          display_name: "worker",
          state: "completed",
          task_title: "无历史代理",
          latest_text: "快照结论：已完成静态检查。",
          elapsed: { secs: 4, nanos: 0 },
        },
      },
    });

    await user.click(
      await screen.findByRole("button", { name: /查看子代理详情：无历史代理/ }),
    );
    const panel = screen.getByLabelText("会话信息区");
    expect(
      await within(panel).findByText(/未找到该子代理的逐条历史；当前仅显示代理结果快照/),
    ).toBeTruthy();
    expect(within(panel).getByText("快照结论：已完成静态检查。")).toBeTruthy();
  });
});
