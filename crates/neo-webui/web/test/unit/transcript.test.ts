/**
 * Transcript projection tests driven by the fixed sample: snapshot rebuild,
 * watermark resume, duplicate dedup, retry retraction, in-place updates by
 * stable id, unknown tags and append coverage.
 */

import { describe, expect, it } from "vitest";
import { agentEventTag, type AgentEvent } from "../../src/protocol";
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
  type ToolItem,
  type UnknownItem,
  type UserMessageItem,
  type WorkflowItem,
} from "../../src/state/transcript";
import { asServerMessage, loadFixture } from "./fixture";
import { appReducer } from "../../src/state/reducer";
import { initialAppState, type AppState } from "../../src/state/appState";

const fixture = loadFixture();
const session1 = fixture.sessions[0];
const session2 = fixture.sessions[1];

function stateWithSnapshot(session = session1): AppState {
  let state = initialAppState(280);
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
    state = initialAppState(280);
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
    delete snapshot.pending_question;
    let state = initialAppState(280);
    state = appReducer(state, { type: "select_session", sessionId: session1.session_id });
    state = appReducer(state, {
      type: "server_message",
      message: { type: "session_snapshot", snapshot },
    });
    const view = state.sessions[session1.session_id];
    expect(view.projection.pendingQuestionId).toBeNull();
    const question = view.projection.items.find(
      (item): item is QuestionItem => item.kind === "question",
    ) as QuestionItem;
    expect(question.resolved).toBe(true);
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
    const before = state.sessions[session1.session_id];
    // Replay the same envelopes: every sequence is <= cursor.
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
    expect(after.cursor).toBe(11);
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
  });
});

describe("append coverage and unknown tags", () => {
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
