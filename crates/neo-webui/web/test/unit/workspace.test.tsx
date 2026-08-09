/**
 * Workspace interaction tests: sidebar grouping and hover actions, one
 * shared context menu (right-click / Shift+F10) with rename/pin/archive,
 * resizer keyboard control, and composer behavior (Enter send, Shift+Enter
 * newline, IME guard, follow-up vs turn vs create).
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

async function renderReady() {
  const utils = renderApp();
  await screen.findByLabelText("会话列表");
  await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
  return { ...utils, socket: FakeWebSocket.instances[0] };
}

function selectSession(title: string) {
  screen.getByText(title).click();
}

describe("sidebar", () => {
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

  it("groups sessions as pinned / normal / archived with state text", async () => {
    const { socket } = await renderReady();
    socket.emit(asServerMessage(fixture.long_connection.workspace_snapshot));
    await screen.findByText("已置顶");
    expect(screen.getByText("会话", { selector: ".session-group-label" })).toBeTruthy();
    const pinnedGroup = screen.getByText("已置顶").parentElement as HTMLElement;
    expect(within(pinnedGroup).getByText("有界中继测试")).toBeTruthy();
    expect(within(pinnedGroup).getByText("等待回答")).toBeTruthy();
    const normalGroup = screen
      .getByText("会话", { selector: ".session-group-label" })
      .closest(".session-group") as HTMLElement;
    expect(within(normalGroup).getByText("并行格式化")).toBeTruthy();
    expect(within(normalGroup).getByText("运行中")).toBeTruthy();
  });

  it("hover pin action does not switch the session", async () => {
    const { socket } = await renderReady();
    socket.emit(asServerMessage(fixture.long_connection.workspace_snapshot));
    await screen.findByText("并行格式化");
    const pinButtons = screen.getAllByRole("button", { name: "置顶" });
    pinButtons[0].click();
    await waitFor(() =>
      expect(
        recordedRequests.some(
          (entry) =>
            entry.method === "PATCH" &&
            entry.url === "/api/sessions/session_0002" &&
            (entry.body as { pinned?: boolean }).pinned === true,
        ),
      ).toBe(true),
    );
    // Still on the new-session composer: no watch_session was sent.
    expect(screen.getByLabelText("输入消息")).toBeTruthy();
    expect(FakeWebSocket.instances[0].watchSessionIds()).toEqual([]);
  });

  it("opens one shared menu via Shift+F10, renames, and returns focus", async () => {
    const user = userEvent.setup();
    const { socket } = await renderReady();
    socket.emit(asServerMessage(fixture.long_connection.workspace_snapshot));
    const title = await screen.findByText("并行格式化");
    const row = title.closest(".session-row") as HTMLElement;
    const mainButton = within(row).getByRole("button", { name: /并行格式化/ });
    mainButton.focus();
    await user.keyboard("{Shift>}{F10}{/Shift}");
    const menu = await screen.findByRole("menu", { name: "会话操作" });
    expect(within(menu).getByRole("menuitem", { name: "重命名" })).toBeTruthy();
    expect(within(menu).getByRole("menuitem", { name: "置顶" })).toBeTruthy();
    expect(within(menu).getByRole("menuitem", { name: "归档" })).toBeTruthy();
    // Opening the menu did not switch the session.
    expect(FakeWebSocket.instances[0].watchSessionIds()).toEqual([]);

    within(menu).getByRole("menuitem", { name: "重命名" }).click();
    const input = await screen.findByLabelText("会话标题");
    await user.clear(input);
    await user.type(input, "新的标题{Enter}");
    await waitFor(() =>
      expect(
        recordedRequests.some(
          (entry) =>
            entry.method === "PATCH" &&
            entry.url === "/api/sessions/session_0002" &&
            (entry.body as { title?: string }).title === "新的标题",
        ),
      ).toBe(true),
    );
  });

  it("closes the menu with Escape and returns focus to the row", async () => {
    const user = userEvent.setup();
    const { socket } = await renderReady();
    socket.emit(asServerMessage(fixture.long_connection.workspace_snapshot));
    const title = await screen.findByText("并行格式化");
    const row = title.closest(".session-row") as HTMLElement;
    const mainButton = within(row).getByRole("button", { name: /并行格式化/ });
    mainButton.focus();
    await user.keyboard("{Shift>}{F10}{/Shift}");
    await screen.findByRole("menu", { name: "会话操作" });
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu", { name: "会话操作" })).toBeNull();
    expect(document.activeElement).toBe(mainButton);
  });

  it("adjusts the sidebar with ArrowLeft/ArrowRight and persists the width", async () => {
    const user = userEvent.setup();
    await renderReady();
    const resizer = screen.getByRole("separator", { name: "调整会话列表宽度" });
    resizer.focus();
    await user.keyboard("{ArrowRight}");
    await waitFor(() =>
      expect(screen.getByLabelText("会话列表").style.width).toBe("296px"),
    );
    expect(window.localStorage.getItem("neo-webui.sidebar-width")).toBe("296");
    await user.keyboard("{ArrowLeft}{ArrowLeft}");
    await waitFor(() =>
      expect(screen.getByLabelText("会话列表").style.width).toBe("264px"),
    );
  });

  it("searches titles on the server only", async () => {
    const user = userEvent.setup();
    const { socket } = await renderReady();
    socket.emit(asServerMessage(fixture.long_connection.workspace_snapshot));
    await screen.findByText("并行格式化");
    const search = screen.getByLabelText("搜索会话标题");
    await user.type(search, "中继");
    await waitFor(() =>
      expect(
        recordedRequests.some(
          (entry) =>
            entry.url.startsWith("/api/sessions?") &&
            entry.url.includes("query=%E4%B8%AD%E7%BB%A7"),
        ),
      ).toBe(true),
    );
    await screen.findByText("搜索结果");
  });
});

describe("composer", () => {
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

  it("sends on Enter, newline on Shift+Enter, never during IME composition", async () => {
    const user = userEvent.setup();
    await renderReady();
    const input = screen.getByLabelText("输入消息") as HTMLTextAreaElement;

    await user.type(input, "hello{Shift>}{Enter}{/Shift}world");
    expect(input.value).toBe("hello\nworld");
    expect(recordedRequests.some((entry) => entry.url === "/api/sessions" && entry.method === "POST")).toBe(false);

    // IME composition must not send.
    await user.clear(input);
    await user.type(input, "组合输入");
    input.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
    await user.keyboard("{Enter}");
    expect(recordedRequests.some((entry) => entry.url === "/api/sessions" && entry.method === "POST")).toBe(false);
    input.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true }));

    await user.keyboard("{Enter}");
    await waitFor(() =>
      expect(
        recordedRequests.some(
          (entry) =>
            entry.url === "/api/sessions" &&
            entry.method === "POST" &&
            (entry.body as { message?: string }).message === "组合输入",
        ),
      ).toBe(true),
    );
  });

  it("creates a session from the centered composer and switches to it", async () => {
    const user = userEvent.setup();
    const { socket } = await renderReady();
    const input = screen.getByLabelText("输入消息");
    await user.type(input, "第一条消息{Enter}");
    await waitFor(() =>
      expect(
        recordedRequests.some((entry) => entry.url === "/api/sessions" && entry.method === "POST"),
      ).toBe(true),
    );
    // After creation the session is selected and watched; the response
    // sequence was not treated as a transcript watermark.
    await waitFor(() => expect(socket.watchSessionIds()).toEqual(["session_0001"]));
  });

  it("routes idle sends to turns and running sends to follow_up input", async () => {
    const user = userEvent.setup();
    const { socket } = await renderReady();
    socket.emit(asServerMessage(fixture.long_connection.workspace_snapshot));
    selectSession("并行格式化");
    await waitFor(() => expect(socket.watchSessionIds()).toEqual(["session_0002"]));

    // Snapshot: session_0002 is running with turn_02 → follow_up.
    socket.emit({ type: "session_snapshot", snapshot: fixture.sessions[1].snapshot });
    await screen.findByText("另一个会话并行跑格式化。");
    const input = await screen.findByLabelText("输入消息");
    await user.type(input, "排队一条{Enter}");
    await waitFor(() =>
      expect(
        recordedRequests.some(
          (entry) =>
            entry.url === "/api/sessions/session_0002/input" &&
            (entry.body as { delivery?: string }).delivery === "follow_up",
        ),
      ).toBe(true),
    );

    // Turn the session idle: sends go to /turns.
    socket.emit({
      type: "session_state",
      stream_id: fixture.stream_id,
      session_id: "session_0002",
      sequence: 4,
      event: { phase: "idle", waiting_approval: false, waiting_question: false, current_turn_id: null },
    });
    await user.type(input, "新回合{Enter}");
    await waitFor(() =>
      expect(
        recordedRequests.some(
          (entry) =>
            entry.url === "/api/sessions/session_0002/turns" &&
            (entry.body as { message?: string }).message === "新回合",
        ),
      ).toBe(true),
    );
  });

  it("keeps stop and steer as separate actions", async () => {
    const user = userEvent.setup();
    const { socket } = await renderReady();
    socket.emit(asServerMessage(fixture.long_connection.workspace_snapshot));
    selectSession("并行格式化");
    socket.emit({ type: "session_snapshot", snapshot: fixture.sessions[1].snapshot });
    await screen.findByText("另一个会话并行跑格式化。");

    await user.click(screen.getByRole("button", { name: "停止当前回合" }));
    await waitFor(() =>
      expect(
        recordedRequests.some(
          (entry) =>
            entry.url === "/api/sessions/session_0002/cancel" &&
            (entry.body as { turn_id?: string }).turn_id === "turn_02",
        ),
      ).toBe(true),
    );

    const input = screen.getByLabelText("输入消息");
    await user.type(input, "立刻改方向");
    await user.click(screen.getByRole("button", { name: "立即引导当前回合" }));
    await waitFor(() =>
      expect(
        recordedRequests.some(
          (entry) =>
            entry.url === "/api/sessions/session_0002/input" &&
            (entry.body as { delivery?: string }).delivery === "steer",
        ),
      ).toBe(true),
    );
  });

  it("keeps the draft when session creation fails", async () => {
    const user = userEvent.setup();
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
        const url = typeof input === "string" ? input : input.toString();
        if (url === "/api/sessions" && init?.method === "POST") {
          return Promise.resolve(
            new Response(JSON.stringify({ code: "session_busy" }), {
              status: 409,
              headers: { "content-type": "application/json" },
            }),
          );
        }
        return mockFetch(input, init);
      }),
    );
    await renderReady();
    const input = screen.getByLabelText("输入消息") as HTMLTextAreaElement;
    await user.type(input, "保留这段草稿{Enter}");
    await screen.findByText(/已变化|失败/);
    // The draft survives the failed create; no session was selected.
    expect(
      (screen.getByLabelText("输入消息") as HTMLTextAreaElement).value,
    ).toBe("保留这段草稿");
    expect(FakeWebSocket.instances[0].watchSessionIds()).toEqual([]);
    expect(input).toBeTruthy();
  });

  it("sends only one create request while one is in flight", async () => {
    const user = userEvent.setup();
    let resolveCreate: (value: Response) => void = () => {};
    const gate = new Promise<Response>((resolve) => {
      resolveCreate = resolve;
    });
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
        const url = typeof input === "string" ? input : input.toString();
        if (url === "/api/sessions" && init?.method === "POST") {
          recordedRequests.push({
            method: "POST",
            url,
            body: init.body ? JSON.parse(String(init.body)) : undefined,
          });
          return gate;
        }
        return mockFetch(input, init);
      }),
    );
    const { socket } = await renderReady();
    const input = screen.getByLabelText("输入消息");
    await user.type(input, "第一条{Enter}");
    // Second submit while the create is in flight must be a no-op.
    await user.keyboard("{Enter}");
    await user.click(screen.getByRole("button", { name: "发送" }));
    const creates = recordedRequests.filter(
      (entry) => entry.url === "/api/sessions" && entry.method === "POST",
    );
    expect(creates).toHaveLength(1);
    resolveCreate(
      new Response(
        JSON.stringify({
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
        }),
        { status: 201, headers: { "content-type": "application/json" } },
      ),
    );
    await waitFor(() => expect(socket.watchSessionIds()).toEqual(["session_0001"]));
  });
});
