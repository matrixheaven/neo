/**
 * Workspace interaction tests: sidebar workspace grouping (R5) — current
 * group expanded with a "+" new-session button, other groups collapsed,
 * cross-workspace pinned section deduped out of groups, archived sessions
 * behind a collapsed entry, compact five-row project lists, running spinner
 * and waiting badges, rAF-
 * throttled resizer drag, keyboard width control, one shared context menu
 * (right-click / Shift+F10) that never switches the session — plus composer
 * behavior (Enter send, Shift+Enter newline, IME guard, follow-up vs turn vs
 * create).
 */

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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

  it("groups sessions by workspace: current expanded, others collapsed, pinned deduped", async () => {
    const { socket } = await renderReady();
    socket.emit(asServerMessage(fixture.long_connection.workspace_snapshot));

    // Pinned section surfaces the pinned session once, cross-workspace.
    const pinnedGroup = (await screen.findByText("已置顶")).parentElement as HTMLElement;
    expect(within(pinnedGroup).getByText("有界中继测试")).toBeTruthy();

    // Current workspace group is expanded with a "+" new-session button and a
    // session count; the pinned session does not repeat inside it.
    const neoGroup = screen.getByRole("group", { name: "neo" });
    const neoToggle = within(neoGroup).getByRole("button", { name: /neo/ });
    expect(neoToggle.getAttribute("aria-expanded")).toBe("true");
    expect(neoToggle.querySelector(".session-group-folder")).not.toBeNull();
    // The count matches the visible rows: the pinned session lives in the
    // Pinned section, so only the running row counts here.
    expect(within(neoToggle).getByText("1")).toBeTruthy();
    expect(within(neoGroup).getByRole("button", { name: "新会话" })).toBeTruthy();
    const runningRow = within(neoGroup).getByText("并行格式化").closest(".session-row");
    expect(runningRow?.querySelector(".session-activity .spin")).not.toBeNull();
    expect(within(neoGroup).queryByText("有界中继测试")).toBeNull();

    // Other workspaces are collapsed by default: their rows are not rendered.
    const playgroundGroup = screen.getByRole("group", { name: "playground" });
    const playgroundToggle = within(playgroundGroup).getByRole("button", { name: /playground/ });
    expect(playgroundToggle.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByText("原型脚本调试")).toBeNull();
  });

  it("expands and collapses workspace groups via the header toggle", async () => {
    const user = userEvent.setup();
    const { socket } = await renderReady();
    socket.emit(asServerMessage(fixture.long_connection.workspace_snapshot));
    await screen.findByText("并行格式化");

    // Collapsed group expands to show its sessions.
    const playgroundToggle = screen.getByRole("button", { name: /playground/ });
    await user.click(playgroundToggle);
    expect(playgroundToggle.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByText("原型脚本调试")).toBeTruthy();

    // The current group collapses too; its rows leave the DOM.
    const neoToggle = screen.getByRole("button", { name: /neo/ });
    await user.click(neoToggle);
    expect(neoToggle.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByText("并行格式化")).toBeNull();
  });

  it("removes a project by archiving its sessions without deleting them", async () => {
    const user = userEvent.setup();
    const { socket } = await renderReady();
    socket.emit(asServerMessage(fixture.long_connection.workspace_snapshot));

    const group = await screen.findByRole("group", { name: "neo" });
    await user.click(within(group).getByRole("button", { name: "项目操作" }));
    const menu = await screen.findByRole("menu", { name: "neo 项目操作" });
    await user.click(within(menu).getByRole("menuitem", { name: "移除项目" }));

    expect(screen.queryByRole("group", { name: "neo" })).toBeNull();
    await waitFor(() =>
      expect(
        recordedRequests.filter(
          (entry) =>
            entry.method === "PATCH" &&
            entry.url.startsWith("/api/sessions/") &&
            (entry.body as { archived?: boolean }).archived === true,
        ),
      ).toHaveLength(2),
    );
  });

  it("adds a project from its local directory path", async () => {
    const user = userEvent.setup();
    await renderReady();
    await user.click(screen.getByRole("button", { name: "添加项目" }));
    await user.type(screen.getByLabelText("项目文件夹"), "/tmp/added");
    await user.click(screen.getByRole("button", { name: "添加", exact: true }));
    await waitFor(() =>
      expect(
        recordedRequests.some(
          (entry) =>
            entry.url === "/api/workspaces" &&
            entry.method === "POST" &&
            (entry.body as { path?: string }).path === "/tmp/added",
        ),
      ).toBe(true),
    );
    expect(await screen.findByRole("group", { name: "added" })).toBeTruthy();
  });

  it("keeps workspace expansion while the selected session loads", async () => {
    const user = userEvent.setup();
    const { socket } = await renderReady();
    socket.emit(asServerMessage(fixture.long_connection.workspace_snapshot));

    const playgroundToggle = await screen.findByRole("button", { name: /playground/ });
    await user.click(playgroundToggle);
    await user.click(screen.getByText("原型脚本调试"));

    await waitFor(() => expect(socket.watchSessionIds()).toContain("session_0003"));
    expect(screen.getByLabelText("会话列表")).toBeTruthy();
    expect(playgroundToggle.getAttribute("aria-expanded")).toBe("true");
    expect(
      within(screen.getByRole("group", { name: "playground" })).getByText("原型脚本调试"),
    ).toBeTruthy();
  });

  it("tucks archived sessions behind a collapsed per-group entry", async () => {
    const user = userEvent.setup();
    const { socket } = await renderReady();
    socket.emit({
      type: "workspace_snapshot",
      stream_id: "ws_archived_test",
      workspace_sequence: 0,
      workspaces: [
        {
          id: "workspace_neo",
          label: "neo",
          current: true,
          sessions: [
            {
              session_id: "session_live",
              title: "活跃会话",
              updated_at: "2026-08-09T10:00:00+00:00",
              pinned: false,
              archived: false,
              state: "idle",
              workspace_label: "neo",
            },
            {
              session_id: "session_old",
              title: "旧会话",
              updated_at: "2026-08-08T10:00:00+00:00",
              pinned: false,
              archived: true,
              state: "idle",
              workspace_label: "neo",
            },
          ],
        },
      ],
    });
    await screen.findByText("活跃会话");
    // Archived rows are hidden until the "已归档 n" entry is opened; the group
    // count covers live sessions only.
    expect(screen.queryByText("旧会话")).toBeNull();
    const neoToggle = screen.getByRole("button", { name: /neo/ });
    expect(within(neoToggle).getByText("1")).toBeTruthy();
    const archivedToggle = screen.getByRole("button", { name: /已归档 1/ });
    expect(archivedToggle.getAttribute("aria-expanded")).toBe("false");
    await user.click(archivedToggle);
    expect(archivedToggle.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByText("旧会话")).toBeTruthy();
  });

  it("keeps session rows single-line and puts details in their hover tooltip", async () => {
    const { socket } = await renderReady();
    socket.emit({
      type: "workspace_snapshot",
      stream_id: "ws_status_test",
      workspace_sequence: 0,
      workspaces: [
        {
          id: "workspace_neo",
          label: "neo",
          current: true,
          sessions: [
            {
              session_id: "session_idle",
              title: "空闲会话",
              updated_at: "2026-08-09T10:00:00+00:00",
              pinned: false,
              archived: false,
              state: "idle",
              workspace_label: "neo",
            },
            {
              session_id: "session_running",
              title: "运行会话",
              updated_at: "2026-08-09T10:01:00+00:00",
              pinned: false,
              archived: false,
              state: "running",
              workspace_label: "neo",
            },
            {
              session_id: "session_waiting",
              title: "等待会话",
              updated_at: "2026-08-09T10:02:00+00:00",
              pinned: false,
              archived: false,
              state: "waiting_question",
              workspace_label: "neo",
            },
            {
              session_id: "session_failed",
              title: "失败会话",
              updated_at: "2026-08-09T10:03:00+00:00",
              pinned: false,
              archived: false,
              state: "failed",
              workspace_label: "neo",
            },
          ],
        },
      ],
    });
    const runningRow = (await screen.findByText("运行会话")).closest(".session-row") as HTMLElement;
    expect(runningRow.querySelector(".spin")).not.toBeNull();
    expect(runningRow.querySelector(".pulse-dot")).toBeNull();
    expect(within(runningRow).queryByText("运行中")).toBeNull();
    expect(runningRow.querySelector(".session-meta")).toBeNull();
    expect(runningRow.querySelector(".session-time")).toBeNull();
    const idleRow = screen.getByText("空闲会话").closest(".session-row") as HTMLElement;
    expect(within(idleRow).queryByText("空闲")).toBeNull();
    expect(idleRow.querySelector(".session-activity")).not.toBeNull();
    expect(idleRow.querySelector(".session-activity")?.childElementCount).toBe(0);
    const idleMain = idleRow.querySelector(".session-main") as HTMLButtonElement;
    expect(idleMain.title).toContain("空闲会话");
    expect(idleMain.title).toContain("工作区：neo");
    expect(idleMain.title).toContain("状态：空闲");
    expect(idleMain.title).toContain("更新时间：");
    const waitingRow = screen.getByText("等待会话").closest(".session-row") as HTMLElement;
    const badge = waitingRow.querySelector(".session-badge") as HTMLElement;
    expect(badge).not.toBeNull();
    expect(badge.textContent).toBe("等待回答");
    expect(badge.closest(".session-title-row")).not.toBeNull();
    expect(screen.getByText("失败", { selector: ".session-state" })).toBeTruthy();
  });

  it("shows five project sessions at a time, expands by five, then collapses", async () => {
    const user = userEvent.setup();
    const { socket } = await renderReady();
    socket.emit({
      type: "workspace_snapshot",
      stream_id: "ws_page_test",
      workspace_sequence: 0,
      workspaces: [
        {
          id: "workspace_neo",
          label: "neo",
          current: true,
          sessions: Array.from({ length: 12 }, (_, index) => ({
            session_id: `session_${index}`,
            title: `会话 ${String(index + 1).padStart(2, "0")}`,
            updated_at: `2026-08-09T10:${String(index).padStart(2, "0")}:00+00:00`,
            pinned: false,
            archived: false,
            state: "idle",
            workspace_label: "neo",
          })),
        },
      ],
    });
    await screen.findByText("会话 12");
    const neoGroup = screen.getByRole("group", { name: "neo" });
    expect(within(neoGroup).getAllByRole("listitem")).toHaveLength(5);
    expect(screen.queryByText("会话 07")).toBeNull();
    expect(neoGroup.querySelector(".session-group-header .lucide-folder-open")).not.toBeNull();
    expect(neoGroup.querySelector(".session-group-header .session-group-caret")).toBeNull();

    await user.click(screen.getByRole("button", { name: "展示更多" }));
    expect(within(neoGroup).getAllByRole("listitem")).toHaveLength(10);
    expect(screen.getByText("会话 07")).toBeTruthy();
    expect(screen.queryByText("会话 01")).toBeNull();
    expect(screen.getByRole("button", { name: "收起" })).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "展示更多" }));
    expect(within(neoGroup).getAllByRole("listitem")).toHaveLength(12);
    expect(screen.getByText("会话 01")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "展示更多" })).toBeNull();

    await user.click(screen.getByRole("button", { name: "收起" }));
    expect(within(neoGroup).getAllByRole("listitem")).toHaveLength(5);
    expect(screen.queryByText("会话 07")).toBeNull();
    expect(screen.getByRole("button", { name: "展示更多" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "收起" })).toBeNull();

    await user.click(within(neoGroup).getByRole("button", { name: /neo/ }));
    expect(neoGroup.querySelector(".session-group-header .lucide-folder-closed")).not.toBeNull();
  });

  it("uses the top-left button to collapse only the desktop sidebar", async () => {
    const user = userEvent.setup();
    await renderReady();
    const sidebar = screen.getByLabelText("会话列表");
    const toggle = screen.getByRole("button", { name: "收起会话列表" });
    expect(toggle.getAttribute("aria-expanded")).toBe("true");

    await user.click(toggle);
    expect(sidebar.classList.contains("sidebar-collapsed")).toBe(true);
    expect(document.querySelector(".app-body")?.classList.contains("sidebar-collapsed")).toBe(true);
    expect(document.querySelector(".drawer-scrim")).toBeNull();
    const expand = screen.getByRole("button", { name: "展开会话列表" });
    await user.click(expand);
    expect(sidebar.classList.contains("sidebar-collapsed")).toBe(false);
    expect(document.querySelector(".app-body")?.classList.contains("sidebar-collapsed")).toBe(false);
    expect(screen.getByRole("button", { name: "收起会话列表" })).toHaveProperty(
      "ariaExpanded",
      "true",
    );
    expect(window.localStorage.getItem("neo-webui.sidebar-width")).toBeNull();
  });

  it("keeps the top-left button as the drawer control on narrow screens", async () => {
    const user = userEvent.setup();
    vi.stubGlobal(
      "matchMedia",
      vi.fn(() => ({
        matches: true,
        media: "(max-width: 980px)",
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
      })),
    );
    await renderReady();
    const sidebar = screen.getByLabelText("会话列表");
    await user.click(screen.getByRole("button", { name: "打开会话列表" }));
    expect(sidebar.classList.contains("drawer-open")).toBe(true);
    expect(sidebar.classList.contains("sidebar-collapsed")).toBe(false);
    expect(screen.getByRole("button", { name: "关闭会话列表" })).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "关闭会话列表" }));
    expect(sidebar.classList.contains("drawer-close-immediate")).toBe(false);
    await user.click(screen.getByRole("button", { name: "打开会话列表" }));
    await user.click(within(sidebar).getByText("有界中继测试", { exact: true }));
    expect(sidebar.classList.contains("drawer-open")).toBe(false);
    expect(sidebar.classList.contains("drawer-close-immediate")).toBe(true);
  });

  it("updates the top-left control when the viewport enters drawer mode", async () => {
    const user = userEvent.setup();
    let onChange: (() => void) | null = null;
    const media = {
      matches: false,
      media: "(max-width: 980px)",
      addEventListener: (_type: string, listener: () => void) => {
        onChange = listener;
      },
      removeEventListener: vi.fn(),
    };
    vi.stubGlobal("matchMedia", vi.fn(() => media));

    await renderReady();
    expect(screen.getByRole("button", { name: "收起会话列表" }).getAttribute("aria-expanded"))
      .toBe("true");

    await act(async () => {
      media.matches = true;
      onChange?.();
    });

    const sidebar = screen.getByLabelText("会话列表");
    const toggle = screen.getByRole("button", { name: "打开会话列表" });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    await user.click(toggle);
    expect(sidebar.classList.contains("drawer-open")).toBe(true);
    expect(sidebar.classList.contains("sidebar-collapsed")).toBe(false);
  });

  it("throttles drag width writes through rAF and persists on release", async () => {
    await renderReady();
    const resizer = screen.getByRole("separator", { name: "调整会话列表宽度" });
    // Committed default mirrors into the CSS variable.
    await waitFor(() =>
      expect(document.documentElement.style.getPropertyValue("--sidebar-w")).toBe("264px"),
    );

    resizer.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, clientX: 100, button: 0 }));
    expect(document.documentElement.classList.contains("resizing")).toBe(true);
    document.dispatchEvent(new MouseEvent("pointermove", { clientX: 140 }));
    // The var write lands on the next animation frame, once per frame.
    await waitFor(() =>
      expect(document.documentElement.style.getPropertyValue("--sidebar-w")).toBe("304px"),
    );
    // Nothing is persisted mid-drag.
    expect(window.localStorage.getItem("neo-webui.sidebar-width")).toBeNull();

    document.dispatchEvent(new MouseEvent("pointerup", { clientX: 140 }));
    expect(document.documentElement.classList.contains("resizing")).toBe(false);
    await waitFor(() =>
      expect(window.localStorage.getItem("neo-webui.sidebar-width")).toBe("304"),
    );
  });

  it("adjusts the sidebar with ArrowLeft/ArrowRight, clamped and persisted", async () => {
    const user = userEvent.setup();
    await renderReady();
    const resizer = screen.getByRole("separator", { name: "调整会话列表宽度" });
    resizer.focus();
    await user.keyboard("{ArrowRight}");
    await waitFor(() =>
      expect(document.documentElement.style.getPropertyValue("--sidebar-w")).toBe("280px"),
    );
    expect(window.localStorage.getItem("neo-webui.sidebar-width")).toBe("280");
    await user.keyboard("{ArrowLeft}{ArrowLeft}");
    await waitFor(() =>
      expect(document.documentElement.style.getPropertyValue("--sidebar-w")).toBe("248px"),
    );
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

  it("returns focus to a right-clicked row when the menu closes", async () => {
    const user = userEvent.setup();
    const { socket } = await renderReady();
    socket.emit(asServerMessage(fixture.long_connection.workspace_snapshot));
    const title = await screen.findByText("并行格式化");
    // Right-click triggers the menu with the <li> row itself as trigger.
    const row = title.closest(".session-row") as HTMLElement;
    fireEvent.contextMenu(row);
    await screen.findByRole("menu", { name: "会话操作" });
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu", { name: "会话操作" })).toBeNull();
    // tabIndex=-1 makes the row programmatically focusable for mouse users.
    expect(document.activeElement).toBe(row);
    // Opening/closing the menu never switched the session.
    expect(FakeWebSocket.instances[0].watchSessionIds()).toEqual([]);
  });

  it("inserts a freshly created session into its workspace group in the sidebar", async () => {
    const user = userEvent.setup();
    // The create response uses a session id the workspace snapshot never
    // listed, so the sidebar depends on the summary insert path.
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
        const url = typeof input === "string" ? input : input.toString();
        if (url === "/api/sessions" && init?.method === "POST") {
          return Promise.resolve(
            new Response(
              JSON.stringify({
                session_id: "session_new",
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
        }
        return mockFetch(input, init);
      }),
    );
    const { socket } = await renderReady();
    socket.emit(asServerMessage(fixture.long_connection.workspace_snapshot));
    await screen.findByText("并行格式化");
    const input = screen.getByLabelText("输入消息");
    await user.type(input, "第一条消息{Enter}");
    await waitFor(() => expect(socket.watchSessionIds()).toEqual(["session_new"]));
    const neoGroup = screen.getByRole("group", { name: "neo" });
    expect(within(neoGroup).queryByText("刚建好的会话")).toBeNull();

    // The workspace layer reports the new session; it lands in the group
    // matching its recorded workspace label, newest first.
    socket.emit({
      type: "session_summary_changed",
      stream_id: fixture.stream_id,
      workspace_sequence: 1,
      event: {
        session_id: "session_new",
        title: "刚建好的会话",
        updated_at: "2026-08-09T11:00:00+00:00",
        pinned: false,
        archived: false,
        state: "running",
        workspace_label: "neo",
      },
    });
    expect(await within(neoGroup).findByText("刚建好的会话")).toBeTruthy();
    const rows = within(neoGroup)
      .getAllByRole("listitem")
      .map((item) => item.textContent ?? "");
    expect(rows[0]).toContain("刚建好的会话");
  });

  it("falls back to the current workspace group for an unknown summary label", async () => {
    const { socket } = await renderReady();
    socket.emit(asServerMessage(fixture.long_connection.workspace_snapshot));
    await screen.findByText("并行格式化");
    socket.emit({
      type: "session_summary_changed",
      stream_id: fixture.stream_id,
      workspace_sequence: 1,
      event: {
        session_id: "session_ghost",
        title: "无标签会话",
        updated_at: "2026-08-09T11:30:00+00:00",
        pinned: false,
        archived: false,
        state: "idle",
      },
    });
    const neoGroup = screen.getByRole("group", { name: "neo" });
    expect(await within(neoGroup).findByText("无标签会话")).toBeTruthy();
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

  it("resets search results to five rows when the keyword changes", async () => {
    const user = userEvent.setup();
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
        const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
        if (url.startsWith("/api/sessions?")) {
          const keyword = new URLSearchParams(url.split("?")[1] ?? "").get("query") ?? "";
          return Promise.resolve(
            new Response(
              JSON.stringify({
                items: Array.from({ length: 6 }, (_, index) => ({
                  session_id: `search_${keyword}_${index}`,
                  title: `${keyword}会话 ${index + 1}`,
                  pinned: false,
                  archived: false,
                  state: "idle",
                })),
              }),
              { status: 200, headers: { "content-type": "application/json" } },
            ),
          );
        }
        return mockFetch(input, init);
      }),
    );
    await renderReady();
    const search = screen.getByLabelText("搜索会话标题");

    await user.type(search, "甲");
    let results = await screen.findByRole("group", { name: "搜索结果" });
    expect(within(results).getAllByRole("listitem")).toHaveLength(5);
    await user.click(within(results).getByRole("button", { name: "展示更多" }));
    expect(within(results).getAllByRole("listitem")).toHaveLength(6);

    await user.clear(search);
    await user.type(search, "乙");
    results = await screen.findByRole("group", { name: "搜索结果" });
    await within(results).findByText("乙会话 1");
    expect(within(results).getAllByRole("listitem")).toHaveLength(5);
    expect(within(results).queryByText("乙会话 6")).toBeNull();
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

  it("creates a new session in the project selected above the composer", async () => {
    const user = userEvent.setup();
    const { socket } = await renderReady();
    socket.emit(asServerMessage(fixture.long_connection.workspace_snapshot));
    const project = await screen.findByLabelText("选择项目");
    await user.selectOptions(project, "workspace_playground");
    expect((project as HTMLSelectElement).value).toBe("workspace_playground");
    expect(screen.getByText("feature", { selector: ".workspace-branch" })).toBeTruthy();

    await user.type(screen.getByLabelText("输入消息"), "在另一个项目工作{Enter}");
    await waitFor(() =>
      expect(
        recordedRequests.some(
          (entry) =>
            entry.url === "/api/sessions" &&
            entry.method === "POST" &&
            (entry.body as { workspace_id?: string }).workspace_id === "workspace_playground",
        ),
      ).toBe(true),
    );
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
