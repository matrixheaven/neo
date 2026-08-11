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

  it("shows a spinner while thinking streams and auto-collapses on finish", async () => {
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
    const runningLine = bar.closest(".think") as HTMLElement;
    expect(runningLine.querySelector("[data-status-icon]")).toBeNull();
    expect(runningLine.querySelector("[data-thinking-icon]")).not.toBeNull();
    expect(runningLine.querySelector("[data-thinking-spinner]")).not.toBeNull();
    expect(runningLine.querySelector("[data-thinking-completed-icon]")).toBeNull();
    expect(runningLine.querySelector(".line-tail")).toBeNull();
    await screen.findByText("实时思考内容");

    emit({ ThinkingFinished: { turn: 2, redacted: false } });
    await waitFor(() => expect(container.querySelector(".think.live")).toBeNull());
    const text = await screen.findByText("实时思考内容");
    const line = text.closest(".think") as HTMLElement;
    expect(line.className).not.toContain("open");
    expect(
      within(line).getByRole("button", { name: /思考，状态：已完成/ }).getAttribute("aria-expanded"),
    ).toBe("false");
    expect(line.querySelector("[data-status-icon]")).toBeNull();
    expect(line.querySelector("[data-thinking-icon]")).not.toBeNull();
    expect(line.querySelector("[data-thinking-spinner]")).toBeNull();
    expect(line.querySelector("[data-thinking-completed-icon]")).not.toBeNull();
    expect(line.querySelector(".line-tail")).toBeNull();
  });

  it("shows workflow state with a left icon and no trailing capsule", async () => {
    const { socket } = await openSession1();
    const emit = (event: unknown, sequence: number) => {
      socket.emit({
        type: "session_event",
        stream_id: fixture.stream_id,
        session_id: "session_0001",
        sequence,
        event: event as never,
      });
    };

    emit(
      {
        WorkflowUpdated: {
          turn: 2,
          workflow: {
            id: "wf_session_01",
            title: "运行测试",
            state: "running",
            current_phase: "executing",
            started_at_ms: 1723000002000,
            updated_at_ms: 1723000002500,
            latest_log_summary: "执行行为测试",
          },
        },
      },
      9,
    );
    const running = await screen.findByRole("button", {
      name: /工作流 运行测试，状态：运行中 · executing/,
    });
    const runningLine = running.closest(".kind-workflow") as HTMLElement;
    expect(runningLine.querySelector('[data-status-icon="running"]')).not.toBeNull();
    expect(runningLine.querySelector(".line-tail")).toBeNull();
    expect(runningLine.querySelector(".tl-status")).toBeNull();
    const runningHead = runningLine.querySelector(".line-head") as HTMLElement;
    const runningChildren = [...runningHead.children];
    expect(runningChildren.indexOf(runningHead.querySelector(".line-caret") as Element)).toBeGreaterThan(
      runningChildren.indexOf(runningHead.querySelector(".tl-mono") as Element),
    );

    emit(
      {
        WorkflowFinished: {
          turn: 2,
          workflow: {
            id: "wf_session_01",
            title: "运行测试",
            state: "completed",
            started_at_ms: 1723000002000,
            updated_at_ms: 1723000004000,
            latest_log_summary: "执行行为测试",
            terminal_reason: "completed",
          },
        },
      },
      10,
    );
    const finished = await screen.findByRole("button", {
      name: /工作流 运行测试，状态：已完成（completed）/,
    });
    const finishedLine = finished.closest(".kind-workflow") as HTMLElement;
    expect(finishedLine.querySelector('[data-status-icon="finished"]')).not.toBeNull();
    expect(finishedLine.querySelector('[data-status-icon="running"]')).toBeNull();
    expect(finishedLine.querySelector(".line-tail")).toBeNull();
    expect(finishedLine.querySelector(".tl-status")).toBeNull();

    for (const [offset, state] of ["failed", "cancelled", "resource_limited"].entries()) {
      const title = `终止工作流 ${state}`;
      emit(
        {
          WorkflowFinished: {
            turn: 2,
            workflow: {
              id: `wf_session_${state}`,
              title,
              state,
              started_at_ms: 1723000002000,
              updated_at_ms: 1723000004000,
              terminal_reason: state,
            },
          },
        },
        11 + offset,
      );
      const failed = await screen.findByRole("button", {
        name: new RegExp(`工作流 ${title}，状态：失败（${state}）`),
      });
      const failedLine = failed.closest(".kind-workflow") as HTMLElement;
      expect(failedLine.querySelector('[data-status-icon="failed"]')).not.toBeNull();
      expect(failedLine.querySelector('[data-status-icon="finished"]')).toBeNull();
      expect(failed.textContent).not.toContain("已完成");
    }
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

  it("keeps completed same-turn thinking blocks independently expandable", async () => {
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

    emit({ MessageAppended: { message: { User: { content: [{ Text: { text: "说明过程" } }] } } } });
    emit({ ThinkingStarted: { turn: 2, id: "reasoning" } });
    emit({ ThinkingDelta: { turn: 2, text: "第一段独立思考" } });
    emit({ ThinkingFinished: { turn: 2, redacted: false } });
    emit({ ThinkingStarted: { turn: 2, id: "reasoning" } });
    emit({ ThinkingDelta: { turn: 2, text: "第二段独立思考" } });
    emit({ ThinkingFinished: { turn: 2, redacted: false } });
    emit({ MessageStarted: { turn: 2, id: "thought_answer" } });
    emit({ TextDelta: { turn: 2, text: "思考后的回答" } });
    emit({ MessageFinished: { turn: 2, id: "thought_answer", stop_reason: "EndTurn" } });

    const answer = await screen.findByText("思考后的回答");
    const group = answer.closest(".a-msg") as HTMLElement;
    const fold = group.querySelector(".turn-fold") as HTMLElement;
    expect(fold).not.toBeNull();
    expect(fold.className).not.toContain("open");
    await user.click(within(fold).getByRole("button", { name: /展开工作过程/ }));
    expect(fold.className).toContain("open");

    const first = (await within(fold).findByText("第一段独立思考")).closest(".think") as HTMLElement;
    const second = (await within(fold).findByText("第二段独立思考")).closest(".think") as HTMLElement;
    expect(first).not.toBe(second);
    expect(within(first).getByRole("button", { name: /展开思考，状态：已完成/ })).toHaveProperty(
      "ariaExpanded",
      "false",
    );
    const secondButton = within(second).getByRole("button", { name: /展开思考，状态：已完成/ });
    await user.click(secondButton);
    expect(first.className).not.toContain("open");
    expect(second.className).toContain("open");
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
    expect(line.querySelector(".tl-status")).toBeNull();
    expect(line.querySelector('[data-status-icon="finished"]')).not.toBeNull();
  });

  it("uses semantic details for Skill and AskUserQuestion instead of parameter JSON", async () => {
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

    emit({
      ToolExecutionStarted: {
        turn: 2,
        id: "tool_skill_semantic",
        name: "Skill",
        arguments: { skill: "release", arguments: { secret: "不要显示" } },
      },
    });
    emit({
      ToolExecutionFinished: {
        turn: 2,
        id: "tool_skill_semantic",
        name: "Skill",
        result: { content: "技能已启用", is_error: false },
      },
    });
    emit({
      ToolExecutionStarted: {
        turn: 2,
        id: "tool_question_semantic",
        name: "AskUserQuestion",
        arguments: { questions: [{ question: "不要显示的问题" }] },
      },
    });
    emit({
      ToolExecutionFinished: {
        turn: 2,
        id: "tool_question_semantic",
        name: "AskUserQuestion",
        result: { content: "提问失败", is_error: true },
      },
    });

    const skillButton = await screen.findByRole("button", { name: /使用技能 release/ });
    await user.click(skillButton);
    const skillLine = skillButton.closest(".tool-line") as HTMLElement;
    expect(within(skillLine).getByText("已调用技能：release")).toBeTruthy();
    expect(within(skillLine).queryByText("技能已启用")).toBeNull();
    expect(within(skillLine).queryByText("不要显示")).toBeNull();

    const questionButton = await screen.findByRole("button", { name: /询问用户/ });
    await user.click(questionButton);
    const questionLine = questionButton.closest(".tool-line") as HTMLElement;
    expect(within(questionLine).getByText("提问失败")).toBeTruthy();
    expect(within(questionLine).queryByText("不要显示的问题")).toBeNull();
    expect(questionLine.querySelector(".tl-status")).toBeNull();
    expect(questionLine.querySelector(".line-tail")).toBeNull();
    expect(questionLine.querySelector('[data-status-icon="failed"]')).not.toBeNull();
  });

  it("renders Edit, Write and TodoList from structured tool fields", async () => {
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

    emit({ MessageAppended: { message: { User: { content: [{ Text: { text: "展示专用工具" } }] } } } });
    emit({
      ToolExecutionStarted: {
        turn: 2,
        id: "tool_edit_special",
        name: "Edit",
        arguments: { path: "src/app.ts", old: "before", new: "after" },
      },
    });
    emit({
      ToolExecutionFinished: {
        turn: 2,
        id: "tool_edit_special",
        name: "Edit",
        result: {
          content: "edited",
          is_error: false,
          details: {
            changes: [{
              path: "src/app.ts",
              status: "committed",
              replacements: 1,
              added: 1,
              removed: 1,
            }],
          },
        },
      },
    });
    emit({
      ToolExecutionStarted: {
        turn: 2,
        id: "tool_edit_diff",
        name: "Edit",
        arguments: { path: "src/diff.ts", old: "old", new: "new" },
      },
    });
    emit({
      ToolExecutionFinished: {
        turn: 2,
        id: "tool_edit_diff",
        name: "Edit",
        result: {
          content: "edited",
          is_error: false,
          details: {
            changes: [{
              path: "src/diff.ts",
              status: "committed",
              added: 1,
              removed: 1,
              diff: "--- src/diff.ts\n+++ src/diff.ts\n@@ -1 +1 @@\n-old\n+new\n---literal old\n+++literal new",
            }],
          },
        },
      },
    });
    emit({
      ToolExecutionStarted: {
        turn: 2,
        id: "tool_write_special",
        name: "Write",
        arguments: { path: "src/new.ts", content: "export const value = 1;" },
      },
    });
    emit({
      ToolExecutionFinished: {
        turn: 2,
        id: "tool_write_special",
        name: "Write",
        result: {
          content: "created",
          is_error: false,
          details: {
            changes: [{
              path: "src/new.ts",
              operation: "created",
              status: "committed",
              added: 1,
              removed: 0,
              content: "结果正文不应覆盖参数正文",
            }],
          },
        },
      },
    });
    emit({
      ToolExecutionStarted: {
        turn: 2,
        id: "tool_todo_special",
        name: "TodoList",
        arguments: {
          todos: [
            { title: "检查展示", status: "in_progress" },
            { title: "补回归", status: "pending" },
          ],
        },
      },
    });
    emit({
      ToolExecutionFinished: {
        turn: 2,
        id: "tool_todo_special",
        name: "TodoList",
        result: {
          content: "updated",
          is_error: false,
          details: {
            todos: [
              { title: "检查展示", status: "in_progress" },
              { title: "补回归", status: "pending" },
            ],
          },
        },
      },
    });
    emit({
      ToolExecutionStarted: {
        turn: 2,
        id: "tool_set_todo_special",
        name: "SetTodoList",
        arguments: {
          todos: [{ title: "发布回归", status: "done" }],
        },
      },
    });
    emit({
      ToolExecutionFinished: {
        turn: 2,
        id: "tool_set_todo_special",
        name: "SetTodoList",
        result: {
          content: "updated",
          is_error: false,
          details: {
            todos: [{ title: "发布回归", status: "done" }],
          },
        },
      },
    });
    emit({ MessageStarted: { turn: 2, id: "special_answer" } });
    emit({ TextDelta: { turn: 2, text: "专用工具已完成。" } });
    emit({ MessageFinished: { turn: 2, id: "special_answer", stop_reason: "EndTurn" } });

    const answer = await screen.findByText("专用工具已完成。");
    const group = answer.closest(".a-msg") as HTMLElement;
    const foldButton = within(group).getByRole("button", { name: /展开工作过程/ });
    await user.click(foldButton);
    const fold = group.querySelector(".turn-fold") as HTMLElement;

    const editButton = within(fold).getByRole("button", { name: /编辑 src\/app.ts/ });
    await user.click(editButton);
    const editLine = editButton.closest(".tool-line") as HTMLElement;
    expect(within(editLine).queryByText("替换前")).toBeNull();
    expect(within(editLine).queryByText("before")).toBeNull();
    expect(within(editLine).queryByText("替换后")).toBeNull();
    expect(within(editLine).queryByText("after")).toBeNull();
    expect(within(editLine).getByText(/替换 1 处 src\/app.ts/)).toBeTruthy();

    const diffButton = within(fold).getByRole("button", { name: /编辑 src\/diff.ts/ });
    await user.click(diffButton);
    const diffLine = diffButton.closest(".tool-line") as HTMLElement;
    expect(within(diffLine).getByText("局部差异：src/diff.ts")).toBeTruthy();
    const localDiff = within(diffLine).getByLabelText("src/diff.ts 的局部差异");
    expect(within(localDiff).getByText("-old").className).toContain("ft-diff-del");
    expect(within(localDiff).getByText("+new").className).toContain("ft-diff-add");
    expect(within(localDiff).getByText("...").className).toContain("ft-diff-separator");
    expect(within(localDiff).queryByText("--- src/diff.ts")).toBeNull();
    expect(within(localDiff).queryByText("+++ src/diff.ts")).toBeNull();
    expect(within(localDiff).queryByText("@@ -1 +1 @@")).toBeNull();
    expect(within(localDiff).getByText("---literal old").className).toContain("ft-diff-del");
    expect(within(localDiff).getByText("+++literal new").className).toContain("ft-diff-add");
    expect(within(diffLine).queryByText("替换前")).toBeNull();
    const changeRatio = within(diffLine).getByRole("img", { name: "新增 1 行，删除 1 行" });
    const changeRatioRow = changeRatio.closest(".tl-change-ratio-row") as HTMLElement;
    expect(within(changeRatioRow).getByText("+1").className).toContain("tl-change-ratio-added");
    expect(within(changeRatioRow).getByText("−1").className).toContain("tl-change-ratio-removed");

    const writeButton = within(fold).getByRole("button", { name: /创建 src\/new.ts/ });
    await user.click(writeButton);
    const writeLine = writeButton.closest(".tool-line") as HTMLElement;
    expect(within(writeLine).getByText("文件内容：src/new.ts")).toBeTruthy();
    expect(within(writeLine).getByText(
      (_, node) => node?.tagName === "CODE" && node.textContent === "export const value = 1;",
    )).toBeTruthy();
    expect(within(writeLine).queryByText("结果正文不应覆盖参数正文")).toBeNull();

    const [todoButton, setTodoButton] = within(fold).getAllByRole("button", { name: /更新任务清单/ });
    await user.click(todoButton);
    const todoLine = todoButton.closest(".tool-line") as HTMLElement;
    expect(within(todoLine).getByLabelText("进行中")).toBeTruthy();
    expect(within(todoLine).getByText("检查展示")).toBeTruthy();
    expect(within(todoLine).getByLabelText("待处理")).toBeTruthy();
    expect(within(todoButton).getByRole("progressbar", { name: "任务进度：0/2" })).toBeTruthy();
    expect(within(todoButton).getByText("0/2")).toBeTruthy();
    expect(within(todoButton).queryByText("已完成 0/2")).toBeNull();

    await user.click(setTodoButton);
    const setTodoLine = setTodoButton.closest(".tool-line") as HTMLElement;
    expect(within(setTodoLine).getByText("发布回归")).toBeTruthy();
    expect(within(setTodoLine).getByLabelText("已完成")).toBeTruthy();
    expect(within(setTodoButton).getByRole("progressbar", { name: "任务进度：1/1" })).toBeTruthy();
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
    expect(within(footer).getByTitle("src/app.ts")).toBeTruthy();
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

  it("shows a body-level file preview, opens Review, and copies the answer", async () => {
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
    expect(within(footer).getByTitle("src/app.ts")).toBeTruthy();
    expect(within(footer).getByText("已编辑 1 个文件")).toBeTruthy();
    expect(footer.querySelector(".ft-summary .ft-add")?.textContent).toBe("+1");
    const fileButton = within(footer).getByRole("button", {
      name: "在 Review 中查看 src/app.ts 的局部差异",
    });
    await user.hover(fileButton);
    const preview = await screen.findByRole("region", { name: "src/app.ts 的局部差异" });
    expect(preview.parentElement).toBe(document.body);
    expect(within(preview).getByText("+c")).toBeTruthy();
    await user.click(fileButton);
    const panel = screen.getByLabelText("会话信息区");
    expect(within(panel).getByRole("tab", { name: "Review" }).getAttribute("aria-selected"))
      .toBe("true");
    expect(panel.querySelector('.review-file[id*="src%2Fapp.ts"]')).not.toBeNull();
    expect(document.body.querySelector(".ft-file-preview")).toBeNull();
    await user.click(within(footer).getByRole("button", { name: "复制回答" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("已修改 app.ts。"));
    await screen.findByRole("button", { name: "已复制" });
  });

  it("shows committed files in batches, floating previews, and Review controls", async () => {
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
    const followUpDiff = [
      "--- a/src/one.ts",
      "+++ b/src/one.ts",
      "@@ -1,1 +1,30 @@",
      ...Array.from({ length: 30 }, (_, index) => `+edit${index}`),
    ].join("\n");

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
                operation: "overwritten",
                added: 1,
                removed: 1,
                diff: "--- src/one.ts\n+++ src/one.ts\n@@ -1,1 +1,1 @@\n---removed body\n+++added body",
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
                path: "src/new file.ts",
                status: "committed",
                operation: "created",
                added: 72,
                removed: 0,
                content: Array.from(
                  { length: 72 },
                  (_, index) => `export const line${index} = ${index};`,
                ).join("\n"),
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
    emit({
      ToolExecutionFinished: {
        turn: 2,
        id: "edit_literal_headers",
        name: "edit",
        result: {
          content: "updated literal body",
          is_error: false,
          details: {
            changes: [
              {
                path: "src/one.ts",
                status: "committed",
                added: 0,
                removed: 0,
                diff: "--- src/one.ts\n+++ src/one.ts\n普通正文，不是差异区块",
              },
            ],
          },
        },
      },
    });
    emit({
      ToolExecutionFinished: {
        turn: 2,
        id: "edit_same_file",
        name: "edit",
        result: {
          content: "updated src/one.ts",
          is_error: false,
          details: {
            changes: [
              {
                path: "src/one.ts",
                status: "committed",
                added: 30,
                removed: 0,
                diff: followUpDiff,
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
    expect(footer.querySelector(".ft-summary .ft-add")?.textContent).toBe("+105");
    expect(footer.querySelector(".ft-summary .ft-del")?.textContent).toBe("−2");
    expect(within(footer).getAllByRole("listitem")).toHaveLength(3);
    expect(within(footer).queryByTitle("src/new file.ts")).toBeNull();
    expect(within(footer).queryByTitle("src/skipped.ts")).toBeNull();

    const moreButton = within(footer).getByRole("button", { name: "显示其余 1 个文件" });
    const fileListId = moreButton.getAttribute("aria-controls");
    expect(moreButton.getAttribute("aria-expanded")).toBe("false");
    expect(fileListId).toBeTruthy();
    expect(document.getElementById(fileListId ?? "")).not.toBeNull();
    await user.click(moreButton);
    expect(within(footer).getAllByRole("listitem")).toHaveLength(4);
    expect(within(footer).getByTitle("src/new file.ts")).toBeTruthy();
    expect(within(footer).queryByTitle("src/skipped.ts")).toBeNull();

    const createdButton = within(footer).getByRole("button", {
      name: "在 Review 中查看 src/new file.ts 的新建文件内容",
    });
    expect(createdButton.getAttribute("aria-describedby")).toBeNull();
    await user.hover(createdButton);
    const createdPreview = await screen.findByRole("region", {
      name: "src/new file.ts 的文件内容",
    });
    const createdPreviewId = createdButton.getAttribute("aria-describedby");
    expect(createdPreview.id).toBe(createdPreviewId);
    expect(/\s/.test(createdPreviewId ?? "")).toBe(false);
    expect(createdPreview.parentElement).toBe(document.body);
    expect(createdPreview.style.maxHeight).toMatch(/px$/);
    expect(within(createdPreview).getByText("export const line0 = 0;")).toBeTruthy();
    expect(within(createdPreview).getByText("export const line39 = 39;")).toBeTruthy();
    expect(within(createdPreview).queryByText("export const line40 = 40;")).toBeNull();
    expect(within(createdPreview).getByText("其余内容未显示")).toBeTruthy();
    const createdLine = within(createdPreview).getByText("export const line39 = 39;")
      .closest(".ft-preview-line") as HTMLElement;
    expect(createdLine.querySelector(".ft-line-new")?.textContent).toBe("40");
    await user.unhover(createdButton);
    await waitFor(() => expect(screen.queryByRole("region", {
      name: "src/new file.ts 的文件内容",
    })).toBeNull());

    const diffButton = within(footer).getByRole("button", {
      name: "在 Review 中查看 src/one.ts 的局部差异",
    });
    await user.hover(diffButton);
    const diffPreview = await screen.findByRole("region", { name: "src/one.ts 的局部差异" });
    expect(diffPreview.parentElement).toBe(document.body);
    expect(within(diffPreview).queryByText("--- a/src/one.ts")).toBeNull();
    expect(within(diffPreview).queryByText("+++ b/src/one.ts")).toBeNull();
    expect(within(diffPreview).getByText("---removed body")).toBeTruthy();
    expect(within(diffPreview).getByText("+++added body")).toBeTruthy();
    expect(within(diffPreview).getByText("--- src/one.ts")).toBeTruthy();
    expect(within(diffPreview).getByText("+++ src/one.ts")).toBeTruthy();
    const removedLine = within(diffPreview).getByText("---removed body")
      .closest(".ft-preview-line") as HTMLElement;
    const addedLine = within(diffPreview).getByText("+++added body")
      .closest(".ft-preview-line") as HTMLElement;
    expect(removedLine.querySelector(".ft-line-old")?.textContent).toBe("1");
    expect(removedLine.querySelector(".ft-line-new")?.textContent).toBe("");
    expect(addedLine.querySelector(".ft-line-old")?.textContent).toBe("");
    expect(addedLine.querySelector(".ft-line-new")?.textContent).toBe("1");
    await user.unhover(diffButton);
    await waitFor(() => expect(screen.queryByRole("region", {
      name: "src/one.ts 的局部差异",
    })).toBeNull());

    await user.click(within(footer).getByRole("button", { name: "收起其余文件" }));
    expect(within(footer).getAllByRole("listitem")).toHaveLength(3);

    await user.click(within(footer).getByRole("button", {
      name: "在 Review 中查看 src/one.ts 的局部差异",
    }));
    const panel = screen.getByLabelText("会话信息区");
    expect(within(panel).getByRole("tab", { name: "Review" }).getAttribute("aria-selected"))
      .toBe("true");
    expect(within(panel).getByLabelText("修改文件树")).toBeTruthy();
    expect(within(panel).getAllByRole("table", { name: "统一差异" }).length).toBeGreaterThan(0);

    await user.click(within(panel).getByRole("button", { name: "左右差异" }));
    expect(within(panel).getAllByRole("table", { name: "左右差异" }).length).toBeGreaterThan(0);
    await user.click(within(panel).getByRole("button", { name: "统一差异" }));

    await user.click(within(panel).getByRole("button", { name: "全部收起" }));
    expect(panel.querySelectorAll(".review-file-body")).toHaveLength(0);
    await user.click(within(panel).getByRole("button", { name: "全部展开" }));
    expect(panel.querySelectorAll(".review-file-body")).toHaveLength(4);

    const jumpButton = within(panel).getByRole("button", { name: "跳转文件" });
    expect(jumpButton.getAttribute("aria-expanded")).toBe("false");
    await user.click(jumpButton);
    expect(jumpButton.getAttribute("aria-expanded")).toBe("true");
    const jump = within(panel).getByRole("dialog", { name: "跳转文件" });
    await user.type(within(jump).getByRole("textbox", { name: "搜索文件" }), "two.ts");
    await user.click(within(jump).getByRole("button", { name: /two.ts/ }));
    expect(jumpButton.getAttribute("aria-expanded")).toBe("false");
    expect(within(panel).getByRole("treeitem", { name: "two.ts" }).getAttribute("aria-selected"))
      .toBe("true");
    expect(within(panel).getByRole("treeitem", { name: "one.ts" })).toBeTruthy();

    const optionsButton = within(panel).getByRole("button", { name: "更多 Review 选项" });
    expect(optionsButton.getAttribute("aria-expanded")).toBe("false");
    await user.click(optionsButton);
    expect(optionsButton.getAttribute("aria-expanded")).toBe("true");
    const menu = within(panel).getByRole("menu", { name: "Review 选项" });
    expect(within(menu).getByRole("menuitem", { name: "刷新" })).toBeTruthy();
    expect(within(menu).getByRole("menuitem", { name: "启用换行" })).toBeTruthy();
    expect(within(menu).getByRole("menuitem", { name: "加载完整文件" })).toBeTruthy();
    expect(within(menu).getByRole("menuitem", { name: "启用字级差异" })).toBeTruthy();
    expect(within(menu).getByRole("menuitem", { name: "隐藏空白改动" })).toBeTruthy();
    expect(within(menu).getByRole("menuitem", { name: "复制应用命令" })).toBeTruthy();
    await user.click(within(menu).getByRole("menuitem", { name: "刷新" }));
    expect(optionsButton.getAttribute("aria-expanded")).toBe("false");
    expect(within(panel).getByRole("status").textContent).toContain("已刷新当前转录中的修改");
  });

  it("renders Markdown edits as real changes in both Review layouts", async () => {
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

    emit({
      ToolExecutionFinished: {
        turn: 2,
        id: "markdown_edit",
        name: "edit",
        result: {
          content: "updated docs/guide.md",
          is_error: false,
          details: {
            changes: [{
              path: "docs/guide.md",
              status: "committed",
              added: 1,
              removed: 1,
              diff: "--- docs/guide.md\n+++ docs/guide.md\n@@ -1,3 +1,3 @@\n # 更新说明\n \n-旧段落\n+新增段落\n",
            }],
          },
        },
      },
    });
    emit({ MessageStarted: { turn: 2, id: "markdown_result" } });
    emit({ TextDelta: { turn: 2, text: "已更新说明文档。" } });
    emit({ MessageFinished: { turn: 2, id: "markdown_result", stop_reason: "EndTurn" } });

    const answer = await screen.findByText("已更新说明文档。");
    const footer = answer.closest(".a-msg")?.querySelector(".answer-ft") as HTMLElement;
    await user.click(within(footer).getByRole("button", {
      name: "在 Review 中查看 docs/guide.md 的局部差异",
    }));

    const panel = screen.getByLabelText("会话信息区");
    const unified = within(panel).getByRole("table", { name: "统一差异" });
    const unifiedDeleted = within(unified).getByText("-旧段落").closest(".review-diff-line") as HTMLElement;
    const unifiedAdded = within(unified).getByText("+新增段落").closest(".review-diff-line") as HTMLElement;
    expect(unifiedDeleted.className).toContain("ft-diff-del");
    expect(unifiedAdded.className).toContain("ft-diff-add");

    await user.click(within(panel).getByRole("button", { name: "左右差异" }));
    const split = within(panel).getByRole("table", { name: "左右差异" });
    const splitDeleted = within(split).getByText("-旧段落").closest(".review-split-side") as HTMLElement;
    const splitAdded = within(split).getByText("+新增段落").closest(".review-split-side") as HTMLElement;
    expect(splitDeleted.className).toContain("ft-diff-del");
    expect(splitAdded.className).toContain("ft-diff-add");
  });

  it("shows a focus preview and routes clicks to Review without inline scrolling", async () => {
    const user = userEvent.setup();
    const { socket } = await openSession1();
    const scrollIntoView = vi.fn();
    const originalScrollIntoView = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "scrollIntoView");
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
      configurable: true,
      value: scrollIntoView,
    });

    try {
      emitFinishedEditTurn(socket);
      const answer = await screen.findByText("已修改 app.ts。");
      const footer = answer.closest(".a-msg")?.querySelector(".answer-ft") as HTMLElement;
      const previewButton = within(footer).getByRole("button", {
        name: "在 Review 中查看 src/app.ts 的局部差异",
      });

      previewButton.focus();
      const preview = await screen.findByRole("region", { name: "src/app.ts 的局部差异" });
      expect(preview.parentElement).toBe(document.body);
      expect(scrollIntoView).not.toHaveBeenCalled();

      await user.click(previewButton);
      expect(screen.getByLabelText("会话信息区").querySelector('[role="tab"][aria-selected="true"]')
        ?.textContent).toContain("Review");
      expect(document.body.querySelector(".ft-file-preview")).toBeNull();
      expect(scrollIntoView).not.toHaveBeenCalled();
    } finally {
      if (originalScrollIntoView) {
        Object.defineProperty(HTMLElement.prototype, "scrollIntoView", originalScrollIntoView);
      } else {
        delete (HTMLElement.prototype as { scrollIntoView?: unknown }).scrollIntoView;
      }
    }
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
    expect(line.querySelector('.tl-result-ic[data-status-icon="running"]')).not.toBeNull();
    expect(line.querySelector(".agent-pill")).toBeNull();
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
    expect(toolBar.textContent).not.toContain("排队等待");
    expect(toolBar.closest(".tool-line")?.querySelector(".line-tail")).toBeNull();
    expect(toolBar.closest(".tool-line")?.querySelector('[data-status-icon="queued"]')).not.toBeNull();

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

  it("keeps single-choice other answers exclusive and restores a failed submission", async () => {
    const user = userEvent.setup();
    let questionAttempts = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
        const url =
          typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
        if (url === "/api/sessions/session_0001/question" && init?.method === "POST") {
          questionAttempts += 1;
          if (questionAttempts === 1) {
            void mockFetch(input, init);
            return Promise.resolve(
              new Response(JSON.stringify({ code: "internal" }), {
                status: 500,
                headers: { "content-type": "application/json" },
              }),
            );
          }
        }
        return mockFetch(input, init);
      }),
    );
    const { socket } = await openSession1();
    socket.emit({
      type: "session_state",
      stream_id: fixture.stream_id,
      session_id: "session_0001",
      sequence: session1.snapshot.watermark + 1,
      event: { ...session1.snapshot.session, waiting_question: true },
    });
    await waitFor(() =>
      expect(socket.watchSessionIds()).toEqual(["session_0001", "session_0001"]),
    );
    const snapshot = structuredClone(session1.snapshot);
    snapshot.watermark += 1;
    snapshot.session.waiting_question = true;
    snapshot.pending_questions = [
      {
        id: "question_live",
        turn_id: "turn_01",
        questions: [
          {
            header: "界面",
            question: "使用哪种界面？",
            options: [{ label: "深色" }, { label: "浅色" }],
            multi_select: false,
          },
          {
            header: "检查",
            question: "要执行哪些检查？",
            options: [{ label: "测试" }, { label: "审查" }],
            multi_select: true,
          },
        ],
      },
    ];
    socket.emit({ type: "session_snapshot", snapshot });

    const question = await screen.findByRole("group", { name: "提问" });
    const dark = within(question).getByRole("button", { name: "深色" });
    const light = within(question).getByRole("button", { name: "浅色" });
    const test = within(question).getByRole("button", { name: "测试" });
    const review = within(question).getByRole("button", { name: "审查" });
    const singleOther = within(question).getByRole("textbox", {
      name: "第 1 题的其他回答（可选）",
    });
    const multiOther = within(question).getByRole("textbox", {
      name: "第 2 题的其他回答（可选）",
    });

    await user.click(dark);
    await user.type(singleOther, "跟随系统");
    expect(dark.getAttribute("aria-pressed")).toBe("false");
    await user.click(light);
    expect(singleOther).toHaveProperty("value", "");
    await user.type(singleOther, "高对比");
    expect(light.getAttribute("aria-pressed")).toBe("false");

    await user.click(test);
    await user.click(review);
    await user.type(multiOther, "类型检查");
    expect(test.getAttribute("aria-pressed")).toBe("true");
    expect(review.getAttribute("aria-pressed")).toBe("true");
    const submit = within(question).getByRole("button", { name: "提交回答" });
    await user.click(submit);
    await waitFor(() => expect(questionAttempts).toBe(1));
    await screen.findByText("网络请求失败，请稍后重试。");
    await waitFor(() => expect(submit).toHaveProperty("disabled", false));

    await user.click(submit);
    await waitFor(() => expect(questionAttempts).toBe(2));
    const requests = recordedRequests.filter(
      (entry) => entry.url === "/api/sessions/session_0001/question",
    );
    expect(requests).toHaveLength(2);
    expect(requests[0].body).toEqual({
      turn_id: "turn_01",
      question_id: "question_live",
      answer: { selections: ["高对比", "测试, 审查, 类型检查"] },
    });
    expect(submit).toHaveProperty("disabled", true);
  });

  it("keeps concurrent question batches ordered and independently answerable", async () => {
    const user = userEvent.setup();
    const { socket } = await openSession1();
    socket.emit({
      type: "session_state",
      stream_id: fixture.stream_id,
      session_id: "session_0001",
      sequence: session1.snapshot.watermark + 1,
      event: { ...session1.snapshot.session, waiting_question: true },
    });
    await waitFor(() =>
      expect(socket.watchSessionIds()).toEqual(["session_0001", "session_0001"]),
    );

    const snapshot = structuredClone(session1.snapshot);
    snapshot.watermark += 1;
    snapshot.session.waiting_question = true;
    snapshot.pending_questions = [
      {
        id: "question_first",
        turn_id: "turn_01",
        questions: [
          {
            question: "选择界面？",
            options: [{ label: "深色" }, { label: "浅色" }],
            multi_select: false,
          },
        ],
      },
      {
        id: "question_second",
        turn_id: "turn_01",
        questions: [
          {
            header: "检查",
            question: "执行检查？",
            options: [{ label: "测试" }, { label: "跳过" }],
            multi_select: false,
          },
        ],
      },
    ];
    socket.emit({ type: "session_snapshot", snapshot });

    let questions = await screen.findAllByRole("group", { name: "提问" });
    expect(questions).toHaveLength(2);
    expect(questions[0].textContent).toContain("选择界面？");
    expect(questions[1].textContent).toContain("执行检查？");
    await user.click(within(questions[0]).getByRole("button", { name: "深色" }));
    await user.click(within(questions[0]).getByRole("button", { name: "提交回答" }));
    await waitFor(() =>
      expect(
        recordedRequests.filter(
          (entry) => entry.url === "/api/sessions/session_0001/question",
        ),
      ).toHaveLength(1),
    );

    socket.emit({
      type: "session_state",
      stream_id: fixture.stream_id,
      session_id: "session_0001",
      sequence: snapshot.watermark + 1,
      event: { ...snapshot.session, waiting_question: true },
    });
    await waitFor(() => expect(socket.watchSessionIds()).toHaveLength(3));
    const refreshed = structuredClone(snapshot);
    refreshed.watermark += 1;
    refreshed.pending_questions = [snapshot.pending_questions[1]];
    socket.emit({ type: "session_snapshot", snapshot: refreshed });

    await waitFor(() => expect(screen.queryByText("选择界面？")).toBeNull());
    questions = screen.getAllByRole("group", { name: "提问" });
    expect(questions).toHaveLength(1);
    expect(questions[0].textContent).toContain("执行检查？");
    await user.click(within(questions[0]).getByRole("button", { name: "测试" }));
    await user.click(within(questions[0]).getByRole("button", { name: "提交回答" }));

    await waitFor(() => {
      const requests = recordedRequests.filter(
        (entry) => entry.url === "/api/sessions/session_0001/question",
      );
      expect(requests).toHaveLength(2);
      expect(requests.map((entry) => entry.body)).toEqual([
        {
          turn_id: "turn_01",
          question_id: "question_first",
          answer: { selections: ["深色"] },
        },
        {
          turn_id: "turn_01",
          question_id: "question_second",
          answer: { selections: ["测试"] },
        },
      ]);
    });
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
    expect(document.querySelectorAll(".task-status-indicator")).toHaveLength(2);
    expect(document.querySelector(".task-item.status-in_progress .task-status-dot")).not.toBeNull();
    expect(document.querySelector(".task-item.status-done .task-status-indicator svg")).not.toBeNull();
    expect(document.querySelectorAll(".task-state")).toHaveLength(0);
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

  it("shows initial connection status when reopening unloaded history during reconnect", async () => {
    const { socket } = await openSession1();

    (await screen.findByText("并行格式化")).click();
    await waitFor(() => expect(socket.watchSessionIds()).toEqual(["session_0001", "session_0002"]));
    socket.emit({ type: "session_snapshot", snapshot: fixture.sessions[1].snapshot });
    await screen.findByText("另一个会话并行跑格式化。");
    expect(screen.queryByText("检查有界中继的行为测试并修复慢连接。")).toBeNull();

    FakeWebSocket.autoOpen = false;
    socket.closeWith(1013);
    await screen.findByText("连接已断开，正在重连…");

    (await screen.findByText("有界中继测试")).click();
    await screen.findByText("正在连接…");
    expect(screen.queryByText("连接已断开，正在重连…")).toBeNull();
  });

  it("shows initial connection status while an open socket loads an unseen history snapshot", async () => {
    const { socket } = await openSession1();

    (await screen.findByText("并行格式化")).click();
    await waitFor(() => expect(socket.watchSessionIds()).toEqual(["session_0001", "session_0002"]));
    await screen.findByText("正在连接…");
    expect(screen.queryByText("连接已断开，正在重连…")).toBeNull();

    socket.emit({ type: "session_snapshot", snapshot: fixture.sessions[1].snapshot });
    await screen.findByText("另一个会话并行跑格式化。");
    expect(screen.queryByText("正在连接…")).toBeNull();
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
    expect(bar.textContent).not.toContain("排队等待");
    expect(bar.textContent).not.toContain("位置 2");
    expect(bar.textContent).not.toContain("已完成");
    const line = bar.closest(".tool-line") as HTMLElement;
    expect(line.querySelector('[data-status-icon="queued"]')).not.toBeNull();
    expect(line.querySelector('[data-status-icon="finished"]')).toBeNull();
    expect(line.querySelector(".line-tail")).toBeNull();
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
