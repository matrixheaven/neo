/**
 * Session view tests: transcript projection rendering, in-place approval /
 * question cards with submit-then-disable, floating task list, collapsible
 * bars with real states, unknown event records, 1013 reconnect and the
 * one-subscription guarantee.
 */

import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "../../src/app";
import { AppProvider } from "../../src/state/store";
import type { WebUiServerMessage } from "../../src/protocol";
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

describe("transcript redesign rows", () => {
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

  /** A complete second turn with an intermediate answer and two tool loops.
   * Sequences continue from the snapshot watermark. */
  function emitFinishedEditTurn(socket: FakeWebSocket) {
    let sequence = 9;
    const emit = (event: unknown) => {
      socket.emit({
        type: "session_event",
        stream_id: fixture.stream_id,
        session_id: "session_0001",
        sequence: sequence++,
        event: event as never,
      });
    };
    emit({ MessageAppended: { message: { User: { content: [{ Text: { text: "修改 app.ts" } }] } } } });
    emit({
      ToolExecutionStarted: {
        turn: 2,
        id: "tool_e1",
        name: "edit",
        // Real EditInput wire shape (neo-agent-core edit.rs): {path, old, new}.
        arguments: { path: "src/app.ts", old: "a\nb", new: "a\nb\nc" },
      },
    });
    emit({
      ToolExecutionFinished: {
        turn: 2,
        id: "tool_e1",
        name: "edit",
        result: {
          content: "ok",
          is_error: false,
          details: {
            changes: [
              {
                path: "src/app.ts",
                status: "committed",
                added: 1,
                removed: 0,
                diff: "--- a/src/app.ts\n+++ b/src/app.ts\n@@ -1,2 +1,3 @@\n a\n b\n+c\n",
              },
            ],
          },
        },
      },
    });
    emit({ MessageStarted: { turn: 2, id: "msg_2a" } });
    emit({ TextDelta: { turn: 2, text: "先完成 app.ts 的初步修改。" } });
    emit({ MessageFinished: { turn: 2, id: "msg_2a", stop_reason: "EndTurn" } });
    emit({
      ToolExecutionStarted: {
        turn: 2,
        id: "tool_g1",
        name: "grep",
        arguments: { pattern: "app" },
      },
    });
    emit({
      ToolExecutionFinished: {
        turn: 2,
        id: "tool_g1",
        name: "grep",
        result: { content: "src/app.ts", is_error: false },
      },
    });
    emit({
      ToolExecutionStarted: {
        turn: 2,
        id: "tool_r1",
        name: "read",
        arguments: { path: "src/app.ts" },
      },
    });
    emit({
      ToolExecutionFinished: {
        turn: 2,
        id: "tool_r1",
        name: "read",
        result: { content: "a\nb\nc", is_error: false },
      },
    });
    emit({
      ToolExecutionStarted: {
        turn: 2,
        id: "cmd_2",
        name: "bash",
        arguments: { command: "cargo test" },
      },
    });
    emit({ ShellCommandStarted: { turn: 2, id: "cmd_2", command: "cargo test", cwd: "." } });
    emit({
      ShellCommandFinished: {
        turn: 2,
        id: "cmd_2",
        exit_code: 1,
        stdout: "",
        stderr: "failed",
        truncated: false,
      },
    });
    emit({
      ToolExecutionFinished: {
        turn: 2,
        id: "cmd_2",
        name: "bash",
        result: { content: "failed", is_error: true },
      },
    });
    emit({ MessageStarted: { turn: 2, id: "msg_2b" } });
    emit({ TextDelta: { turn: 2, text: "已修改 app.ts。" } });
    emit({ MessageFinished: { turn: 2, id: "msg_2b", stop_reason: "EndTurn" } });
  }

  it("clamps long user messages behind a gradient with an expand toggle", async () => {
    const user = userEvent.setup();
    const { socket } = await openSession1();
    const longText = Array.from({ length: 12 }, (_, index) => `第 ${index + 1} 行`).join("\n");
    socket.emit({
      type: "session_event",
      stream_id: fixture.stream_id,
      session_id: "session_0001",
      sequence: 9,
      event: { MessageAppended: { message: { User: { content: [{ Text: { text: longText } }] } } } },
    });
    const toggle = await screen.findByRole("button", { name: "展开" });
    const wrap = toggle.closest(".u-text-wrap") as HTMLElement;
    expect(wrap.className).toContain("is-clamped");
    await user.click(toggle);
    const collapse = screen.getByRole("button", { name: "收起" });
    expect((collapse.closest(".u-text-wrap") as HTMLElement).className).not.toContain(
      "is-clamped",
    );
  });

  it("breathes while thinking streams and auto-collapses on finish", async () => {
    const { socket, container } = await openSession1();
    let sequence = 9;
    const emit = (event: unknown) => {
      socket.emit({
        type: "session_event",
        stream_id: fixture.stream_id,
        session_id: "session_0001",
        sequence: sequence++,
        event: event as never,
      });
    };
    emit({ ThinkingStarted: { turn: 2, id: "th_live" } });
    emit({ ThinkingDelta: { turn: 2, text: "实时思考内容" } });
    const bar = await screen.findByRole("button", { name: /思考，状态：思考中/ });
    expect(bar.getAttribute("aria-expanded")).toBe("true");
    expect(container.querySelector(".think.live .think-title")).not.toBeNull();
    await screen.findByText("实时思考内容");

    emit({ ThinkingFinished: { turn: 2, redacted: false } });
    await waitFor(() => expect(container.querySelector(".think.live")).toBeNull());
    const text = await screen.findByText("实时思考内容");
    const line = text.closest(".think") as HTMLElement;
    expect(line.className).not.toContain("open");
    expect(
      within(line).getByRole("button", { name: /思考，状态：已完成/ }).getAttribute("aria-expanded"),
    ).toBe("false");
  });

  it("respects a manual collapse choice across the stream finish", async () => {
    const user = userEvent.setup();
    const { socket } = await openSession1();
    let sequence = 9;
    const emit = (event: unknown) => {
      socket.emit({
        type: "session_event",
        stream_id: fixture.stream_id,
        session_id: "session_0001",
        sequence: sequence++,
        event: event as never,
      });
    };
    emit({ ThinkingStarted: { turn: 2, id: "th_live" } });
    emit({ ThinkingDelta: { turn: 2, text: "实时思考内容" } });
    // Streaming default is open; the user collapses it mid-stream.
    const bar = await screen.findByRole("button", { name: /思考，状态：思考中/ });
    await user.click(bar);
    expect(
      screen.getByRole("button", { name: /思考，状态：思考中/ }).getAttribute("aria-expanded"),
    ).toBe("false");
    // The finish must not re-expand it.
    emit({ ThinkingFinished: { turn: 2, redacted: false } });
    const text = await screen.findByText("实时思考内容");
    const line = text.closest(".think") as HTMLElement;
    expect(line.className).not.toContain("open");
    expect(
      within(line).getByRole("button", { name: /思考，状态：已完成/ }).getAttribute("aria-expanded"),
    ).toBe("false");
  });

  it("shows the command echo and status metadata in the expanded tool line", async () => {
    const user = userEvent.setup();
    const { socket } = await openSession1();
    for (const envelope of session1.after_snapshot.slice(0, 6)) {
      socket.emit(asServerMessage(envelope)); // through ToolExecutionFinished (seq 14)
    }
    const toolBar = await screen.findByRole("button", { name: /运行 cargo test -p neo-webui/ });
    await user.click(toolBar);
    const line = toolBar.closest(".tool-line") as HTMLElement;
    // Command echo in mono, then the status metadata line.
    expect(within(line).getByText("$ cargo test -p neo-webui")).toBeTruthy();
    expect(within(line).getByText(/状态：已完成/)).toBeTruthy();
  });

  it("folds one user prompt's full activity behind its final answer", async () => {
    const user = userEvent.setup();
    const { socket } = await openSession1();
    emitFinishedEditTurn(socket);
    const fold = await screen.findByRole("button", {
      name: /展开工作过程（搜索 1 · 读取 1 · 编辑 1 · 命令 1 · 失败 1 · 5 个步骤）/,
    });
    expect(fold.getAttribute("aria-expanded")).toBe("false");
    await user.click(fold);
    const openFold = screen.getByRole("button", {
      name: /收起工作过程（搜索 1 · 读取 1 · 编辑 1 · 命令 1 · 失败 1 · 5 个步骤）/,
    });
    expect(openFold.getAttribute("aria-expanded")).toBe("true");
    const foldRoot = openFold.closest(".turn-fold") as HTMLElement;
    expect(foldRoot.className).toContain("open");
    expect(within(foldRoot).getByRole("button", { name: /编辑 src\/app.ts/ })).toBeTruthy();
    expect(within(foldRoot).getByRole("button", { name: /搜索 app/ })).toBeTruthy();
    const readBar = within(foldRoot).getByRole("button", { name: /读取 app.ts/ });
    const readLine = readBar.closest(".tool-line") as HTMLElement;
    expect(readLine.querySelector('[data-tool-icon="file"]')).not.toBeNull();
    expect(readLine.querySelector(".tl-subtle")?.textContent).toBe("src/");
    expect(readLine.querySelector(".tl-subtle")?.nextElementSibling?.className).toContain("line-caret");
    const failedCommand = within(foldRoot).getAllByRole("button", {
      name: /运行 cargo test，状态：失败/,
    });
    expect(failedCommand).toHaveLength(1);
    const failedLine = failedCommand[0].closest(".tool-line") as HTMLElement;
    expect(failedLine.querySelector('[data-status-icon="failed"]')).not.toBeNull();
    expect(failedLine.querySelector('[data-tool-icon="terminal"]')).not.toBeNull();
    expect(failedLine.querySelector(".line-tail")).toBeNull();
    await user.click(failedCommand[0]);
    expect(within(failedLine).getByText(/退出码 1/)).toBeTruthy();
    expect(within(foldRoot).getByText("先完成 app.ts 的初步修改。")).toBeTruthy();

    expect(screen.getAllByRole("button", { name: "复制回答" })).toHaveLength(1);
    const finalAnswer = screen.getByText("已修改 app.ts。");
    const finalGroup = finalAnswer.closest(".a-msg") as HTMLElement;
    const footer = finalGroup.querySelector(".answer-ft") as HTMLElement;
    expect(within(footer).getByText("src/app.ts")).toBeTruthy();
    expect(within(footer).getByText("已编辑 1 个文件")).toBeTruthy();
    expect(footer.querySelector(".ft-summary .ft-add")?.textContent).toBe("+1");
  });

  it("keeps in-progress process rows visible in an open fold", async () => {
    const { container } = await openSession1();
    // The active turn is grouped with its answer, but remains open and cannot
    // be collapsed while it is still receiving activity.
    const fold = container.querySelector(".turn-fold") as HTMLElement;
    expect(fold).not.toBeNull();
    expect(fold.className).toContain("open");
    expect(within(fold).getByRole("button", { name: /工作中/ })).toHaveProperty("disabled", true);
    expect(screen.getByRole("button", { name: /运行 cargo test -p neo-webui/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: /展开思考/ })).toBeTruthy();
  });

  it("derives the answer footer file list from edit tools and copies the answer", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn<(text: string) => Promise<void>>().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    const { socket } = await openSession1();
    emitFinishedEditTurn(socket);
    await screen.findByText("已修改 app.ts。");
    const footer = document.querySelector(".answer-ft") as HTMLElement;
    expect(footer).not.toBeNull();
    expect(within(footer).getByText("src/app.ts")).toBeTruthy();
    expect(within(footer).getByText("已编辑 1 个文件")).toBeTruthy();
    expect(footer.querySelector(".ft-summary .ft-add")?.textContent).toBe("+1");
    await user.click(within(footer).getByRole("button", { name: "展开 src/app.ts 的局部差异" }));
    expect(within(footer).getByText("+c")).toBeTruthy();
    await user.click(within(footer).getByRole("button", { name: "复制回答" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("已修改 app.ts。"));
    await screen.findByRole("button", { name: "已复制" });
  });

  it("shows committed files in batches and expands local diffs", async () => {
    const user = userEvent.setup();
    const { socket } = await openSession1();
    let sequence = 9;
    const emit = (event: unknown) => {
      socket.emit({
        type: "session_event",
        stream_id: fixture.stream_id,
        session_id: "session_0001",
        sequence: sequence++,
        event: event as never,
      });
    };

    emit({ MessageAppended: { message: { User: { content: [{ Text: { text: "整理文件变更" } }] } } } });
    emit({
      ToolExecutionFinished: {
        turn: 2,
        id: "write_many",
        name: "write",
        result: {
          content: "wrote files",
          is_error: false,
          details: {
            changes: [
              {
                path: "src/one.ts",
                status: "committed",
                added: 1,
                removed: 1,
                diff: "@@ -1 +1 @@\n-before\n+after",
              },
              {
                path: "src/two.ts",
                status: "committed_unsynced",
                added: 2,
                removed: 0,
                diff: "@@ -1 +1,2 @@\n+two\n+more",
              },
              {
                path: "src/three.ts",
                status: "committed",
                added: 0,
                removed: 1,
                diff: "@@ -1 +0,0 @@\n-three",
              },
              {
                path: "src/new-file.ts",
                status: "committed",
                added: 4,
                removed: 0,
              },
              {
                path: "src/skipped.ts",
                status: "not_attempted",
                added: 99,
                removed: 99,
              },
            ],
          },
        },
      },
    });
    emit({ MessageStarted: { turn: 2, id: "files_result" } });
    emit({ TextDelta: { turn: 2, text: "文件变更已整理。" } });
    emit({ MessageFinished: { turn: 2, id: "files_result", stop_reason: "EndTurn" } });

    const answer = await screen.findByText("文件变更已整理。");
    const footer = answer.closest(".a-msg")?.querySelector(".answer-ft") as HTMLElement;
    expect(within(footer).getByText("已编辑 4 个文件")).toBeTruthy();
    expect(within(footer).getAllByRole("listitem")).toHaveLength(3);
    expect(within(footer).queryByText("src/new-file.ts")).toBeNull();
    expect(within(footer).queryByText("src/skipped.ts")).toBeNull();

    await user.click(within(footer).getByRole("button", { name: "显示其余 1 个文件" }));
    expect(within(footer).getAllByRole("listitem")).toHaveLength(4);
    expect(within(footer).getByText("src/new-file.ts")).toBeTruthy();
    expect(within(footer).queryByText("src/skipped.ts")).toBeNull();

    await user.click(within(footer).getByRole("button", { name: "展开 src/one.ts 的局部差异" }));
    expect(within(footer).getByText("-before")).toBeTruthy();
    expect(within(footer).getByText("+after")).toBeTruthy();

    await user.click(within(footer).getByRole("button", { name: "收起其余文件" }));
    expect(within(footer).getAllByRole("listitem")).toHaveLength(3);
  });

  it("renders approval rows as pending and submitted, then hides resolved history", async () => {
    const user = userEvent.setup();
    const { socket } = await openSession1();
    const row = screen.getByRole("group", { name: /审批请求/ });
    expect(row.textContent).toContain("等待确认");
    expect((row as HTMLElement).className).toContain("approval-row");

    await user.click(within(row).getByRole("button", { name: "允许一次" }));
    const submittedRow = screen.getByRole("group", { name: /审批请求/ });
    expect(submittedRow.textContent).toContain("已提交，等待确认");
    expect(within(submittedRow).getByRole("button", { name: "允许一次" })).toHaveProperty(
      "disabled",
      true,
    );

    // Resolved history stays out of the transcript once it is no longer pending.
    const snapshot = structuredClone(session1.snapshot);
    delete snapshot.pending_approval;
    socket.emit({ type: "session_snapshot", snapshot });
    await waitFor(() => expect(screen.queryByRole("group", { name: /审批请求/ })).toBeNull());
  });

  it("merges delegate progress into the agent line instead of an unknown record", async () => {
    const { socket } = await openSession1();
    socket.emit({
      type: "session_event",
      stream_id: fixture.stream_id,
      session_id: "session_0001",
      sequence: 9,
      event: {
        DelegateStarted: {
          turn: 1,
          agent: { id: "agent_x", display_name: "explorer", state: "running", task_title: "巡检" },
        },
      },
    });
    socket.emit({
      type: "session_event",
      stream_id: fixture.stream_id,
      session_id: "session_0001",
      sequence: 10,
      event: {
        DelegateProgressUpdated: {
          turn: 1,
          progress: { agent_id: "agent_x", state: "running", latest_text: "扫描 src/ 中" },
        },
      },
    });
    const line = await screen.findByRole("button", { name: /查看子代理详情：巡检/ });
    expect(line.textContent).toContain("运行中");
    expect(line.textContent).toContain("扫描 src/ 中");
    expect(screen.queryByRole("button", { name: /未识别事件/ })).toBeNull();
  });
});

describe("session view", () => {
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

  it("renders user bubble, streamed assistant text, thinking and queued tool", async () => {
    await openSession1();
    await screen.findByText("检查有界中继的行为测试并修复慢连接。");
    await screen.findByText("我来检查一下有界中继的行为测试。");

    const thinkingBar = screen.getByRole("button", { name: /展开思考/ });
    expect(thinkingBar.getAttribute("aria-expanded")).toBe("false");

    const toolBar = screen.getByRole("button", { name: /运行 cargo test -p neo-webui/ });
    expect(toolBar.textContent).toContain("排队等待");

    const approval = screen.getByRole("group", { name: /审批请求/ });
    expect(approval.textContent).toContain("等待确认");
  });

  it("submits an approval once and disables the buttons until confirmation", async () => {
    const user = userEvent.setup();
    const { socket } = await openSession1();
    const approvalButton = await screen.findByRole("button", { name: "允许一次" });
    await user.click(approvalButton);
    await waitFor(() =>
      expect(
        recordedRequests.some(
          (entry) =>
            entry.url === "/api/sessions/session_0001/approval" &&
            (entry.body as { request_id?: string }).request_id === "approval_01" &&
            (entry.body as { turn_id?: string }).turn_id === "turn_01",
        ),
      ).toBe(true),
    );
    expect(screen.getByRole("button", { name: "允许一次" })).toHaveProperty("disabled", true);
    expect(screen.getByRole("group", { name: /审批请求/ }).textContent).toContain(
      "已提交，等待确认",
    );
    // Server confirmation removes the resolved card from the transcript (sequences 9..12).
    for (const envelope of session1.after_snapshot.slice(0, 4)) {
      socket.emit(asServerMessage(envelope));
    }
    await waitFor(() => expect(screen.queryByRole("group", { name: /审批请求/ })).toBeNull());
  });

  it("projects TodoUpdated into the floating task list above the composer", async () => {
    const user = userEvent.setup();
    const { socket } = await openSession1();
    expect(screen.queryByText(/任务 \d+\/\d+/)).toBeNull();
    for (const envelope of session1.after_snapshot.slice(0, 3)) {
      socket.emit(asServerMessage(envelope));
    }
    const summary = await screen.findByText("任务 1/2");
    expect(summary.textContent).toContain("任务 1/2");
    const taskSummary = screen.getByRole("button", { name: "展开任务清单" });
    await user.click(taskSummary);
    await screen.findByText("建立 neo-webui 包");
    const titles = await screen.findAllByText("编写行为测试");
    expect(titles.length).toBeGreaterThanOrEqual(2); // collapsed current + expanded list
  });

  it("expands thinking and tool bars to show real content", async () => {
    const user = userEvent.setup();
    const { socket } = await openSession1();
    for (const envelope of session1.after_snapshot.slice(0, 6)) {
      socket.emit(asServerMessage(envelope)); // through ToolExecutionFinished (seq 14)
    }

    await user.click(await screen.findByRole("button", { name: /展开思考/ }));
    await screen.findByText("先检查有界中继的边界条件。");

    const toolBar = await screen.findByRole("button", { name: /运行 cargo test -p neo-webui/ });
    const toolLine = toolBar.closest(".tool-line") as HTMLElement;
    expect(toolLine.querySelector('[data-status-icon="finished"]')).not.toBeNull();
    expect(toolLine.querySelector(".line-tail")).toBeNull();
    await user.click(toolBar);
    await screen.findByText(/42 passed/);
    // Full output loads through the opaque reference, verbatim.
    const readFull = await screen.findByRole("button", { name: "读取完整输出" });
    await user.click(readFull);
    await screen.findByText(/服务端返回的完整输出内容/);
    // The opaque output id is passed back verbatim, never encoded or decoded.
    await waitFor(() =>
      expect(
        recordedRequests.some((entry) =>
          entry.url.startsWith(
            "/api/sessions/session_0001/tool-output/eyJhZ2VudF9pZCI6Im1haW4iLCJ0YXNrX2lkIjoidGFza18wMSJ9?",
          ),
        ),
      ).toBe(true),
    );
  });

  it("keeps unknown event tags as collapsible records and continues", async () => {
    const user = userEvent.setup();
    const { socket } = await openSession1();
    const unknown: WebUiServerMessage = {
      type: "session_event",
      stream_id: fixture.stream_id,
      session_id: "session_0001",
      sequence: 9,
      event: { FutureThing: { note: "保留原始内容" } } as never,
    };
    socket.emit(unknown);
    const bar = await screen.findByRole("button", { name: /未识别事件 FutureThing/ });
    await user.click(bar);
    await screen.findByText(/"FutureThing"/);

    socket.emit({
      type: "session_event",
      stream_id: fixture.stream_id,
      session_id: "session_0001",
      sequence: 10,
      event: { TextDelta: { turn: 1, text: "后续文本" } },
    });
    await screen.findByText(/后续文本/);
  });

  it("reconnects after a 1013 close with fresh subscriptions, exactly one session watch", async () => {
    const { socket } = await openSession1();
    socket.closeWith(1013);
    await screen.findByText("连接已断开，正在重连…");
    await waitFor(
      () => expect(FakeWebSocket.instances.length).toBe(2),
      { timeout: 3000 },
    );
    const next = FakeWebSocket.instances[1];
    expect(socket.closed).toBe(false); // server closed it; client just observes
    await waitFor(() => expect(next.watchSessionIds()).toEqual(["session_0001"]));
    // One workspace + one session watch on the new socket: no duplicate
    // current-session subscription.
    const types = next.sent.map((data) => (JSON.parse(data) as { type: string }).type);
    expect(types.filter((type) => type === "watch_workspace")).toHaveLength(1);
    expect(types.filter((type) => type === "watch_session")).toHaveLength(1);
  });

  it("keeps the initial connection status while the first handshake retries", async () => {
    FakeWebSocket.autoOpen = false;
    renderApp();
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));

    FakeWebSocket.instances[0].closeWith(1013);

    expect(screen.queryByText("连接已断开，正在重连…")).toBeNull();
    expect(screen.getByText("正在连接…")).toBeTruthy();
  });

  it("shows initial connection status when opening an unseen historical session during reconnect", async () => {
    renderApp();
    await screen.findByLabelText("会话列表");
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    const socket = FakeWebSocket.instances[0];
    socket.emit(asServerMessage(fixture.long_connection.workspace_snapshot));
    await screen.findByText("并行格式化");

    FakeWebSocket.autoOpen = false;
    socket.closeWith(1013);
    await screen.findByText("连接已断开，正在重连…");

    (await screen.findByText("并行格式化")).click();
    await screen.findByText("正在连接…");
    expect(screen.queryByText("连接已断开，正在重连…")).toBeNull();
  });

  it("switching sessions keeps the background summary updating and drops the old transcript", async () => {
    const { socket } = await openSession1();
    await screen.findByText("检查有界中继的行为测试并修复慢连接。");
    // Switch away: transcript of session_0001 is dropped from memory.
    (await screen.findByText("并行格式化")).click();
    await waitFor(() => expect(socket.watchSessionIds()).toEqual(["session_0001", "session_0002"]));
    expect(screen.queryByText("检查有界中继的行为测试并修复慢连接。")).toBeNull();
    // A summary update for session_0001 still updates the sidebar.
    socket.emit({
      type: "session_summary_changed",
      stream_id: fixture.stream_id,
      workspace_sequence: 2,
      event: {
        session_id: "session_0001",
        title: "有界中继测试",
        updated_at: "2026-08-09T10:05:00+00:00",
        pinned: true,
        archived: false,
        state: "idle",
      },
    });
    const row = (await screen.findByText("有界中继测试")).closest(".session-row") as HTMLElement;
    await waitFor(() => expect((row.querySelector(".session-main") as HTMLButtonElement).title).toContain("状态：空闲"));
  });

  it("shows queued shell positions and truncation markers from explicit fields", async () => {
    const { socket } = await openSession1();
    socket.emit({
      type: "session_event",
      stream_id: fixture.stream_id,
      session_id: "session_0001",
      sequence: 9,
      event: {
        ShellCommandQueued: { turn: 1, id: "sh_01", command: "cargo test", cwd: ".", origin: "auto" },
      },
    });
    socket.emit({
      type: "session_event",
      stream_id: fixture.stream_id,
      session_id: "session_0001",
      sequence: 10,
      event: { ShellCommandQueueUpdated: { turn: 1, id: "sh_01", position: 2, waiting_ms: 150 } },
    });
    const bar = await screen.findByRole("button", {
      name: /展开运行 cargo test，状态：排队等待 · 位置 2 · 已等待 150ms/,
    });
    expect(bar.textContent).toContain("排队等待");
    expect(bar.textContent).toContain("位置 2");
    expect(bar.textContent).not.toContain("已完成");
    const line = bar.closest(".tool-line") as HTMLElement;
    expect(line.querySelector('[data-status-icon="queued"]')).not.toBeNull();
    expect(line.querySelector('[data-status-icon="finished"]')).toBeNull();
    expect(line.querySelector(".line-tail")?.textContent).toContain("排队等待");
  });

  it("escalates reconnect delays while the service stays unreachable", async () => {
    const { socket } = await openSession1();
    // From now on, new connections never complete the handshake: the backoff
    // counter must not reset, so delays escalate (500ms → 1000ms → …).
    FakeWebSocket.autoOpen = false;
    socket.closeWith(1013);
    await waitFor(() => expect(FakeWebSocket.instances.length).toBe(2), { timeout: 1500 });
    const second = FakeWebSocket.instances[1];
    expect(second.readyState).toBe(FakeWebSocket.CONNECTING);
    second.closeWith(1013);
    await new Promise((resolve) => setTimeout(resolve, 700));
    // Still waiting: the second delay is ~1000ms, not reset to 500ms.
    expect(FakeWebSocket.instances.length).toBe(2);
    await waitFor(() => expect(FakeWebSocket.instances.length).toBe(3), { timeout: 2000 });
  });

  it("re-subscribes once per resync generation, not per arriving event", async () => {
    const { socket } = await openSession1();
    const gapEvent = (sequence: number) => ({
      type: "session_event" as const,
      stream_id: fixture.stream_id,
      session_id: "session_0001",
      sequence,
      event: { TextDelta: { turn: 1, text: "缺口" } },
    });
    // Cursor is 8: sequence 10 is a gap → one resync re-subscription.
    socket.emit(gapEvent(10));
    await waitFor(() => expect(socket.watchSessionIds()).toEqual(["session_0001", "session_0001"]));
    // Further events while the cursor cannot resume must not re-send.
    socket.emit(gapEvent(11));
    socket.emit(gapEvent(12));
    await new Promise((resolve) => setTimeout(resolve, 150));
    expect(socket.watchSessionIds()).toHaveLength(2);
    // Fresh snapshot resolves; a later new gap is a new generation.
    socket.emit({ type: "session_snapshot", snapshot: session1.snapshot });
    await screen.findByText("检查有界中继的行为测试并修复慢连接。");
    socket.emit(gapEvent(10));
    await waitFor(() => expect(socket.watchSessionIds()).toHaveLength(3));
  });
});
