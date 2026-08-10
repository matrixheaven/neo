/**
 * Subagent UI tests (R4 §5): agent-line state pill / pulse / elapsed, swarm
 * aggregate bar + member stagger + connector rows, and the drill-down panel —
 * lazy history load through the shared transcript rendering, Esc close with
 * focus restore, session-switch auto-close and the 404 error state.
 */

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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

  it("agent-line shows a pulsing dot, running pill and elapsed while running", async () => {
    const { socket } = await openSession1();
    const emit = emitter(socket);
    emit({ DelegateStarted: { turn: 1, agent: runningAgent() } });

    const button = await screen.findByRole("button", {
      name: /查看子代理详情：检查测试覆盖/,
    });
    const line = button.closest(".agent-line") as HTMLElement;
    expect(line.className).toContain("state-running");
    expect(line.querySelector(".pulse-dot")).not.toBeNull();
    const pill = line.querySelector(".agent-pill.st-running");
    expect(pill?.textContent).toContain("运行中");
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
      expect(finished.querySelector(".agent-pill.st-completed")?.textContent).toContain("已完成");
      expect(finished.querySelector(".agent-elapsed")?.textContent).toBe("7s");
    });
  });

  it("agent-line renders failure and timeout states as distinct pills", async () => {
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
    expect(failedLine.querySelector(".agent-pill.st-failed")?.textContent).toContain("失败");
    expect(timedOutLine.querySelector(".agent-pill.st-timed_out")?.textContent).toContain("超时");
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
    expect(
      members[0].querySelector(".agent-pill.st-completed")?.textContent,
    ).toContain("已完成");
  });

  it("opens the drill-down panel with the child transcript, Esc closes and focus returns", async () => {
    const user = userEvent.setup();
    const { socket } = await openSession1();
    const emit = emitter(socket);
    emit({ DelegateStarted: { turn: 1, agent: runningAgent() } });

    const lineButton = await screen.findByRole("button", {
      name: /查看子代理详情：检查测试覆盖/,
    });
    await user.click(lineButton);

    const dialog = await screen.findByRole("dialog", { name: /子代理详情：检查测试覆盖/ });
    await waitFor(() =>
      expect(
        recordedRequests.some(
          (entry) =>
            entry.method === "GET" &&
            entry.url === "/api/sessions/session_0001/agents/agent_02/history",
        ),
      ).toBe(true),
    );
    // Header: state pill, running lag note, token usage from the snapshot.
    expect(within(dialog).getByText("运行中面板显示截至上次落盘点的内容")).toBeTruthy();
    expect(within(dialog).getByText("token 16")).toBeTruthy();
    // Child transcript renders through the shared transcript components.
    expect(await within(dialog).findByText("子代理结论：relay 覆盖达标。")).toBeTruthy();
    expect(within(dialog).getByText("检查 relay 测试覆盖")).toBeTruthy();

    await user.keyboard("{Escape}");
    // Close is a reverse transition: the panel stays mounted with `.closing`
    // until the slide-out animation ends on the panel element itself.
    const panelEl = document.querySelector(".agent-panel") as HTMLElement;
    expect(panelEl.className).toContain("closing");
    expect(screen.queryByRole("dialog")).not.toBeNull();
    fireEvent.animationEnd(panelEl);
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(document.activeElement).toBe(lineButton);
  });

  it("close button runs the reverse animation before unmount", async () => {
    const user = userEvent.setup();
    const { socket } = await openSession1();
    const emit = emitter(socket);
    emit({ DelegateStarted: { turn: 1, agent: runningAgent() } });

    await user.click(
      await screen.findByRole("button", { name: /查看子代理详情：检查测试覆盖/ }),
    );
    const dialog = await screen.findByRole("dialog", { name: /子代理详情：检查测试覆盖/ });
    await user.click(within(dialog).getByRole("button", { name: "关闭子代理详情" }));
    // Marked closing but not yet unmounted; the animationend settles it.
    const panelEl = document.querySelector(".agent-panel") as HTMLElement;
    expect(panelEl.className).toContain("closing");
    expect(screen.queryByRole("dialog")).not.toBeNull();
    fireEvent.animationEnd(panelEl);
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("traps Tab and Shift+Tab inside the open panel", async () => {
    const user = userEvent.setup();
    const { socket } = await openSession1();
    const emit = emitter(socket);
    emit({ DelegateStarted: { turn: 1, agent: runningAgent() } });

    await user.click(
      await screen.findByRole("button", { name: /查看子代理详情：检查测试覆盖/ }),
    );
    const dialog = await screen.findByRole("dialog", { name: /子代理详情：检查测试覆盖/ });
    await within(dialog).findByText("子代理结论：relay 覆盖达标。");

    const focusables = Array.from(
      dialog.querySelectorAll<HTMLElement>(
        'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
      ),
    );
    expect(focusables.length).toBeGreaterThan(1);
    const first = focusables[0];
    const last = focusables[focusables.length - 1];

    // Shift+Tab on the first control wraps to the last; Tab on the last wraps
    // back to the first — focus never leaves the dialog.
    first.focus();
    await user.keyboard("{Shift>}{Tab}{/Shift}");
    expect(document.activeElement).toBe(last);
    await user.keyboard("{Tab}");
    expect(document.activeElement).toBe(first);
    for (let step = 0; step < focusables.length + 2; step++) {
      await user.keyboard("{Tab}");
      expect(dialog.contains(document.activeElement)).toBe(true);
    }
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
    const dialog = await screen.findByRole("dialog", { name: /子代理详情：relay 覆盖/ });
    await waitFor(() =>
      expect(
        recordedRequests.some(
          (entry) =>
            entry.method === "GET" &&
            entry.url === "/api/sessions/session_0001/agents/agent_10/history",
        ),
      ).toBe(true),
    );
    expect(await within(dialog).findByText("子代理结论：relay 覆盖达标。")).toBeTruthy();
  });

  it("closes the panel when switching sessions", async () => {
    const user = userEvent.setup();
    const { socket } = await openSession1();
    const emit = emitter(socket);
    emit({ DelegateStarted: { turn: 1, agent: runningAgent() } });

    await user.click(
      await screen.findByRole("button", { name: /查看子代理详情：检查测试覆盖/ }),
    );
    await screen.findByRole("dialog", { name: /子代理详情：检查测试覆盖/ });

    (await screen.findByText("并行格式化")).click();
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
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
          elapsed: { secs: 4, nanos: 0 },
        },
      },
    });

    await user.click(
      await screen.findByRole("button", { name: /查看子代理详情：无历史代理/ }),
    );
    const dialog = await screen.findByRole("dialog", { name: /子代理详情：无历史代理/ });
    expect(
      await within(dialog).findByText(/未找到该子代理的历史记录/),
    ).toBeTruthy();
  });
});
