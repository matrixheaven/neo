/**
 * Transcript projection tests driven by the fixed sample: snapshot rebuild,
 * watermark resume, duplicate dedup, retry retraction, in-place updates by
 * stable id, unknown tags and append coverage.
 */

import { fireEvent, render } from "@testing-library/react";
import React from "react";
import { describe, expect, it } from "vitest";
import { agentEventTag, type AgentEvent } from "../../src/protocol";
import {
  groupTurns,
  presentationItems,
  processPresentationItems,
  TranscriptDocument,
} from "../../src/components/transcript";
import {
  readGroupStatusForItems,
  TranscriptItemView,
  toolPresentation,
} from "../../src/components/transcriptItems";
import {
  applyAgentEvent,
  buildFromHistory,
  emptyProjection,
  type ApprovalItem,
  type AssistantMessageItem,
  type DelegateItem,
  type QuestionItem,
  type RetryItem,
  type SwarmItem,
  type TerminalItem,
  type ThinkingItem,
  type TranscriptItem,
  type ToolItem,
  type UnknownItem,
  type UserMessageItem,
  type WorkflowItem,
} from "../../src/state/transcript";
import { asServerMessage, loadFixture } from "./fixture";
import { appReducer } from "../../src/state/reducer";
import { initialAppState, type AppState } from "../../src/state/appState";
import { AppProvider } from "../../src/state/store";

const fixture = loadFixture();
const session1 = fixture.sessions[0];
const session2 = fixture.sessions[1];

describe("tool presentation", () => {
  it("maps known tool calls, prioritizes Read filenames and preserves unknown arguments", () => {
    const cases = [
      ["Read", { path: "src/ui/app.ts" }, "读取", "app.ts", "src/ui/", "file"],
      ["List", { path: "src/ui" }, "读取", "ui", "src/", "file"],
      ["ReadMediaFile", { path: "assets/preview.png" }, "观察", "preview.png", "assets/", "media"],
      ["Edit", { path: "src/app.ts" }, "编辑", "src/app.ts", undefined, "file-edit"],
      ["Write", { path: "src/new.ts" }, "创建", "src/new.ts", undefined, "file-plus"],
      ["Grep", { pattern: "tool", directory: "src" }, "搜索", "tool", "src", "search"],
      ["Glob", { pattern: "*.ts", directory: "src" }, "搜索", "*.ts", "src", "folder-search"],
      ["Find", { pattern: "*.tsx", directory: "src" }, "搜索", "*.tsx", "src", "folder-search"],
      ["Bash", { command: "cargo test" }, "运行", "cargo test", undefined, "terminal"],
      ["Shell", { command: "git status" }, "运行", "git status", undefined, "terminal"],
      ["Terminal", { command: "npm run dev" }, "启动终端", "npm run dev", undefined, "terminal"],
      ["Skill", { skill: "release" }, "使用技能", "release", undefined, "skill"],
      [
        "MoveSkill",
        { skill: "release", destination_parent: "~/.neo/skills" },
        "移动技能",
        "release",
        "~/.neo/skills",
        "skill",
      ],
      ["AskUserQuestion", { questions: [] }, "询问用户", "", undefined, "question"],
      ["TaskList", { active_only: true }, "查看后台任务", "", undefined, "todo"],
      ["TaskOutput", { task_id: "task_1" }, "查看任务输出", "task_1", undefined, "todo"],
      ["TaskStop", { task_id: "task_1" }, "停止后台任务", "task_1", undefined, "stop"],
      ["TaskPause", { task_id: "task_1" }, "暂停后台任务", "task_1", undefined, "wait"],
      ["TaskResume", { task_id: "task_1" }, "恢复后台任务", "task_1", undefined, "wait"],
      ["TaskAnswer", { task_id: "task_1" }, "回答后台任务", "task_1", undefined, "message"],
      ["EnterPlanMode", {}, "进入计划模式", "", undefined, "workflow"],
      ["ExitPlanMode", {}, "退出计划模式", "", undefined, "workflow"],
      ["Delegate", { task: "检查样式" }, "派发子代理", "检查样式", undefined, "delegate"],
      ["DelegateGroup", { task: "并行检查" }, "派发子代理", "并行检查", undefined, "delegate"],
      ["DelegateSwarm", { task: "并行检查" }, "派发子代理", "并行检查", undefined, "swarm"],
      ["ListDelegates", {}, "查看子代理", "", undefined, "delegate"],
      ["TodoList", {}, "更新任务清单", "", undefined, "todo"],
      ["SetTodoList", {}, "更新任务清单", "", undefined, "todo"],
      ["WaitDelegate", { ids: ["agent_a", "agent_b"] }, "等待子代理", "agent_a, agent_b", undefined, "wait"],
      ["InterruptDelegate", { agent_id: "agent_a" }, "停止子代理", "", undefined, "stop"],
      ["MessageDelegate", { message: "继续", agent_id: "agent_a" }, "联系子代理", "继续", undefined, "message"],
      ["Sleep", { reason: "等待构建", duration_seconds: 5 }, "等待", "等待构建", undefined, "wait"],
      ["Workflow", { action: "run_saved", name: "nightly" }, "运行工作流", "nightly", undefined, "workflow"],
    ] as const;

    for (const [name, args, action, target, secondary, icon] of cases) {
      const presentation = toolPresentation(name, args);
      expect(presentation).toMatchObject({ action, target, icon });
      expect(presentation.secondary).toBe(secondary);
    }

    expect(toolPresentation("FutureTool", { scope: "test" })).toMatchObject({
      action: "FutureTool",
      target: '{"scope":"test"}',
      icon: "unknown",
    });
    expect(toolPresentation("Read", { path: "C:\\repo\\src\\app.ts" })).toMatchObject({
      action: "读取",
      target: "app.ts",
      secondary: "C:\\repo\\src\\",
      icon: "file",
    });
  });
});

describe("transcript presentation", () => {
  it("prioritizes failed and running states when summarizing read groups", () => {
    const read = (id: string, status: ToolItem["status"]): ToolItem => ({
      kind: "tool",
      id,
      name: "Read",
      arguments: { path: `src/${id}.ts` },
      status,
      turn: 1,
    });

    expect(readGroupStatusForItems([read("done", "finished")])).toEqual({
      status: "finished",
      text: "已完成",
    });
    expect(readGroupStatusForItems([read("queued", "queued")])).toEqual({
      status: "running",
      text: "读取中",
    });
    expect(readGroupStatusForItems([read("running", "running"), read("done", "finished")])).toEqual({
      status: "running",
      text: "读取中",
    });
    expect(readGroupStatusForItems([read("done", "finished"), read("failed", "failed")])).toEqual({
      status: "failed",
      text: "部分失败",
    });
    expect(readGroupStatusForItems([{
      ...read("errored", "finished"),
      result: { content: "读取失败", is_error: true },
    }])).toEqual({
      status: "failed",
      text: "部分失败",
    });
  });

  it("keeps unresolved interactions in order and renders only the paired command runtime", () => {
    const items: TranscriptItem[] = [
      { kind: "user_message", id: "user:1", text: "继续任务" },
      {
        kind: "thinking",
        id: "think:1",
        text: "分析中",
        finished: true,
        redacted: false,
        turn: 1,
      },
      {
        kind: "tool",
        id: "tool:command_1",
        name: "Bash",
        arguments: { command: "npm test" },
        status: "finished",
        turn: 1,
      },
      {
        kind: "question",
        id: "question:1",
        turn: 1,
        questions: [{
          header: "确认",
          question: "继续吗？",
          options: [{ label: "继续" }],
          multi_select: false,
        }],
        resolved: false,
      },
      {
        kind: "shell",
        id: "shell:command_1",
        command: "npm test",
        cwd: "/workspace",
        status: "finished",
        stdout: "通过",
        stderr: "",
        truncated: false,
        exitCode: 0,
        turn: 1,
      },
      {
        kind: "tool",
        id: "tool:read_1",
        name: "Read",
        arguments: { path: "src/app.ts" },
        status: "finished",
        turn: 1,
      },
      {
        kind: "approval",
        id: "approval:1",
        request: {
          turn: 1,
          id: "approval_1",
          operation: "bash",
          presentation: { kind: "command", title: "运行测试", command: "npm test" },
          options: [{ label: "允许", action: { kind: "approve" } }],
        },
      },
      {
        kind: "assistant_message",
        id: "assistant:1",
        text: "最终回复",
        finished: true,
        turn: 1,
      },
    ];

    const groups = groupTurns(items);
    expect(groups.map((group) => group.kind)).toEqual(["user", "assist"]);
    const assist = groups[1];
    expect(assist?.kind).toBe("assist");
    if (!assist || assist.kind !== "assist") return;
    expect(assist.activity.map((item) => item.id)).toEqual([
      "think:1",
      "tool:command_1",
      "question:1",
      "shell:command_1",
      "tool:read_1",
      "approval:1",
    ]);
    expect(assist.process.map((item) => item.id)).toEqual([
      "think:1",
      "tool:command_1",
      "shell:command_1",
      "tool:read_1",
    ]);
    expect(assist.msg.id).toBe("assistant:1");

    const view = render(
      React.createElement(
        AppProvider,
        null,
        React.createElement(TranscriptDocument, { sessionId: "test", items }),
      ),
    );
    const question = view.getByRole("group", { name: "提问" });
    const approval = view.getByRole("group", { name: "审批请求：运行测试" });
    const commandRows = Array.from(view.container.querySelectorAll(".tool-line"))
      .filter((row) => row.querySelector(".tl-mono")?.textContent === "npm test");
    const runtime = commandRows[0];
    const answer = view.getByText("最终回复").closest(".msg") as HTMLElement;
    expect(question.closest(".tf-body")).toBeNull();
    expect(approval.closest(".tf-body")).toBeNull();
    expect(view.container.querySelectorAll(".question-row")).toHaveLength(1);
    expect(view.container.querySelectorAll(".approval-row:not(.question-row)")).toHaveLength(1);
    expect(commandRows).toHaveLength(1);
    expect(runtime.classList.contains("kind-shell")).toBe(true);
    expect((view.getByRole("button", { name: "继续" }) as HTMLButtonElement).disabled).toBe(false);
    expect((view.getByRole("button", { name: "允许" }) as HTMLButtonElement).disabled).toBe(false);
    expect(question.compareDocumentPosition(runtime) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0);
    expect(runtime.compareDocumentPosition(approval) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0);
    expect(approval.compareDocumentPosition(answer) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0);
    view.unmount();
  });

  it("treats result errors as failed TodoList aliases without progress or completed tasks", () => {
    for (const [index, name] of ["TodoList", "SetTodoList"].entries()) {
      const id = `todo_${index}`;
      let projection = emptyProjection();
      projection = applyAgentEvent(projection, {
        ToolExecutionStarted: {
          turn: 1,
          id,
          name,
          arguments: {
            todos: [{ title: "不应显示为成功进度", status: "done" }],
          },
        },
      });
      projection = applyAgentEvent(projection, {
        ToolExecutionFinished: {
          turn: 1,
          id,
          name,
          result: { content: "任务清单无效", is_error: true },
        },
      });
      const item = projection.items.find(
        (entry): entry is ToolItem => entry.kind === "tool" && entry.id === `tool:${id}`,
      ) as ToolItem;
      const view = render(
        React.createElement(
          AppProvider,
          null,
          React.createElement(TranscriptItemView, { sessionId: "test", item }),
        ),
      );

      expect(item.status).toBe("failed");
      expect(view.container.querySelector(".tool-line.status-failed")).not.toBeNull();
      expect(view.container.querySelector('[role="progressbar"]')).toBeNull();
      expect(view.container.textContent).toContain("任务清单无效");
      view.unmount();

      const staleStatusItem: ToolItem = {
        ...item,
        status: "finished",
        result: {
          content: "任务清单无效",
          is_error: true,
          details: { todos: [{ title: "不应显示为完成任务", status: "done" }] },
        },
      };
      const staleView = render(
        React.createElement(
          AppProvider,
          null,
          React.createElement(TranscriptItemView, { sessionId: "test", item: staleStatusItem }),
        ),
      );

      expect(staleView.container.querySelector(".tool-line.status-failed")).not.toBeNull();
      expect(staleView.container.querySelector('[data-status-icon="failed"]')).not.toBeNull();
      expect(staleView.container.querySelector('[role="progressbar"]')).toBeNull();
      fireEvent.click(staleView.getByRole("button", { name: /更新任务清单/ }));
      expect(staleView.container.querySelector(".tl-todos")).toBeNull();
      expect(staleView.container.textContent).toContain("任务清单无效");
      expect(staleView.container.textContent).not.toContain("不应显示为完成任务");
      staleView.unmount();
    }
  });

  it("shows only the skill name and destination for MoveSkill errors", () => {
    const source = "/private/workspace/skills/release";
    const item: ToolItem = {
      kind: "tool",
      id: "tool:move_skill",
      name: "MoveSkill",
      arguments: { source, destination_parent: "~/.neo/skills" },
      status: "finished",
      result: { content: `无法移动 ${source}`, is_error: true },
      output: { id: "output_move_skill", byte_len: 200, line_count: 2, complete: true },
      turn: 1,
    };
    const view = render(
      React.createElement(
        AppProvider,
        null,
        React.createElement(TranscriptItemView, { sessionId: "test", item }),
      ),
    );

    fireEvent.click(view.getByRole("button", { name: /移动技能 release/ }));
    expect(view.getByText("技能：release")).toBeTruthy();
    expect(view.getByText("目标目录：~/.neo/skills")).toBeTruthy();
    expect(view.getByText("无法移动 release")).toBeTruthy();
    expect(view.queryByText(/完整输出/)).toBeNull();
    expect(view.container.textContent).not.toContain(source);
    view.unmount();
  });

  it("hides delegated control tools when their card already carries the activity", () => {
    const names = ["Delegate", "DelegateGroup", "DelegateSwarm", "WaitDelegate"];
    const controls: ToolItem[] = names.map((name, index) => ({
      kind: "tool",
      id: `tool:control_${index}`,
      name,
      arguments: {},
      status: "finished",
      turn: 1,
    }));
    const delegateCard: TranscriptItem = {
      kind: "delegate",
      id: "delegate:agent_1",
      agent: { id: "agent_1", display_name: "检查", state: "completed" },
      finished: true,
      turn: 1,
    };
    const items: TranscriptItem[] = [
      ...controls,
      {
        kind: "tool",
        id: "tool:read_1",
        name: "Read",
        arguments: { path: "src/app.ts" },
        status: "finished",
        turn: 1,
      },
      delegateCard,
    ];

    expect(presentationItems(items).map((item) => item.id)).toEqual([
      "tool:read_1",
      "delegate:agent_1",
    ]);
    expect(presentationItems(controls).map((item) => item.id)).toEqual(
      controls.map((item) => item.id),
    );
    expect(presentationItems(
      controls,
      [
        ...controls,
        { kind: "question", id: "question:split", turn: 1, questions: [], resolved: false },
        delegateCard,
      ],
    )).toEqual([]);
    expect(processPresentationItems(
      [controls[0]],
      [...controls, { kind: "question", id: "question:split-2", turn: 1, questions: [], resolved: false }, delegateCard],
    )).toEqual([]);
    const failedControl: ToolItem = {
      kind: "tool",
      id: "tool:control_failed",
      name: "WaitDelegate",
      arguments: {},
      status: "failed",
      turn: 1,
    };
    expect(presentationItems([failedControl], [...controls, failedControl, delegateCard])).toEqual([]);
    expect(presentationItems([failedControl])).toEqual([failedControl]);
    const failedControls = names.map((name, index): ToolItem => ({
      kind: "tool",
      id: `tool:failed_${index}`,
      name,
      arguments: {},
      status: "failed",
      turn: 1,
    }));
    expect(presentationItems(failedControls, [...failedControls, delegateCard])).toEqual([]);
    expect(presentationItems(failedControls).map((item) => item.id)).toEqual(
      failedControls.map((item) => item.id),
    );
  });

  it("uses the question row for active and completed AskUserQuestion calls", () => {
    const ask = (id: string, status: ToolItem["status"]): ToolItem => ({
      kind: "tool",
      id,
      name: "AskUserQuestion",
      arguments: { questions: [] },
      status,
      turn: 1,
    });
    const items: TranscriptItem[] = [
      ask("tool:question_running", "running"),
      ask("tool:question_finished", "finished"),
      ask("tool:question_failed", "failed"),
      {
        kind: "question",
        id: "question:question_1",
        turn: 1,
        questions: [],
        resolved: false,
      },
    ];

    expect(presentationItems(items).map((item) => item.id)).toEqual([
      "tool:question_failed",
      "question:question_1",
    ]);
  });

  it("groups only adjacent active reads from the same explicit turn", () => {
    const read = (id: string, status: ToolItem["status"], turn?: number): ToolItem => ({
      kind: "tool",
      id,
      name: "Read",
      arguments: { path: `src/${id}.ts` },
      status,
      turn,
    });
    const items: TranscriptItem[] = [
      read("read_1", "finished", 1),
      read("read_2", "running", 1),
      read("read_3", "finished", 1),
      {
        kind: "tool",
        id: "edit_1",
        name: "Edit",
        arguments: { path: "src/app.ts", old: "a", new: "b" },
        status: "finished",
        turn: 1,
      },
      read("read_failed", "failed", 1),
      read("read_after_failure", "running", 1),
      read("read_after_failure_finished", "finished", 1),
      read("read_turn_2", "finished", 2),
      read("read_turn_3", "finished", 3),
      read("read_unscoped_1", "finished"),
      read("read_unscoped_2", "finished"),
    ];

    const labels = processPresentationItems(items).map((entry) =>
      entry.kind === "read_group"
        ? `group:${entry.items.map((item) => item.id).join(",")}`
        : entry.item.id,
    );
    expect(labels).toEqual([
      "group:read_1,read_2,read_3",
      "edit_1",
      "read_failed",
      "group:read_after_failure,read_after_failure_finished",
      "read_turn_2",
      "read_turn_3",
      "read_unscoped_1",
      "read_unscoped_2",
    ]);
    const runningGroup = processPresentationItems(items).find(
      (entry) => entry.kind === "read_group" && entry.items.some((item) => item.status === "running"),
    );
    expect(runningGroup?.kind).toBe("read_group");
    if (runningGroup?.kind === "read_group") {
      expect(readGroupStatusForItems(runningGroup.items)).toEqual({
        status: "running",
        text: "读取中",
      });
    }
  });
});

function stateWithSnapshot(session = session1): AppState {
  let state = initialAppState(280, "dark");
  state = appReducer(state, { type: "select_session", sessionId: session.session_id });
  state = appReducer(state, {
    type: "server_message",
    message: { type: "session_snapshot", snapshot: session.snapshot },
  });
  return state;
}

function applyEnvelopes(
  state: AppState,
  envelopes: typeof session1.after_snapshot,
): AppState {
  return envelopes.reduce(
    (current, envelope) =>
      appReducer(current, { type: "server_message", message: asServerMessage(envelope) }),
    state,
  );
}

describe("snapshot rebuild", () => {
  it("rebuilds the projection from history up to the watermark", () => {
    const state = stateWithSnapshot();
    const view = state.sessions[session1.session_id];
    expect(view.cursor).toBe(8);
    expect(view.streamId).toBe(fixture.stream_id);
    expect(view.phase).toBe("running");
    expect(view.waitingApproval).toBe(true);
    expect(view.currentTurnId).toBe("turn_01");

    const kinds = view.projection.items.map((item) => item.kind);
    expect(kinds).toEqual(["user_message", "assistant_message", "thinking", "tool", "approval"]);

    const user = view.projection.items[0] as UserMessageItem;
    expect(user.text).toBe("检查有界中继的行为测试并修复慢连接。");

    const assistant = view.projection.items[1] as AssistantMessageItem;
    expect(assistant.text).toBe("我来检查一下有界中继的行为测试。");
    expect(assistant.finished).toBe(false);

    const thinking = view.projection.items[2] as ThinkingItem;
    expect(thinking.text).toBe("先检查有界中继的边界条件。");
    expect(thinking.finished).toBe(true);
    expect(thinking.redacted).toBe(false);

    const tool = view.projection.items[3] as ToolItem;
    expect(tool.status).toBe("queued");

    const approval = view.projection.items[4] as ApprovalItem;
    expect(approval.request.id).toBe("approval_01");
    expect(view.projection.pendingApprovalId).toBe("approval_01");
  });

  it("clears derived state before rebuilding from a new snapshot", () => {
    let state = stateWithSnapshot();
    state = applyEnvelopes(state, session1.after_snapshot.slice(0, 3));
    // A fresh snapshot replaces everything derived, keeping only draft and
    // expansion UI state.
    state = appReducer(state, {
      type: "server_message",
      message: { type: "session_snapshot", snapshot: session1.snapshot },
    });
    const view = state.sessions[session1.session_id];
    expect(view.cursor).toBe(8);
    expect(view.projection.items.map((item) => item.kind)).toEqual([
      "user_message",
      "assistant_message",
      "thinking",
      "tool",
      "approval",
    ]);
    expect(view.projection.todos).toEqual([]);
  });

  it("uses the snapshot waiting items as the final arbiter of pending state", () => {
    // Fixture snapshot: pending_approval present → card stays actionable.
    let state = stateWithSnapshot();
    let view = state.sessions[session1.session_id];
    expect(view.projection.pendingApprovalId).toBe("approval_01");
    let approval = view.projection.items.find(
      (item): item is ApprovalItem => item.kind === "approval",
    ) as ApprovalItem;
    expect(approval.resolution).toBeUndefined();

    // Same history, but the snapshot carries no pending approval: the card
    // remains in place yet must not stay actionable.
    const snapshot = structuredClone(session1.snapshot);
    delete snapshot.pending_approval;
    state = initialAppState(280, "dark");
    state = appReducer(state, { type: "select_session", sessionId: session1.session_id });
    state = appReducer(state, {
      type: "server_message",
      message: { type: "session_snapshot", snapshot },
    });
    view = state.sessions[session1.session_id];
    expect(view.projection.pendingApprovalId).toBeNull();
    approval = view.projection.items.find(
      (item): item is ApprovalItem => item.kind === "approval",
    ) as ApprovalItem;
    expect(approval.resolution?.kind).toBe("no_longer_pending");
  });

  it("marks question cards resolved when the snapshot carries no pending question", () => {
    const snapshot = structuredClone(session1.snapshot);
    snapshot.watermark = 9;
    snapshot.history = [
      ...snapshot.history,
      {
        sequence: 9,
        event: {
          QuestionRequested: {
            turn: 1,
            id: "question_77",
            questions: [
              { question: "继续？", header: "确认", options: [{ label: "是" }], multi_select: false },
            ],
          },
        },
      },
    ];
    snapshot.pending_questions = [];
    let state = initialAppState(280, "dark");
    state = appReducer(state, { type: "select_session", sessionId: session1.session_id });
    state = appReducer(state, {
      type: "server_message",
      message: { type: "session_snapshot", snapshot },
    });
    const view = state.sessions[session1.session_id];
    expect(view.projection.pendingQuestionIds).toEqual([]);
    const question = view.projection.items.find(
      (item): item is QuestionItem => item.kind === "question",
    ) as QuestionItem;
    expect(question.resolved).toBe(true);
  });

  it("hydrates pending questions in stable order and resolves each independently", () => {
    let state = stateWithSnapshot();
    state = appReducer(state, {
      type: "server_message",
      message: {
        type: "session_state",
        stream_id: fixture.stream_id,
        session_id: session1.session_id,
        sequence: session1.snapshot.watermark + 1,
        event: {
          ...session1.snapshot.session,
          waiting_question: true,
        },
      },
    });
    expect(state.sessions[session1.session_id].resyncNeeded).toBe(true);

    const snapshot = structuredClone(session1.snapshot);
    snapshot.watermark += 1;
    snapshot.session.waiting_question = true;
    snapshot.pending_questions = [
      {
        id: "question_first",
        turn_id: "turn_01",
        questions: [
          {
            question: "使用哪种界面？",
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
            question: "执行哪些检查？",
            options: [{ label: "测试" }, { label: "审查" }],
            multi_select: true,
          },
        ],
      },
    ];
    state = appReducer(state, {
      type: "server_message",
      message: { type: "session_snapshot", snapshot },
    });
    let view = state.sessions[session1.session_id];
    let questions = view.projection.items.filter(
      (item): item is QuestionItem => item.kind === "question",
    );
    expect(questions.map((question) => question.id)).toEqual([
      "question:question_first",
      "question:question_second",
    ]);
    expect(questions.every((question) => !question.resolved)).toBe(true);
    expect(view.projection.pendingQuestionIds).toEqual([
      "question_first",
      "question_second",
    ]);
    expect(view.resyncNeeded).toBe(false);

    state = appReducer(state, {
      type: "server_message",
      message: {
        type: "session_state",
        stream_id: fixture.stream_id,
        session_id: session1.session_id,
        sequence: snapshot.watermark + 1,
        event: { ...snapshot.session, waiting_question: true },
      },
    });
    expect(state.sessions[session1.session_id].resyncNeeded).toBe(true);

    const refreshed = structuredClone(snapshot);
    refreshed.watermark += 1;
    refreshed.pending_questions = [snapshot.pending_questions[1]];
    state = appReducer(state, {
      type: "server_message",
      message: { type: "session_snapshot", snapshot: refreshed },
    });
    view = state.sessions[session1.session_id];
    questions = view.projection.items.filter(
      (item): item is QuestionItem => item.kind === "question",
    );
    expect(questions).toHaveLength(1);
    expect(questions[0].id).toBe("question:question_second");
    expect(questions[0].resolved).toBe(false);
    expect(view.projection.pendingQuestionIds).toEqual(["question_second"]);
    expect(view.resyncNeeded).toBe(false);

    state = appReducer(state, {
      type: "server_message",
      message: {
        type: "session_state",
        stream_id: fixture.stream_id,
        session_id: session1.session_id,
        sequence: refreshed.watermark + 1,
        event: { ...refreshed.session, waiting_question: false },
      },
    });
    view = state.sessions[session1.session_id];
    const resolved = view.projection.items.find(
      (item): item is QuestionItem => item.kind === "question",
    ) as QuestionItem;
    expect(resolved.resolved).toBe(true);
    expect(view.projection.pendingQuestionIds).toEqual([]);
  });
});

describe("event application after snapshot", () => {
  it("updates queued tools, questions, tasks, approvals and tool lifecycle in place", () => {
    let state = stateWithSnapshot();
    state = applyEnvelopes(state, session1.after_snapshot);
    const view = state.sessions[session1.session_id];
    expect(view.cursor).toBe(17);

    const tool = view.projection.items.find(
      (item): item is ToolItem => item.kind === "tool",
    ) as ToolItem;
    expect(tool.status).toBe("finished");
    expect(tool.result?.content).toContain("42 passed");
    expect(tool.output?.id).toBe("eyJhZ2VudF9pZCI6Im1haW4iLCJ0YXNrX2lkIjoidGFza18wMSJ9");
    expect(tool.output?.complete).toBe(true);

    const approval = view.projection.items.find(
      (item): item is ApprovalItem => item.kind === "approval",
    ) as ApprovalItem;
    expect(approval.resolution?.kind).toBe("selected");
    expect(view.projection.pendingApprovalId).toBeNull();

    const question = view.projection.items.find(
      (item): item is QuestionItem => item.kind === "question",
    ) as QuestionItem;
    expect(question.questions[0].question).toBe("继续运行测试吗？");

    expect(view.projection.todos).toHaveLength(2);
    expect(view.projection.todos[1].status).toBe("in_progress");

    // The appended assistant text differs from the streamed attempt: it is a
    // new canonical bubble, and the streamed message stays in place.
    const assistants = view.projection.items.filter(
      (item): item is AssistantMessageItem => item.kind === "assistant_message",
    );
    expect(assistants).toHaveLength(2);
    expect(assistants[1].text).toContain("42 个行为测试全部通过");
    expect(assistants[1].finished).toBe(true);

    // session_state and metadata_changed advance the same cursor.
    expect(view.waitingQuestion).toBe(true);
    expect(view.metadata?.title).toBe("有界中继测试");
    expect(view.metadata?.pinned).toBe(true);
  });

  it("ignores events at or below the cursor (duplicate sequences)", () => {
    let state = stateWithSnapshot();
    state = applyEnvelopes(state, session1.after_snapshot);
    // The fixture tail arrives out of order (metadata 17 after events 18-20),
    // so the first pass defers 18-20 as a gap; the replay applies them.
    state = applyEnvelopes(state, session1.after_snapshot);
    const before = state.sessions[session1.session_id];
    expect(before.cursor).toBe(20);
    // A third pass is pure duplicates: nothing moves.
    state = applyEnvelopes(state, session1.after_snapshot);
    const after = state.sessions[session1.session_id];
    expect(after.cursor).toBe(before.cursor);
    expect(after.projection.items).toEqual(before.projection.items);
  });

  it("flags resync on sequence gaps instead of inventing events", () => {
    let state = stateWithSnapshot();
    // Sequence 9 skipped, apply sequence 10 directly.
    state = appReducer(state, {
      type: "server_message",
      message: asServerMessage(session1.after_snapshot[1]),
    });
    const view = state.sessions[session1.session_id];
    expect(view.resyncNeeded).toBe(true);
    expect(view.cursor).toBe(8);
  });

  it("flags resync when the stream id changes", () => {
    let state = stateWithSnapshot();
    const envelope = {
      ...session1.after_snapshot[0],
      stream_id: "webui_sample_zzzzzzzz",
    };
    state = appReducer(state, {
      type: "server_message",
      message: asServerMessage(envelope),
    });
    expect(state.sessions[session1.session_id].resyncNeeded).toBe(true);
  });

  it("detects gaps in session_state and session_metadata_changed like session_event", () => {
    let state = stateWithSnapshot(); // cursor 8
    // Gap: sequence 10 while cursor is 8.
    state = appReducer(state, {
      type: "server_message",
      message: {
        type: "session_state",
        stream_id: fixture.stream_id,
        session_id: session1.session_id,
        sequence: 10,
        event: {
          phase: "idle",
          waiting_approval: false,
          waiting_question: false,
          current_turn_id: null,
        },
      },
    });
    let view = state.sessions[session1.session_id];
    expect(view.resyncNeeded).toBe(true);
    expect(view.cursor).toBe(8);
    expect(view.phase).toBe("running"); // unchanged: the gapped state was not applied

    // Fresh snapshot resets the resync marker.
    state = appReducer(state, {
      type: "server_message",
      message: { type: "session_snapshot", snapshot: session1.snapshot },
    });
    expect(state.sessions[session1.session_id].resyncNeeded).toBe(false);

    // Metadata gap behaves the same.
    state = appReducer(state, {
      type: "server_message",
      message: {
        type: "session_metadata_changed",
        stream_id: fixture.stream_id,
        session_id: session1.session_id,
        sequence: 12,
        event: { title: "缺口", pinned: false, archived: false },
      },
    });
    view = state.sessions[session1.session_id];
    expect(view.resyncNeeded).toBe(true);
    expect(view.metadata?.title).toBe("初始标题");

    // Duplicate (<= cursor) transport states are ignored.
    state = appReducer(state, {
      type: "server_message",
      message: { type: "session_snapshot", snapshot: session1.snapshot },
    });
    state = appReducer(state, {
      type: "server_message",
      message: {
        type: "session_state",
        stream_id: fixture.stream_id,
        session_id: session1.session_id,
        sequence: 8,
        event: {
          phase: "idle",
          waiting_approval: false,
          waiting_question: false,
          current_turn_id: null,
        },
      },
    });
    view = state.sessions[session1.session_id];
    expect(view.phase).toBe("running");
    expect(view.resyncNeeded).toBe(false);
  });

  it("resumes from a replay cursor without duplicates", () => {
    const replay = session1.replay_after_cursor;
    expect(replay).toBeDefined();
    // Snapshot watermark 8, cursor 7: the server replays 8..9. Applying the
    // snapshot then the replay must produce exactly one approval card and one
    // queue position update on the same tool item.
    let state = stateWithSnapshot();
    state = applyEnvelopes(state, replay!.envelopes);
    const view = state.sessions[session1.session_id];
    expect(view.cursor).toBe(9);
    const approvals = view.projection.items.filter((item) => item.kind === "approval");
    expect(approvals).toHaveLength(1);
    const tools = view.projection.items.filter(
      (item): item is ToolItem => item.kind === "tool",
    );
    expect(tools).toHaveLength(1);
    expect(tools[0].queuePosition).toBe(1);
    expect(tools[0].status).toBe("queued");
  });
});

describe("retry retraction", () => {
  it("retracts transient attempt output on RetryResumed and keeps stable text", () => {
    let state = stateWithSnapshot(session2);
    state = applyEnvelopes(state, session2.after_snapshot.slice(0, 4));
    const view = state.sessions[session2.session_id];
    const assistants = view.projection.items.filter(
      (item): item is AssistantMessageItem => item.kind === "assistant_message",
    );
    // The "这次尝试的输出会在重试时被撤回" delta was retracted; only stable
    // text remains, and the retry status card is gone after Resume.
    expect(assistants).toHaveLength(1);
    expect(assistants[0].text).toBe("重试后的稳定正文。");
    expect(
      view.projection.items.filter((item) => item.kind === "retry"),
    ).toHaveLength(0);
  });

  it("shows the retry status while waiting", () => {
    let state = stateWithSnapshot(session2);
    state = applyEnvelopes(state, session2.after_snapshot.slice(0, 1));
    const retry = state.sessions[session2.session_id].projection.items.find(
      (item): item is RetryItem => item.kind === "retry",
    );
    expect(retry?.phase).toBe("waiting");
    expect(retry?.errorCode).toBe("provider.rate_limit");
    // The failed attempt's streamed message was retracted at the boundary.
    const assistants = state.sessions[session2.session_id].projection.items.filter(
      (item) => item.kind === "assistant_message",
    );
    expect(assistants).toHaveLength(0);
  });

  it("rebuild under a different stream id leaves no failed-attempt residue", () => {
    let state = stateWithSnapshot(session2);
    state = applyEnvelopes(state, session2.after_snapshot);
    // Service restart: replacement snapshot with a new stream id.
    const replacement = fixture.snapshot_replacement;
    state = appReducer(state, {
      type: "server_message",
      message: { type: "session_snapshot", snapshot: replacement.snapshot },
    });
    const view = state.sessions[session2.session_id];
    expect(view.streamId).toBe("webui_sample_bbbbbbbb");
    expect(view.cursor).toBe(4);
    const assistants = view.projection.items.filter(
      (item): item is AssistantMessageItem => item.kind === "assistant_message",
    );
    expect(assistants).toHaveLength(1);
    expect(assistants[0].text).toBe("另一个会话在并行跑格式化。重试后的稳定正文。");
    expect(view.projection.items.some((item) => item.kind === "retry")).toBe(false);
    expect(view.projection.items.some((item) => item.kind === "swarm")).toBe(false);

    // Later events on the new stream resume at watermark + 1.
    state = applyEnvelopes(state, replacement.after_snapshot);
    const after = state.sessions[session2.session_id];
    expect(after.cursor).toBe(12);
    expect(after.phase).toBe("idle");
    expect(after.currentTurnId).toBeNull();
    const delegate = after.projection.items.find(
      (item): item is DelegateItem => item.kind === "delegate",
    ) as DelegateItem;
    expect(delegate.agent.state).toBe("completed");
    expect(delegate.agent.terminal_reason).toBe("completed");
    const workflow = after.projection.items.find(
      (item): item is WorkflowItem => item.kind === "workflow",
    ) as WorkflowItem;
    expect(workflow.workflow.state).toBe("completed");
    const terminal = after.projection.items.find(
      (item): item is TerminalItem => item.kind === "terminal",
    ) as TerminalItem;
    expect(terminal.output).toContain("42 passed");
    expect(terminal.outputRef?.complete).toBe(true);
  });
});

describe("delegate swarm", () => {
  it("keeps one swarm item updated in place with final aggregate counts", () => {
    let state = stateWithSnapshot(session2);
    state = applyEnvelopes(state, session2.after_snapshot);
    const view = state.sessions[session2.session_id];
    const swarms = view.projection.items.filter(
      (item): item is SwarmItem => item.kind === "swarm",
    );
    expect(swarms).toHaveLength(1);
    const swarm = swarms[0];
    expect(swarm.swarm.swarm_id).toBe("swarm_02");
    expect(swarm.swarm.state).toBe("completed");
    expect(swarm.swarm.aggregate.completed).toBe(2);
    expect(swarm.swarm.children).toHaveLength(2);
    expect(swarm.swarm.children[1].agent.state).toBe("completed");
    // The progress payloads merged into the same item: nothing fell through
    // to the unknown-event record.
    expect(view.projection.items.some((item) => item.kind === "unknown")).toBe(false);
  });
});

describe("delegate progress events", () => {
  it("merges DelegateProgressUpdated into the existing delegate item", () => {
    let projection = emptyProjection();
    projection = applyAgentEvent(projection, {
      DelegateStarted: {
        turn: 1,
        agent: { id: "agent_1", display_name: "explorer", state: "running", task_title: "检查覆盖" },
      },
    });
    projection = applyAgentEvent(projection, {
      DelegateProgressUpdated: {
        turn: 1,
        progress: {
          agent_id: "agent_1",
          state: "running",
          tool_count: 2,
          token_count: 30,
          elapsed_ms: 4500,
          latest_text: "进展中",
        },
      },
    });
    const delegates = projection.items.filter(
      (item): item is DelegateItem => item.kind === "delegate",
    );
    expect(delegates).toHaveLength(1);
    // Identity from the full snapshot survives; progress fields win.
    expect(delegates[0].agent.display_name).toBe("explorer");
    expect(delegates[0].agent.tool_count).toBe(2);
    expect(delegates[0].agent.latest_text).toBe("进展中");
    expect(delegates[0].agent.elapsed?.secs).toBe(4);
    expect(delegates[0].finished).toBe(false);
    expect(projection.items.some((item) => item.kind === "unknown")).toBe(false);
  });

  it("merges DelegateSwarmProgressUpdated into the existing swarm item", () => {
    let projection = emptyProjection();
    projection = applyAgentEvent(projection, {
      DelegateSwarmStarted: {
        turn: 1,
        swarm: {
          swarm_id: "swarm_1",
          description: "并行检查",
          state: "running",
          max_concurrency: 2,
          aggregate: { total: 2, queued: 1, running: 1, completed: 0, failed: 0, cancelled: 0, timed_out: 0 },
          children: [
            { item_index: 0, item: "甲", agent: { id: "a1", display_name: "a1", state: "running" } },
            { item_index: 1, item: "乙", agent: { id: "a2", display_name: "a2", state: "queued" } },
          ],
        },
      },
    });
    projection = applyAgentEvent(projection, {
      DelegateSwarmProgressUpdated: {
        turn: 1,
        swarm_id: "swarm_1",
        state: "running",
        aggregate: { total: 2, queued: 0, running: 1, completed: 1, failed: 0, cancelled: 0, timed_out: 0 },
        child_progress: {
          item_index: 0,
          progress: { agent_id: "a1", state: "completed", elapsed_ms: 7000, latest_text: "完成" },
        },
      },
    });
    const swarms = projection.items.filter((item): item is SwarmItem => item.kind === "swarm");
    expect(swarms).toHaveLength(1);
    expect(swarms[0].swarm.aggregate.completed).toBe(1);
    expect(swarms[0].swarm.children[0].agent.state).toBe("completed");
    expect(swarms[0].swarm.children[0].agent.latest_text).toBe("完成");
    expect(swarms[0].swarm.children[1].agent.state).toBe("queued");
    expect(swarms[0].finished).toBe(false);
  });

  it("keeps fixture swarm progress updates inside the single swarm item", () => {
    let state = stateWithSnapshot(session2);
    state = applyEnvelopes(state, session2.after_snapshot.slice(0, 7)); // through seq 10
    const view = state.sessions[session2.session_id];
    const swarms = view.projection.items.filter(
      (item): item is SwarmItem => item.kind === "swarm",
    );
    expect(swarms).toHaveLength(1);
    expect(swarms[0].swarm.children[0].agent.state).toBe("completed");
    expect(view.projection.items.some((item) => item.kind === "unknown")).toBe(false);
  });
});

describe("usage and context projections", () => {
  it("projects TokenUsage and ContextWindowUpdated latest-wins, never as rows", () => {
    let projection = emptyProjection();
    projection = applyAgentEvent(projection, {
      TokenUsage: { turn: 1, usage: { input_tokens: 10, output_tokens: 2 } },
    });
    projection = applyAgentEvent(projection, {
      TokenUsage: { turn: 1, usage: { input_tokens: 20, output_tokens: 3 } },
    });
    projection = applyAgentEvent(projection, {
      ContextWindowUpdated: {
        turn: 1,
        used_tokens: 23,
        max_tokens: 200000,
        remaining_tokens: 199977,
      },
    });
    expect(projection.latestUsage?.input_tokens).toBe(20);
    expect(projection.latestUsage?.output_tokens).toBe(3);
    expect(projection.contextWindow?.used_tokens).toBe(23);
    expect(projection.contextWindow?.max_tokens).toBe(200000);
    expect(projection.items).toHaveLength(0);
  });

  it("seeds usage and context window from the snapshot session state", () => {
    const state = stateWithSnapshot();
    const view = state.sessions[session1.session_id];
    expect(view.projection.latestUsage?.input_tokens).toBe(12480);
    expect(view.projection.contextWindow?.used_tokens).toBe(12792);
    expect(view.projection.contextWindow?.max_tokens).toBe(200000);
  });

  it("applies fresher usage fields carried by session_state messages", () => {
    let state = stateWithSnapshot();
    // seq 9..16: the session_state at 16 carries no usage → snapshot values survive.
    state = applyEnvelopes(state, session1.after_snapshot.slice(0, 8));
    let view = state.sessions[session1.session_id];
    expect(view.projection.latestUsage?.input_tokens).toBe(12480);
    state = appReducer(state, {
      type: "server_message",
      message: {
        type: "session_state",
        stream_id: fixture.stream_id,
        session_id: session1.session_id,
        sequence: 17,
        event: {
          phase: "running",
          waiting_approval: false,
          waiting_question: true,
          current_turn_id: "turn_01",
          token_usage: { input_tokens: 13000, output_tokens: 400 },
          context_window: { used_tokens: 13400, max_tokens: 200000 },
        },
      },
    });
    view = state.sessions[session1.session_id];
    expect(view.projection.latestUsage?.input_tokens).toBe(13000);
    expect(view.projection.contextWindow?.used_tokens).toBe(13400);
  });
});

describe("grouped workspace snapshot", () => {
  it("parses the grouped snapshot, derives the flat list and syncs summary changes", () => {
    let state = initialAppState(280, "dark");
    state = appReducer(state, {
      type: "server_message",
      message: asServerMessage(fixture.long_connection.workspace_snapshot),
    });
    expect(state.workspaces).toHaveLength(2);
    expect(state.workspaces[0].label).toBe("neo");
    expect(state.workspaces[0].current).toBe(true);
    expect(state.workspaces[1].label).toBe("playground");
    expect(state.summaries.map((summary) => summary.session_id)).toEqual([
      "session_0001",
      "session_0002",
      "session_0003",
    ]);

    state = appReducer(state, {
      type: "server_message",
      message: asServerMessage(fixture.long_connection.session_summary_changed),
    });
    expect(
      state.summaries.find((summary) => summary.session_id === "session_0002")?.state,
    ).toBe("idle");
    expect(
      state.workspaces[0].sessions.find((summary) => summary.session_id === "session_0002")
        ?.state,
    ).toBe("idle");
  });
});

describe("append coverage and unknown tags", () => {
  it("does not turn injected messages into user turns and keeps real user messages", () => {
    let projection = emptyProjection();
    projection = applyAgentEvent(projection, { TextDelta: { turn: 1, text: "稳定的正文。" } });
    projection = applyAgentEvent(projection, {
      MessageAppended: {
        message: {
          User: {
            content: [{ Text: { text: "<system-reminder>忽略这段内容</system-reminder>" } }],
            origin: { kind: "injection", variant: "system_reminder" },
          },
        },
      },
    });
    expect(projection.items.some((item) => item.kind === "user_message")).toBe(false);
    expect(projection.coverage.text).toBe("稳定的正文。");

    projection = applyAgentEvent(projection, {
      MessageAppended: {
        message: {
          Assistant: {
            content: [{ Text: { text: "稳定的正文。" } }],
            tool_calls: [],
            stop_reason: "EndTurn",
          },
        },
      },
    });
    expect(projection.items.filter((item) => item.kind === "assistant_message")).toHaveLength(1);

    projection = applyAgentEvent(projection, {
      MessageAppended: {
        message: { User: { content: [{ Text: { text: "继续处理。" } }] } },
      },
    });
    expect(projection.items.filter((item) => item.kind === "user_message")).toHaveLength(1);
  });

  it("keeps repeated thinking ids in separate blocks", () => {
    let projection = emptyProjection();
    const start = (turn: number) => {
      projection = applyAgentEvent(projection, { ThinkingStarted: { turn, id: "reasoning" } });
    };
    const delta = (turn: number, text: string) => {
      projection = applyAgentEvent(projection, { ThinkingDelta: { turn, text } });
    };
    const finish = (turn: number) => {
      projection = applyAgentEvent(projection, {
        ThinkingFinished: { turn, redacted: false },
      });
    };

    start(1);
    delta(1, "第一段思考");
    finish(1);
    start(1);
    delta(1, "第二段思考");
    finish(1);
    start(2);
    delta(2, "第三段思考");
    finish(2);

    const thinking = projection.items.filter((item) => item.kind === "thinking");
    expect(thinking.map((item) => item.id)).toEqual([
      "think:1:reasoning:1",
      "think:1:reasoning:2",
      "think:2:reasoning:1",
    ]);
    expect(thinking.map((item) => item.text)).toEqual([
      "第一段思考",
      "第二段思考",
      "第三段思考",
    ]);
  });

  it("finishes open thinking for every terminal turn reason", () => {
    for (const stopReason of ["EndTurn", "ToolUse", "MaxTokens", "Cancelled", "Error"] as const) {
      let projection = emptyProjection();
      projection = applyAgentEvent(projection, {
        ThinkingStarted: { turn: 1, id: "first" },
      });
      projection = applyAgentEvent(projection, {
        ThinkingDelta: { turn: 1, text: "第一段未闭合思考" },
      });
      projection = applyAgentEvent(projection, {
        ThinkingStarted: { turn: 1, id: "second" },
      });
      projection = applyAgentEvent(projection, {
        ThinkingDelta: { turn: 1, text: "第二段未闭合思考" },
      });
      if (stopReason === "Error") {
        projection = applyAgentEvent(projection, {
          Error: { turn: 1, message: "模型请求失败" },
        });
      }
      projection = applyAgentEvent(projection, {
        TurnFinished: { turn: 1, stop_reason: stopReason },
      });

      const thinking = projection.items.filter(
        (item): item is ThinkingItem => item.kind === "thinking",
      );
      expect({ stopReason, finished: thinking.map((item) => item.finished) }).toEqual({
        stopReason,
        finished: [true, true],
      });
      expect(projection.liveThinkingId).toBeNull();
    }
  });

  it("folds late Delegate and WaitDelegate items back into their assistant turn", () => {
    let projection = emptyProjection();
    projection = applyAgentEvent(projection, {
      MessageAppended: { message: { User: { content: [{ Text: { text: "处理任务。" } }] } } },
    });
    projection = applyAgentEvent(projection, {
      TurnStarted: { turn: 1 },
    });
    projection = applyAgentEvent(projection, {
      MessageAppended: {
        message: {
          Assistant: {
            content: [{ Text: { text: "处理完成。" } }],
            tool_calls: [],
            stop_reason: "EndTurn",
          },
        },
      },
    });
    projection = applyAgentEvent(projection, {
      MessageAppended: { message: { User: { content: [{ Text: { text: "继续处理。" } }] } } },
    });
    projection = applyAgentEvent(projection, {
      ToolExecutionFinished: {
        turn: 1,
        id: "wait_1",
        name: "WaitDelegate",
        result: { content: "已等待", is_error: false },
      },
    });
    projection = applyAgentEvent(projection, {
      DelegateFinished: {
        turn: 1,
        agent: { id: "agent_1", display_name: "分析", state: "completed" },
      },
    });
    projection = applyAgentEvent(projection, { TurnStarted: { turn: 2 } });
    projection = applyAgentEvent(projection, {
      MessageAppended: {
        message: {
          Assistant: {
            content: [{ Text: { text: "下一回合完成。" } }],
            tool_calls: [],
            stop_reason: "EndTurn",
          },
        },
      },
    });

    const groups = groupTurns(projection.items);
    const assist = groups.find(
      (group) => group.kind === "assist" && group.msg.text === "处理完成。",
    );
    expect(assist?.kind).toBe("assist");
    if (!assist || assist.kind !== "assist") return;
    expect(assist.process.map((item) => item.id)).toEqual(["tool:wait_1", "delegate:agent_1"]);
    const nextAssist = groups.find(
      (group) => group.kind === "assist" && group.msg.text === "下一回合完成。",
    );
    expect(nextAssist?.kind).toBe("assist");
    if (nextAssist?.kind === "assist") expect(nextAssist.process).toEqual([]);
    expect(groups.filter((group) => group.kind === "user")).toHaveLength(2);
    expect(groups.some((group) => group.kind === "process")).toBe(false);
  });

  it("skips an appended assistant message exactly covered by the stream", () => {
    let projection = emptyProjection();
    projection = applyAgentEvent(projection, {
      MessageStarted: { turn: 1, id: "m1" },
    });
    projection = applyAgentEvent(projection, {
      TextDelta: { turn: 1, text: "稳定的正文。" },
    });
    projection = applyAgentEvent(projection, {
      MessageFinished: { turn: 1, id: "m1", stop_reason: "EndTurn" },
    });
    projection = applyAgentEvent(projection, {
      MessageAppended: {
        message: {
          Assistant: {
            content: [{ Text: { text: "稳定的正文。" } }],
            tool_calls: [],
            stop_reason: "EndTurn",
          },
        },
      },
    });
    const assistants = projection.items.filter((item) => item.kind === "assistant_message");
    expect(assistants).toHaveLength(1);
  });

  it("keeps unknown tags as collapsible raw records and keeps applying events", () => {
    let projection = emptyProjection();
    const unknown = { BrandNewEvent: { turn: 1, payload: "x" } } as unknown as AgentEvent;
    projection = applyAgentEvent(projection, unknown);
    projection = applyAgentEvent(projection, {
      TextDelta: { turn: 1, text: "后续事件仍被应用" },
    });
    expect(agentEventTag(unknown)).toBe("BrandNewEvent");
    const unknownItem = projection.items.find(
      (item): item is UnknownItem => item.kind === "unknown",
    );
    expect(unknownItem?.tag).toBe("BrandNewEvent");
    expect(unknownItem?.raw).toContain("payload");
    const assistant = projection.items.find(
      (item): item is AssistantMessageItem => item.kind === "assistant_message",
    );
    expect(assistant?.text).toBe("后续事件仍被应用");
  });

  it("does not turn deltas into standalone bubbles", () => {
    let projection = emptyProjection();
    projection = applyAgentEvent(projection, { TextDelta: { turn: 1, text: "a" } });
    projection = applyAgentEvent(projection, { TextDelta: { turn: 1, text: "b" } });
    const assistants = projection.items.filter(
      (item): item is AssistantMessageItem => item.kind === "assistant_message",
    );
    expect(assistants).toHaveLength(1);
    expect(assistants[0].text).toBe("ab");
  });
});

describe("buildFromHistory", () => {
  it("applies history entries with their envelope output references", () => {
    const projection = buildFromHistory(
      [
        {
          sequence: 1,
          event: {
            ToolExecutionStarted: { turn: 1, id: "t1", name: "bash", arguments: {} },
          },
          output: { id: "opaque", byte_len: 10, line_count: 2, complete: true },
        },
      ],
      [],
    );
    const tool = projection.items[0] as ToolItem;
    expect(tool.output?.id).toBe("opaque");
  });
});
