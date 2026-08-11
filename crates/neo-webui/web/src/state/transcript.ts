/**
 * Transcript projection: display state rebuilt from snapshots and events.
 * It is a projection, never a second canonical record. Every update is
 * driven by an explicit event tag and stable identifier — never by text,
 * name patterns or regular expressions.
 */

import type {
  AgentEvent,
  AgentMessage,
  AgentMessageUser,
  AgentProgressSnapshot,
  AgentSnapshot,
  AgentTokenUsage,
  ApprovalRequest,
  ApprovalResolution,
  QuestionEventData,
  StopReason,
  SwarmAggregate,
  SwarmSnapshot,
  TodoEventData,
  ToolResult,
  WebUiContextWindow,
  WebUiHistoryEntry,
  WebUiOutputRef,
  WorkflowSnapshot,
} from "../protocol";
import { agentEventTag } from "../protocol";

// ---------------------------------------------------------------------------
// Display items
// ---------------------------------------------------------------------------

export type ToolStatus = "queued" | "running" | "finished" | "failed";

interface TurnScopedItem {
  turn?: number;
}

export interface UserMessageItem {
  kind: "user_message";
  id: string;
  text: string;
}

export interface AssistantMessageItem extends TurnScopedItem {
  kind: "assistant_message";
  id: string;
  text: string;
  finished: boolean;
  stopReason?: StopReason;
}

export interface ThinkingItem extends TurnScopedItem {
  kind: "thinking";
  id: string;
  text: string;
  finished: boolean;
  redacted: boolean;
}

export interface ToolItem extends TurnScopedItem {
  kind: "tool";
  id: string;
  name: string;
  arguments: unknown;
  status: ToolStatus;
  queuePosition?: number;
  queueWaitingMs?: number;
  partialResult?: ToolResult;
  result?: ToolResult;
  output?: WebUiOutputRef;
}

export interface ShellItem extends TurnScopedItem {
  kind: "shell";
  id: string;
  command: string;
  cwd: string;
  status: ToolStatus;
  queuePosition?: number;
  queueWaitingMs?: number;
  stdout: string;
  stderr: string;
  truncated: boolean;
  exitCode?: number | null;
  output?: WebUiOutputRef;
}

export interface TerminalItem extends TurnScopedItem {
  kind: "terminal";
  id: string;
  handle: string;
  command?: string;
  cwd?: string;
  output: string;
  truncated: boolean;
  finished: boolean;
  statusText?: string;
  exitCode?: number | null;
  outputRef?: WebUiOutputRef;
}

export interface WorkflowItem extends TurnScopedItem {
  kind: "workflow";
  id: string;
  workflow: WorkflowSnapshot;
  finished: boolean;
}

export interface ApprovalItem {
  kind: "approval";
  id: string;
  request: ApprovalRequest;
  resolution?: ApprovalResolution;
}

export interface QuestionItem {
  kind: "question";
  id: string;
  turn: number;
  questions: QuestionEventData[];
  resolved: boolean;
}

export interface DelegateItem extends TurnScopedItem {
  kind: "delegate";
  id: string;
  agent: AgentSnapshot;
  finished: boolean;
}

export interface SwarmItem extends TurnScopedItem {
  kind: "swarm";
  id: string;
  swarm: SwarmSnapshot;
  finished: boolean;
}

export interface RetryItem {
  kind: "retry";
  id: string;
  turn: number;
  retry: number;
  maxRetries: number;
  delayMs: number;
  errorCode: string;
  message: string;
  phase: "waiting" | "connecting" | "exhausted";
}

export interface StatusLineItem {
  kind: "status";
  id: string;
  severity: "error" | "info";
  text: string;
}

/** Unknown future event tags: preserved verbatim as collapsible safe JSON. */
export interface UnknownItem {
  kind: "unknown";
  id: string;
  tag: string;
  raw: string;
}

export type TranscriptItem =
  | UserMessageItem
  | AssistantMessageItem
  | ThinkingItem
  | ToolItem
  | ShellItem
  | TerminalItem
  | WorkflowItem
  | ApprovalItem
  | QuestionItem
  | DelegateItem
  | SwarmItem
  | RetryItem
  | StatusLineItem
  | UnknownItem;

// ---------------------------------------------------------------------------
// Projection with internal bookkeeping
// ---------------------------------------------------------------------------

interface AttemptCoverage {
  /** Accumulated streamed text since the last canonical append. */
  text: string;
  thinking: string;
  thinkingRedacted: boolean;
  hasDetail: boolean;
  toolIds: Set<string>;
}

export interface TranscriptProjection {
  items: TranscriptItem[];
  todos: TodoEventData[];
  /** Current pending approval request id (mirrors ApprovalRequested until
   * ApprovalResolved or the next snapshot). */
  pendingApprovalId: string | null;
  pendingQuestionIds: string[];
  /** Latest TokenUsage payload on the stream (latest-wins). */
  latestUsage: AgentTokenUsage | null;
  /** Latest ContextWindowUpdated payload on the stream (latest-wins). */
  contextWindow: WebUiContextWindow | null;
  // Internal, never rendered directly:
  /** Last explicit event turn, used because MessageAppended has no turn field. */
  latestTurn: number | null;
  liveMessageId: string | null;
  liveThinkingId: string | null;
  /** Per-(turn, provider id) occurrence counts keep separate thinking
   * blocks independently expandable when a provider reuses an id. */
  thinkingOccurrenceByKey: Record<string, number>;
  coverage: AttemptCoverage;
  completedToolResults: string[];
  appendedCounter: number;
}

export function emptyProjection(): TranscriptProjection {
  return {
    items: [],
    todos: [],
    pendingApprovalId: null,
    pendingQuestionIds: [],
    latestUsage: null,
    contextWindow: null,
    latestTurn: null,
    liveMessageId: null,
    liveThinkingId: null,
    thinkingOccurrenceByKey: {},
    coverage: {
      text: "",
      thinking: "",
      thinkingRedacted: false,
      hasDetail: false,
      toolIds: new Set<string>(),
    },
    completedToolResults: [],
    appendedCounter: 0,
  };
}

function resetCoverage(): AttemptCoverage {
  return {
    text: "",
    thinking: "",
    thinkingRedacted: false,
    hasDetail: false,
    toolIds: new Set<string>(),
  };
}

function replaceItem(
  items: TranscriptItem[],
  id: string,
  update: (item: TranscriptItem) => TranscriptItem,
): TranscriptItem[] {
  const index = items.findIndex((item) => item.id === id);
  if (index < 0) return items;
  const next = items.slice();
  next[index] = update(items[index]);
  return next;
}

/** Remove the current unfinished live model attempt (streamed message and
 * thinking blocks that never reached a finished boundary). Finished entries
 * stay. Driven only by the explicit Retry* attempt boundaries. */
function resetLiveAttempt(projection: TranscriptProjection): TranscriptProjection {
  const removable = new Set<string>();
  for (const item of projection.items) {
    if (item.kind === "assistant_message" && !item.finished) {
      removable.add(item.id);
    }
    if (item.kind === "thinking" && !item.finished) {
      removable.add(item.id);
    }
  }
  if (removable.size === 0 && projection.liveMessageId === null) {
    return {
      ...projection,
      liveMessageId: null,
      liveThinkingId: null,
      coverage: resetCoverage(),
    };
  }
  return {
    ...projection,
    items: projection.items.filter((item) => !removable.has(item.id)),
    liveMessageId: null,
    liveThinkingId: null,
    coverage: resetCoverage(),
  };
}

function messageText(message: AgentMessage): {
  text: string;
  thinking: string;
  thinkingRedacted: boolean;
} {
  let text = "";
  let thinking = "";
  let thinkingRedacted = false;
  const content =
    "User" in (message as Record<string, unknown>)
      ? (message as { User: { content: unknown[] } }).User.content
      : "Assistant" in (message as Record<string, unknown>)
        ? (message as { Assistant: { content: unknown[] } }).Assistant.content
        : [];
  for (const part of content) {
    const record = part as Record<string, Record<string, unknown>>;
    if (record && typeof record === "object") {
      if (record.Text && typeof record.Text.text === "string") {
        text += record.Text.text;
      }
      if (record.Thinking) {
        if (typeof record.Thinking.text === "string") {
          thinking += record.Thinking.text;
        }
        if (record.Thinking.redacted === true) {
          thinkingRedacted = true;
        }
      }
    }
  }
  return { text, thinking, thinkingRedacted };
}

function isInjectedUserMessage(message: AgentMessage): boolean {
  const user = (message as Partial<AgentMessageUser>).User;
  return user?.origin?.kind === "injection";
}

function userDisplayText(message: AgentMessage): string {
  const user = (message as Partial<AgentMessageUser>).User;
  if (!user) return "";
  if (typeof user.display_text === "string" && user.display_text.trim() !== "") {
    return user.display_text;
  }
  return messageText(message).text;
}

function nextAppendedId(projection: TranscriptProjection, prefix: string): [string, number] {
  const n = projection.appendedCounter + 1;
  return [`${prefix}:${n}`, n];
}

const KNOWN_SILENT_TAGS = new Set([
  "RunStarted",
  "TurnStarted",
  "RunFinished",
  "SteeringQueued",
  "FollowUpQueued",
  "QueueDrained",
  "CompactionStarted",
  "CompactionProgress",
  "CompactionApplied",
  "InstructionEpoch",
  "SkillInvocation",
  "ToolCallStarted",
  "ToolCallArgumentsDelta",
  "ToolCallFinished",
  "GoalStarted",
  "GoalPaused",
  "GoalResumed",
  "GoalBlocked",
  "GoalFinished",
  "PlanModeEntered",
]);

/** Apply one canonical AgentEvent (plus its optional opaque output
 * reference) to the projection. Pure: returns a new projection. */
export function applyAgentEvent(
  projection: TranscriptProjection,
  event: AgentEvent,
  output?: WebUiOutputRef | null,
): TranscriptProjection {
  const tag = agentEventTag(event);
  const body = (event as Record<string, Record<string, unknown>>)[tag] ?? {};
  const eventTurn = typeof body.turn === "number" ? body.turn : null;
  if (eventTurn !== null) {
    projection = { ...projection, latestTurn: eventTurn };
  }

  switch (tag) {
    // -- Retry series: explicit attempt boundaries retract transient output.
    case "RetryScheduled": {
      const b = body as {
        turn: number;
        retry: number;
        max_retries: number;
        delay_ms: number;
        error_code: string;
        message: string;
      };
      const cleared = resetLiveAttempt(projection);
      const item: RetryItem = {
        kind: "retry",
        id: `retry:${b.turn}`,
        turn: b.turn,
        retry: b.retry,
        maxRetries: b.max_retries,
        delayMs: b.delay_ms,
        errorCode: b.error_code,
        message: b.message,
        phase: "waiting",
      };
      return upsertRetry(cleared, item);
    }
    case "RetryStarted": {
      const b = body as { turn: number; retry: number; max_retries: number };
      const item: RetryItem = {
        kind: "retry",
        id: `retry:${b.turn}`,
        turn: b.turn,
        retry: b.retry,
        maxRetries: b.max_retries,
        delayMs: 0,
        errorCode: "",
        message: "",
        phase: "connecting",
      };
      return upsertRetry(projection, item);
    }
    case "RetryResumed":
    case "RetrySucceeded": {
      const b = body as { turn: number };
      const cleared = tag === "RetryResumed" ? resetLiveAttempt(projection) : projection;
      return {
        ...cleared,
        items: cleared.items.filter((item) => item.id !== `retry:${b.turn}`),
      };
    }
    case "RetryExhausted": {
      const b = body as {
        turn: number;
        retries_used: number;
        error_code: string;
        message: string;
      };
      const cleared = resetLiveAttempt(projection);
      const item: RetryItem = {
        kind: "retry",
        id: `retry:${b.turn}`,
        turn: b.turn,
        retry: b.retries_used,
        maxRetries: b.retries_used,
        delayMs: 0,
        errorCode: b.error_code,
        message: b.message,
        phase: "exhausted",
      };
      return upsertRetry(cleared, item);
    }

    // -- Assistant message streaming: in-place updates by stable id.
    case "MessageStarted": {
      const b = body as { turn: number; id: string };
      const coverage = { ...projection.coverage, hasDetail: true };
      const item: AssistantMessageItem = {
        kind: "assistant_message",
        id: `msg:${b.id}`,
        text: "",
        finished: false,
        turn: b.turn,
      };
      return {
        ...projection,
        items: [...projection.items, item],
        liveMessageId: item.id,
        coverage,
      };
    }
    case "TextDelta": {
      const b = body as { turn: number; text: string };
      const coverage = {
        ...projection.coverage,
        hasDetail: true,
        text: projection.coverage.text + b.text,
      };
      let liveMessageId = projection.liveMessageId;
      let items = projection.items;
      if (liveMessageId === null || !items.some((i) => i.id === liveMessageId)) {
        const [id, counter] = nextAppendedId(projection, "msg:live");
        const item: AssistantMessageItem = {
          kind: "assistant_message",
          id,
          text: "",
          finished: false,
          turn: b.turn,
        };
        items = [...items, item];
        liveMessageId = id;
        return {
          ...projection,
          items: replaceItem(items, id, (item) =>
            item.kind === "assistant_message"
              ? { ...item, text: item.text + b.text }
              : item,
          ),
          liveMessageId,
          coverage,
          appendedCounter: counter,
        };
      }
      return {
        ...projection,
        items: replaceItem(items, liveMessageId, (item) =>
          item.kind === "assistant_message"
            ? { ...item, text: item.text + b.text }
            : item,
        ),
        coverage,
      };
    }
    case "MessageFinished": {
      const b = body as { id: string; stop_reason: StopReason };
      const id = `msg:${b.id}`;
      if (!projection.items.some((item) => item.id === id)) return projection;
      return {
        ...projection,
        items: replaceItem(projection.items, id, (item) =>
          item.kind === "assistant_message"
            ? { ...item, finished: true, stopReason: b.stop_reason }
            : item,
        ),
      };
    }

    // -- Thinking blocks: in-place, collapsed by default.
    case "ThinkingStarted": {
      const b = body as { turn: number; id: string };
      const coverage = { ...projection.coverage, hasDetail: true };
      const occurrenceKey = `${b.turn}\u0000${b.id}`;
      const occurrence = (projection.thinkingOccurrenceByKey[occurrenceKey] ?? 0) + 1;
      const item: ThinkingItem = {
        kind: "thinking",
        id: `think:${b.turn}:${b.id}:${occurrence}`,
        text: "",
        finished: false,
        redacted: false,
        turn: b.turn,
      };
      return {
        ...projection,
        items: [...projection.items, item],
        liveThinkingId: item.id,
        thinkingOccurrenceByKey: {
          ...projection.thinkingOccurrenceByKey,
          [occurrenceKey]: occurrence,
        },
        coverage,
      };
    }
    case "ThinkingDelta": {
      const b = body as { turn: number; text: string };
      const coverage = {
        ...projection.coverage,
        hasDetail: true,
        thinking: projection.coverage.thinking + b.text,
      };
      let liveThinkingId = projection.liveThinkingId;
      let items = projection.items;
      if (liveThinkingId === null || !items.some((i) => i.id === liveThinkingId)) {
        const [id, counter] = nextAppendedId(projection, "think:live");
        items = [
          ...items,
          {
            kind: "thinking",
            id,
            text: "",
            finished: false,
            redacted: false,
            turn: b.turn,
          } satisfies ThinkingItem,
        ];
        liveThinkingId = id;
        return {
          ...projection,
          items: replaceItem(items, id, (item) =>
            item.kind === "thinking" ? { ...item, text: item.text + b.text } : item,
          ),
          liveThinkingId,
          coverage,
          appendedCounter: counter,
        };
      }
      return {
        ...projection,
        items: replaceItem(items, liveThinkingId, (item) =>
          item.kind === "thinking" ? { ...item, text: item.text + b.text } : item,
        ),
        coverage,
      };
    }
    case "ThinkingFinished": {
      const b = body as { redacted: boolean };
      const id = projection.liveThinkingId;
      const coverage = {
        ...projection.coverage,
        hasDetail: true,
        thinkingRedacted: projection.coverage.thinkingRedacted || b.redacted,
      };
      if (id === null) return { ...projection, coverage };
      return {
        ...projection,
        items: replaceItem(projection.items, id, (item) =>
          item.kind === "thinking"
            ? { ...item, finished: true, redacted: item.redacted || b.redacted }
            : item,
        ),
        liveThinkingId: null,
        coverage,
      };
    }
    case "TurnFinished": {
      const b = body as { turn: number; stop_reason: StopReason };
      const items = projection.items.map((item) =>
        item.kind === "thinking" && item.turn === b.turn && !item.finished
          ? { ...item, finished: true }
          : item,
      );
      const live = projection.liveThinkingId === null
        ? null
        : items.find((item) => item.id === projection.liveThinkingId);
      return {
        ...projection,
        items,
        liveThinkingId:
          live === null ||
          live === undefined ||
          (live.kind === "thinking" && live.turn === b.turn)
            ? null
            : projection.liveThinkingId,
      };
    }

    // -- Tool lifecycle.
    case "ToolExecutionQueued": {
      const b = body as { turn: number; id: string; name: string; arguments: unknown };
      const toolIds = new Set(projection.coverage.toolIds);
      toolIds.add(b.id);
      const item: ToolItem = {
        kind: "tool",
        id: `tool:${b.id}`,
        name: b.name,
        arguments: b.arguments,
        status: "queued",
        turn: b.turn,
      };
      return {
        ...projection,
        items: upsertById(projection.items, item),
        coverage: { ...projection.coverage, toolIds },
      };
    }
    case "ToolExecutionQueueUpdated": {
      const b = body as { id: string; position: number; waiting_ms: number };
      const id = `tool:${b.id}`;
      return {
        ...projection,
        items: replaceItem(projection.items, id, (item) =>
          item.kind === "tool"
            ? { ...item, status: "queued", queuePosition: b.position, queueWaitingMs: b.waiting_ms }
            : item,
        ),
      };
    }
    case "ToolExecutionStarted": {
      const b = body as { turn: number; id: string; name: string; arguments: unknown };
      const toolIds = new Set(projection.coverage.toolIds);
      toolIds.add(b.id);
      const id = `tool:${b.id}`;
      const item: ToolItem = {
        kind: "tool",
        id,
        name: b.name,
        arguments: b.arguments,
        status: "running",
        turn: b.turn,
        ...(output ? { output } : {}),
      };
      return {
        ...projection,
        items: upsertById(projection.items, item),
        coverage: { ...projection.coverage, toolIds },
      };
    }
    case "ToolExecutionUpdate": {
      const b = body as { id: string; partial_result: ToolResult };
      const id = `tool:${b.id}`;
      return {
        ...projection,
        items: replaceItem(projection.items, id, (item) =>
          item.kind === "tool"
            ? { ...item, status: "running", partialResult: b.partial_result, ...(output ? { output } : {}) }
            : item,
        ),
      };
    }
    case "ToolExecutionFinished": {
      const b = body as { turn: number; id: string; name: string; result: ToolResult };
      const toolIds = new Set(projection.coverage.toolIds);
      toolIds.add(b.id);
      const completed = [...projection.completedToolResults, b.id];
      const id = `tool:${b.id}`;
      const item: ToolItem = {
        kind: "tool",
        id,
        name: b.name,
        arguments: undefined,
        status: b.result.is_error ? "failed" : "finished",
        result: b.result,
        turn: b.turn,
        ...(output ? { output } : {}),
      };
      const existing = projection.items.find((entry) => entry.id === id);
      const merged: ToolItem =
        existing && existing.kind === "tool"
          ? {
              ...existing,
              status: item.status,
              result: b.result,
              ...(output ? { output } : {}),
            }
          : item;
      const items = upsertById(projection.items, merged);
      // A Bash tool's runtime row is the one shown in the transcript. Carry
      // its final tool error onto that row for failures without an exit code.
      const projectedItems =
        b.name.toLowerCase() === "bash" && b.result.is_error
          ? replaceItem(items, `shell:${b.id}`, (entry) =>
              entry.kind === "shell" ? { ...entry, status: "failed" } : entry,
            )
          : items;
      return {
        ...projection,
        items: projectedItems,
        completedToolResults: completed,
        coverage: { ...projection.coverage, toolIds },
      };
    }

    // -- Approvals.
    case "ApprovalRequested": {
      const b = body as { request: ApprovalRequest };
      const item: ApprovalItem = {
        kind: "approval",
        id: `approval:${b.request.id}`,
        request: b.request,
      };
      return {
        ...projection,
        items: upsertById(projection.items, item),
        pendingApprovalId: b.request.id,
      };
    }
    case "ApprovalResolved": {
      const b = body as { request_id: string; resolution: ApprovalResolution };
      const id = `approval:${b.request_id}`;
      return {
        ...projection,
        items: replaceItem(projection.items, id, (item) =>
          item.kind === "approval" ? { ...item, resolution: b.resolution } : item,
        ),
        pendingApprovalId:
          projection.pendingApprovalId === b.request_id
            ? null
            : projection.pendingApprovalId,
      };
    }

    // -- Questions.
    case "QuestionRequested": {
      const b = body as { turn: number; id: string; questions: QuestionEventData[] };
      const item: QuestionItem = {
        kind: "question",
        id: `question:${b.id}`,
        turn: b.turn,
        questions: b.questions,
        resolved: false,
      };
      return {
        ...projection,
        items: upsertById(projection.items, item),
        pendingQuestionIds: projection.pendingQuestionIds.includes(b.id)
          ? projection.pendingQuestionIds
          : [...projection.pendingQuestionIds, b.id],
      };
    }
    // -- Shell commands.
    case "ShellCommandQueued":
    case "ShellCommandStarted": {
      const b = body as { turn: number; id: string; command: string; cwd: string };
      const item: ShellItem = {
        kind: "shell",
        id: `shell:${b.id}`,
        command: b.command,
        cwd: b.cwd,
        status: tag === "ShellCommandQueued" ? "queued" : "running",
        stdout: "",
        stderr: "",
        truncated: false,
        turn: b.turn,
      };
      return { ...projection, items: upsertById(projection.items, item) };
    }
    case "ShellCommandQueueUpdated": {
      const b = body as { id: string; position: number; waiting_ms: number };
      const id = `shell:${b.id}`;
      return {
        ...projection,
        items: replaceItem(projection.items, id, (item) =>
          item.kind === "shell"
            ? { ...item, status: "queued", queuePosition: b.position, queueWaitingMs: b.waiting_ms }
            : item,
        ),
      };
    }
    case "ShellCommandFinished": {
      const b = body as {
        turn: number;
        id: string;
        exit_code: number | null;
        stdout: string;
        stderr: string;
        truncated: boolean;
      };
      const id = `shell:${b.id}`;
      const existing = projection.items.find((entry) => entry.id === id);
      const base: ShellItem =
        existing && existing.kind === "shell"
          ? existing
          : {
              kind: "shell",
              id,
              command: "",
              cwd: "",
              status: "finished",
              stdout: "",
              stderr: "",
              truncated: false,
              turn: b.turn,
            };
      return {
        ...projection,
        items: upsertById(projection.items, {
          ...base,
          status:
            (b.exit_code !== null && b.exit_code !== 0) || base.status === "failed"
              ? "failed"
              : "finished",
          exitCode: b.exit_code,
          stdout: b.stdout,
          stderr: b.stderr,
          truncated: b.truncated,
          turn: b.turn,
          ...(output ? { output } : {}),
        }),
      };
    }

    // -- Terminal sessions.
    case "TerminalSessionStarted": {
      const b = body as { turn: number; id: string; handle: string; command: string; cwd: string };
      const item: TerminalItem = {
        kind: "terminal",
        id: `term:${b.id}`,
        handle: b.handle,
        command: b.command,
        cwd: b.cwd,
        output: "",
        truncated: false,
        finished: false,
        turn: b.turn,
        ...(output ? { outputRef: output } : {}),
      };
      return { ...projection, items: upsertById(projection.items, item) };
    }
    case "TerminalSessionOutput": {
      const b = body as { turn: number; id: string; handle: string; output: string; truncated: boolean };
      const id = `term:${b.id}`;
      const existing = projection.items.find((entry) => entry.id === id);
      const base: TerminalItem =
        existing && existing.kind === "terminal"
          ? existing
          : {
              kind: "terminal",
              id,
              handle: b.handle,
              output: "",
              truncated: false,
              finished: false,
              turn: b.turn,
            };
      return {
        ...projection,
        items: upsertById(projection.items, {
          ...base,
          output: base.output + b.output,
          truncated: b.truncated,
          turn: b.turn,
          ...(output ? { outputRef: output } : {}),
        }),
      };
    }
    case "TerminalSessionFinished": {
      const b = body as { id: string; status: string; exit_code: number | null };
      const id = `term:${b.id}`;
      return {
        ...projection,
        items: replaceItem(projection.items, id, (item) =>
          item.kind === "terminal"
            ? {
                ...item,
                finished: true,
                statusText: b.status,
                exitCode: b.exit_code,
                ...(output ? { outputRef: output } : {}),
              }
            : item,
        ),
      };
    }

    // -- Usage / context projections: latest-wins state, never transcript rows.
    case "TokenUsage": {
      const b = body as { usage: AgentTokenUsage };
      return { ...projection, latestUsage: b.usage };
    }
    case "ContextWindowUpdated": {
      const b = body as {
        used_tokens: number;
        projected_tokens?: number | null;
        max_tokens?: number | null;
        remaining_tokens?: number | null;
      };
      return {
        ...projection,
        contextWindow: {
          used_tokens: b.used_tokens,
          projected_tokens: b.projected_tokens ?? null,
          max_tokens: b.max_tokens ?? null,
          remaining_tokens: b.remaining_tokens ?? null,
        },
      };
    }

    // -- Delegate / DelegateSwarm: keep hierarchy verbatim from snapshots.
    case "DelegateStarted":
    case "DelegateUpdated":
    case "DelegateFinished": {
      const b = body as { turn: number; agent: AgentSnapshot };
      const item: DelegateItem = {
        kind: "delegate",
        id: `delegate:${b.agent.id}`,
        agent: b.agent,
        finished: tag === "DelegateFinished",
        turn: b.turn,
      };
      const existing = projection.items.find((entry) => entry.id === item.id);
      if (existing) {
        return {
          ...projection,
          items: replaceItem(projection.items, item.id, () => ({
            ...item,
            finished: tag === "DelegateFinished" || (existing as DelegateItem).finished,
          })),
        };
      }
      return { ...projection, items: [...projection.items, item] };
    }
    case "DelegateSwarmStarted":
    case "DelegateSwarmUpdated":
    case "DelegateSwarmFinished": {
      const b = body as { turn: number; swarm: SwarmSnapshot };
      const item: SwarmItem = {
        kind: "swarm",
        id: `swarm:${b.swarm.swarm_id}`,
        swarm: b.swarm,
        finished: tag === "DelegateSwarmFinished",
        turn: b.turn,
      };
      const existing = projection.items.find((entry) => entry.id === item.id);
      if (existing) {
        return {
          ...projection,
          items: replaceItem(projection.items, item.id, () => item),
        };
      }
      return { ...projection, items: [...projection.items, item] };
    }

    case "DelegateProgressUpdated": {
      const b = body as { turn: number; progress: AgentProgressSnapshot };
      const id = `delegate:${b.progress.agent_id}`;
      const existing = projection.items.find((entry) => entry.id === id);
      const base: AgentSnapshot =
        existing && existing.kind === "delegate"
          ? existing.agent
          : {
              id: b.progress.agent_id,
              display_name: b.progress.agent_id,
              state: b.progress.state,
            };
      const item: DelegateItem = {
        kind: "delegate",
        id,
        agent: mergeAgentProgress(base, b.progress),
        finished: existing && existing.kind === "delegate" ? existing.finished : false,
        turn: b.turn,
      };
      return { ...projection, items: upsertById(projection.items, item) };
    }
    case "DelegateSwarmProgressUpdated": {
      const b = body as {
        turn: number;
        swarm_id: string;
        state: string;
        aggregate: SwarmAggregate;
        child_progress: { item_index: number; progress: AgentProgressSnapshot };
      };
      const id = `swarm:${b.swarm_id}`;
      const existing = projection.items.find((entry) => entry.id === id);
      const base: SwarmSnapshot =
        existing && existing.kind === "swarm"
          ? existing.swarm
          : {
              swarm_id: b.swarm_id,
              description: b.swarm_id,
              state: b.state,
              max_concurrency: 0,
              aggregate: b.aggregate,
              children: [],
            };
      const children = base.children.slice();
      const childIndex = children.findIndex(
        (child) => child.item_index === b.child_progress.item_index,
      );
      if (childIndex >= 0) {
        const child = children[childIndex];
        children[childIndex] = {
          ...child,
          agent: mergeAgentProgress(child.agent, b.child_progress.progress),
        };
      }
      const item: SwarmItem = {
        kind: "swarm",
        id,
        swarm: { ...base, state: b.state, aggregate: b.aggregate, children },
        finished: existing && existing.kind === "swarm" ? existing.finished : false,
        turn: b.turn,
      };
      return { ...projection, items: upsertById(projection.items, item) };
    }

    // -- Workflows.
    case "WorkflowUpdated":
    case "WorkflowFinished": {
      const b = body as { turn: number; workflow: WorkflowSnapshot };
      const item: WorkflowItem = {
        kind: "workflow",
        id: `wf:${b.workflow.id}`,
        workflow: b.workflow,
        finished: tag === "WorkflowFinished",
        turn: b.turn,
      };
      const existing = projection.items.find((entry) => entry.id === item.id);
      if (existing) {
        return {
          ...projection,
          items: replaceItem(projection.items, item.id, () => item),
        };
      }
      return { ...projection, items: [...projection.items, item] };
    }

    // -- Tasks: projected to the floating list, never edited locally.
    case "TodoUpdated": {
      const b = body as { todos: TodoEventData[] };
      return { ...projection, todos: b.todos };
    }

    // -- Canonical message appends.
    case "MessageAppended": {
      const b = body as { message: AgentMessage };
      return applyMessageAppended(projection, b.message, projection.latestTurn);
    }

    // -- Errors become visible status lines.
    case "Error": {
      const b = body as { message: string };
      const [id, counter] = nextAppendedId(projection, "status");
      const item: StatusLineItem = {
        kind: "status",
        id,
        severity: "error",
        text: b.message,
      };
      return {
        ...projection,
        items: [...projection.items, item],
        appendedCounter: counter,
      };
    }

    default: {
      if (KNOWN_SILENT_TAGS.has(tag)) {
        // Tool call ids still feed append coverage.
        if (tag === "ToolCallStarted" || tag === "ToolCallFinished") {
          const b = body as { id?: string; tool_call?: { id: string } };
          const toolId = b.id ?? b.tool_call?.id;
          if (toolId) {
            const toolIds = new Set(projection.coverage.toolIds);
            toolIds.add(toolId);
            return { ...projection, coverage: { ...projection.coverage, toolIds } };
          }
        }
        return projection;
      }
      // Unknown tag: keep a collapsible safe raw JSON record; never throw,
      // never drop later events, never interpret it as success/failure.
      const [id, counter] = nextAppendedId(projection, "unknown");
      const raw = safeJson(event);
      const item: UnknownItem = { kind: "unknown", id, tag: tag || "unknown", raw };
      return {
        ...projection,
        items: [...projection.items, item],
        appendedCounter: counter,
      };
    }
  }
}

function safeJson(value: unknown): string {
  try {
    const text = JSON.stringify(value, null, 2);
    return text.length > 8000 ? `${text.slice(0, 8000)}\n…` : text;
  } catch {
    return "{}";
  }
}

function upsertRetry(projection: TranscriptProjection, item: RetryItem): TranscriptProjection {
  return { ...projection, items: upsertById(projection.items, item) };
}

function upsertById(items: TranscriptItem[], item: TranscriptItem): TranscriptItem[] {
  const index = items.findIndex((entry) => entry.id === item.id);
  if (index < 0) return [...items, item];
  const next = items.slice();
  next[index] = item;
  return next;
}

/** Merge a live AgentProgressSnapshot into the last full AgentSnapshot:
 * progress fields win, identity fields (display_name, task) survive. */
function mergeAgentProgress(
  agent: AgentSnapshot,
  progress: AgentProgressSnapshot,
): AgentSnapshot {
  const elapsedMs = progress.elapsed_ms;
  return {
    ...agent,
    state: progress.state ?? agent.state,
    mode: progress.mode ?? agent.mode,
    detached_from_foreground:
      progress.detached_from_foreground ?? agent.detached_from_foreground,
    updated_at_ms: progress.updated_at_ms ?? agent.updated_at_ms,
    terminal_reason: progress.terminal_reason ?? agent.terminal_reason,
    run_count: progress.run_count ?? agent.run_count,
    live_messages_received:
      progress.live_messages_received ?? agent.live_messages_received,
    tool_count: progress.tool_count ?? agent.tool_count,
    token_count: progress.token_count ?? agent.token_count,
    latest_text: progress.latest_text ?? agent.latest_text,
    elapsed:
      elapsedMs !== undefined
        ? { secs: Math.floor(elapsedMs / 1000), nanos: (elapsedMs % 1000) * 1_000_000 }
        : agent.elapsed,
  };
}

function applyMessageAppended(
  projection: TranscriptProjection,
  message: AgentMessage,
  turn: number | null,
): TranscriptProjection {
  const record = message as Record<string, unknown>;
  if ("User" in record) {
    if (isInjectedUserMessage(message)) return projection;
    const text = userDisplayText(message);
    const coverage = resetCoverage();
    if (text.trim() === "") {
      return { ...projection, coverage };
    }
    const [id, counter] = nextAppendedId(projection, "user");
    const item: UserMessageItem = { kind: "user_message", id, text };
    return {
      ...projection,
      items: [...projection.items, item],
      coverage,
      liveMessageId: null,
      liveThinkingId: null,
      appendedCounter: counter,
    };
  }
  if ("Assistant" in record) {
    const assistant = (message as { Assistant: { content: unknown[]; tool_calls?: { id: string }[] } }).Assistant;
    const { text, thinking, thinkingRedacted } = messageText(message);
    const coverage = projection.coverage;
    const coveredTools = (assistant.tool_calls ?? []).every((call) =>
      coverage.toolIds.has(call.id),
    );
    // Exact-match rule (mirrors ReplayAggregateCoverage): when the streamed
    // detail fully covers the appended message, skip; otherwise render the
    // canonical message in full.
    if (
      coverage.hasDetail &&
      coverage.text === text &&
      coverage.thinking === thinking &&
      coverage.thinkingRedacted === thinkingRedacted &&
      coveredTools
    ) {
      return { ...projection, coverage: resetCoverage() };
    }
    const [id, counter] = nextAppendedId(projection, "assistant");
    const items = [...projection.items];
    let nextCounter = counter;
    if (thinking !== "" && coverage.thinking !== thinking) {
      const [thinkId, c2] = [`think:appended:${nextCounter}`, nextCounter + 1];
      nextCounter = c2;
      items.push({
        kind: "thinking",
        id: thinkId,
        text: thinking,
        finished: true,
        redacted: thinkingRedacted,
        ...(turn !== null ? { turn } : {}),
      } satisfies ThinkingItem);
    }
    if (text.trim() !== "") {
      items.push({
        kind: "assistant_message",
        id,
        text,
        finished: true,
        ...(turn !== null ? { turn } : {}),
      } satisfies AssistantMessageItem);
    }
    return {
      ...projection,
      items,
      coverage: resetCoverage(),
      liveMessageId: null,
      liveThinkingId: null,
      appendedCounter: nextCounter,
    };
  }
  if ("ToolResult" in record) {
    const result = record.ToolResult as {
      tool_call_id: string;
      name?: string;
      content?: unknown[];
      is_error?: boolean;
    };
    const coveredIndex = projection.completedToolResults.indexOf(result.tool_call_id);
    const completed = projection.completedToolResults.slice();
    if (coveredIndex >= 0) {
      completed.splice(coveredIndex, 1);
      return { ...projection, completedToolResults: completed };
    }
    // Uncovered canonical tool result: surface it as a finished tool card.
    let content = "";
    for (const part of result.content ?? []) {
      const r = part as Record<string, Record<string, unknown>>;
      if (r?.Text && typeof r.Text.text === "string") content += r.Text.text;
    }
    const item: ToolItem = {
      kind: "tool",
      id: `tool:${result.tool_call_id}`,
      name: result.name ?? "tool",
      arguments: undefined,
      status: result.is_error ? "failed" : "finished",
      result: { content, is_error: result.is_error === true },
    };
    return { ...projection, items: upsertById(projection.items, item), completedToolResults: completed };
  }
  if ("ShellCommand" in record) {
    const shell = record.ShellCommand as {
      command: string;
      stdout: string;
      stderr: string;
      exit_code: number | null;
      truncated: boolean;
    };
    const [id, counter] = nextAppendedId(projection, "shell");
    const item: ShellItem = {
      kind: "shell",
      id,
      command: shell.command,
      cwd: "",
      status: "finished",
      stdout: shell.stdout,
      stderr: shell.stderr,
      truncated: shell.truncated,
      exitCode: shell.exit_code,
    };
    return {
      ...projection,
      items: [...projection.items, item],
      appendedCounter: counter,
    };
  }
  // System and other message variants are context-only: not transcript UI.
  return projection;
}

/** Rebuild the projection from a snapshot's history (the authoritative
 * watermark). Failed retry attempts never appear in it. */
export function buildFromHistory(
  history: WebUiHistoryEntry[],
  todos: TodoEventData[],
): TranscriptProjection {
  let projection = emptyProjection();
  for (const entry of history) {
    projection = applyAgentEvent(projection, entry.event, entry.output ?? null);
  }
  return { ...projection, todos };
}
