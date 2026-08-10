/**
 * Mirrors crates/neo-webui/src/protocol.rs and the fixed sample
 * crates/neo-webui/fixtures/webui-events.json. Fields are snake_case and
 * derived only from the final protocol — never from free text.
 */

// ---------------------------------------------------------------------------
// neo-ai / neo-agent-core wire shapes carried inside AgentEvent.
// ---------------------------------------------------------------------------

export type PermissionMode = "ask" | "auto" | "yolo";

export type StopReason =
  | "EndTurn"
  | "ToolUse"
  | "MaxTokens"
  | "Cancelled"
  | "Error";

export type MessagePhase = "commentary" | "final_answer";
export type ThinkingKind = "full" | "summary";

export interface TextContentPart {
  Text: { text: string };
}
export interface ThinkingContentPart {
  Thinking: { text: string; signature?: string | null };
}
export type ContentPart = TextContentPart | ThinkingContentPart | Record<string, unknown>;

export interface AgentToolCall {
  id: string;
  name: string;
  arguments: unknown;
}

export interface AgentMessageUser {
  User: { content: ContentPart[] };
}
export interface AgentMessageAssistant {
  Assistant: {
    content: ContentPart[];
    tool_calls: AgentToolCall[];
    stop_reason: StopReason | null;
  };
}
export type AgentMessage =
  | AgentMessageUser
  | AgentMessageAssistant
  | Record<string, unknown>;

export interface ToolResult {
  content: string;
  is_error: boolean;
  details?: unknown;
  terminate?: boolean;
}

/** Per-turn model token accounting (AgentTokenUsage in neo-agent-core). */
export interface AgentTokenUsage {
  input_tokens: number;
  output_tokens: number;
  input_cache_read_tokens?: number;
  input_cache_write_tokens?: number;
}

/** Latest context-window occupancy cached from ContextWindowUpdated. */
export interface WebUiContextWindow {
  used_tokens: number;
  projected_tokens?: number | null;
  max_tokens?: number | null;
  remaining_tokens?: number | null;
}

export interface ApprovalAction {
  kind: string;
  [key: string]: unknown;
}

export interface ApprovalOption {
  label: string;
  description?: string | null;
  action: ApprovalAction;
}

export interface ApprovalPresentation {
  kind: string;
  title?: string | null;
  command?: string | null;
  cwd?: string | null;
  [key: string]: unknown;
}

export interface ApprovalRequest {
  turn: number;
  id: string;
  operation: string;
  presentation: ApprovalPresentation;
  options: ApprovalOption[];
}

export interface ApprovalResolution {
  kind: string;
  action?: ApprovalAction | null;
  label?: string | null;
  feedback?: string | null;
}

export interface QuestionOption {
  label: string;
  description?: string | null;
}

export interface QuestionEventData {
  question: string;
  header: string;
  body?: string | null;
  options: QuestionOption[];
  multi_select: boolean;
}

export interface TodoEventData {
  title: string;
  status: string;
}

export interface AgentSnapshot {
  id: string;
  display_name: string;
  path?: string | null;
  role?: string | null;
  mode?: string | null;
  context?: string | null;
  state: string;
  task?: string | null;
  task_title?: string | null;
  created_at_ms?: number | null;
  updated_at_ms?: number | null;
  started_at_ms?: number | null;
  detached_from_foreground?: boolean | null;
  terminal_reason?: string | null;
  run_count?: number | null;
  live_messages_received?: number | null;
  tool_count?: number | null;
  token_count?: number | null;
  elapsed?: { secs: number; nanos: number } | null;
  latest_text?: string | null;
}

/** Live progress payload of DelegateProgressUpdated (AgentProgressSnapshot). */
export interface AgentProgressSnapshot {
  agent_id: string;
  state: string;
  mode?: string | null;
  detached_from_foreground?: boolean;
  started_at_ms?: number | null;
  updated_at_ms?: number;
  terminal_at_ms?: number | null;
  terminal_reason?: string | null;
  run_count?: number;
  live_messages_received?: number;
  tool_count?: number;
  token_count?: number;
  elapsed_ms?: number;
  latest_text?: string | null;
  latest_thinking?: string | null;
  last_tool?: {
    id: string;
    name: string;
    summary?: string | null;
    phase?: string | null;
  } | null;
  outcome?: unknown;
}

export interface SwarmAggregate {
  total: number;
  queued: number;
  running: number;
  completed: number;
  failed: number;
  cancelled: number;
  timed_out: number;
}

export interface SwarmChild {
  item_index: number;
  item: string;
  agent: AgentSnapshot;
}

export interface SwarmSnapshot {
  swarm_id: string;
  description: string;
  role?: string | null;
  mode?: string | null;
  state: string;
  max_concurrency: number;
  aggregate: SwarmAggregate;
  children: SwarmChild[];
}

export interface WorkflowSnapshot {
  id: string;
  title: string;
  state: string;
  current_phase?: string | null;
  started_at_ms?: number | null;
  updated_at_ms?: number | null;
  invocation_count?: number | null;
  failure_count?: number | null;
  latest_log_summary?: string | null;
  display_name?: string | null;
  purpose?: string | null;
  terminal_reason?: string | null;
}

// ---------------------------------------------------------------------------
// AgentEvent: externally tagged, carried verbatim on the wire.
// ---------------------------------------------------------------------------

export type AgentEvent =
  | { RunStarted: { turn: number } }
  | { TurnStarted: { turn: number } }
  | {
      RetryScheduled: {
        turn: number;
        retry: number;
        max_retries: number;
        delay_ms: number;
        error_code: string;
        message: string;
      };
    }
  | { RetryStarted: { turn: number; retry: number; max_retries: number } }
  | { RetryResumed: { turn: number; retry: number } }
  | { RetrySucceeded: { turn: number; retries_used: number } }
  | {
      RetryExhausted: {
        turn: number;
        retries_used: number;
        error_code: string;
        message: string;
      };
    }
  | { MessageStarted: { turn: number; id: string; phase?: MessagePhase } }
  | {
      MessageFinished: {
        turn: number;
        id: string;
        stop_reason: StopReason;
        phase?: MessagePhase;
      };
    }
  | { TextDelta: { turn: number; text: string } }
  | { ThinkingStarted: { turn: number; id: string; kind?: ThinkingKind } }
  | { ThinkingDelta: { turn: number; text: string } }
  | {
      ThinkingFinished: {
        turn: number;
        signature?: string | null;
        redacted: boolean;
      };
    }
  | { ToolCallStarted: { turn: number; id: string; name: string } }
  | { ToolCallArgumentsDelta: { turn: number; id: string; json_fragment: string } }
  | { ToolCallFinished: { turn: number; tool_call: AgentToolCall } }
  | {
      ToolExecutionStarted: {
        turn: number;
        id: string;
        name: string;
        arguments: unknown;
      };
    }
  | {
      ToolExecutionQueued: {
        turn: number;
        id: string;
        name: string;
        arguments: unknown;
      };
    }
  | {
      ToolExecutionQueueUpdated: {
        turn: number;
        id: string;
        position: number;
        waiting_ms: number;
      };
    }
  | {
      ToolExecutionFinished: {
        turn: number;
        id: string;
        name: string;
        result: ToolResult;
      };
    }
  | {
      ToolExecutionUpdate: {
        turn: number;
        id: string;
        name: string;
        partial_result: ToolResult;
      };
    }
  | { ApprovalRequested: { request: ApprovalRequest } }
  | {
      ApprovalResolved: {
        turn: number;
        request_id: string;
        resolution: ApprovalResolution;
      };
    }
  | {
      QuestionRequested: {
        turn: number;
        id: string;
        questions: QuestionEventData[];
      };
    }
  | {
      QuestionResolved: {
        turn: number;
        question_id: string;
        [key: string]: unknown;
      };
    }
  | {
      ShellCommandStarted: {
        turn: number;
        id: string;
        command: string;
        cwd: string;
        origin?: unknown;
      };
    }
  | {
      ShellCommandQueued: {
        turn: number;
        id: string;
        command: string;
        cwd: string;
        origin?: unknown;
      };
    }
  | {
      ShellCommandQueueUpdated: {
        turn: number;
        id: string;
        position: number;
        waiting_ms: number;
      };
    }
  | {
      ShellCommandFinished: {
        turn: number;
        id: string;
        exit_code: number | null;
        signal?: number | null;
        stdout: string;
        stderr: string;
        truncated: boolean;
        outcome?: unknown;
      };
    }
  | {
      TerminalSessionStarted: {
        turn: number;
        id: string;
        handle: string;
        command: string;
        cwd: string;
        cols?: number;
        rows?: number;
      };
    }
  | {
      TerminalSessionOutput: {
        turn: number;
        id: string;
        handle: string;
        output: string;
        truncated: boolean;
      };
    }
  | {
      TerminalSessionFinished: {
        turn: number;
        id: string;
        handle: string;
        status: string;
        exit_code: number | null;
      };
    }
  | { DelegateStarted: { turn: number; agent: AgentSnapshot } }
  | { DelegateUpdated: { turn: number; agent: AgentSnapshot } }
  | { DelegateFinished: { turn: number; agent: AgentSnapshot } }
  | { DelegateProgressUpdated: { turn: number; progress: AgentProgressSnapshot } }
  | { DelegateSwarmStarted: { turn: number; swarm: SwarmSnapshot } }
  | { DelegateSwarmUpdated: { turn: number; swarm: SwarmSnapshot } }
  | { DelegateSwarmFinished: { turn: number; swarm: SwarmSnapshot } }
  | {
      DelegateSwarmProgressUpdated: {
        turn: number;
        swarm_id: string;
        state: string;
        aggregate: SwarmAggregate;
        child_progress: { item_index: number; progress: AgentProgressSnapshot };
      };
    }
  | { TokenUsage: { turn: number; usage: AgentTokenUsage } }
  | {
      ContextWindowUpdated: {
        turn: number;
        used_tokens: number;
        projected_tokens?: number | null;
        max_tokens?: number | null;
        remaining_tokens?: number | null;
      };
    }
  | { WorkflowUpdated: { turn: number; workflow: WorkflowSnapshot } }
  | { WorkflowFinished: { turn: number; workflow: WorkflowSnapshot } }
  | { TodoUpdated: { turn: number; todos: TodoEventData[] } }
  | { SteeringQueued: { message: AgentMessage } }
  | { FollowUpQueued: { message: AgentMessage } }
  | { QueueDrained: { kind: unknown; count: number } }
  | { MessageAppended: { message: AgentMessage } }
  | { TurnFinished: { turn: number; stop_reason: StopReason } }
  | { RunFinished: { turn: number; stop_reason: StopReason } }
  | {
      Error: {
        turn: number;
        message: string;
        code?: string | null;
        retry_after?: number | null;
      };
    }
  // Unknown future event tags are preserved verbatim as collapsible records.
  | Record<string, unknown>;

/** The tag (variant name) of an externally tagged AgentEvent. */
export function agentEventTag(event: AgentEvent): string {
  const keys = Object.keys(event);
  return keys.length > 0 ? keys[0] : "";
}

// ---------------------------------------------------------------------------
// neo-webui protocol (protocol.rs).
// ---------------------------------------------------------------------------

export type WebUiDevelopmentMode = "normal" | "plan" | "goal";

export type WebUiPhase =
  | "starting"
  | "running"
  | "finishing"
  | "idle"
  | "cancelled"
  | "failed";

export type WebUiSummaryState =
  | "idle"
  | "running"
  | "waiting_approval"
  | "waiting_question"
  | "failed";

export type WebUiInputDelivery = "follow_up" | "steer";

export interface WebUiComposer {
  model?: string;
  reasoning_effort?: string;
  permission_mode?: PermissionMode;
  development_mode?: WebUiDevelopmentMode;
}

export interface WebUiSessionSummary {
  session_id: string;
  title?: string | null;
  updated_at?: string | null;
  pinned: boolean;
  archived: boolean;
  state: WebUiSummaryState;
  workspace_label?: string;
}

export interface WebUiSessionPage {
  items: WebUiSessionSummary[];
  next_cursor?: string | null;
}

export interface WebUiSessionState {
  phase: WebUiPhase;
  waiting_approval: boolean;
  waiting_question: boolean;
  current_turn_id?: string | null;
  token_usage?: AgentTokenUsage | null;
  context_window?: WebUiContextWindow | null;
}

export interface WebUiSessionMetadata {
  title?: string | null;
  pinned: boolean;
  archived: boolean;
  updated_at?: string | null;
}

export interface WebUiPendingApproval {
  request_id: string;
  turn_id: string;
  presentation: ApprovalPresentation;
  options: ApprovalOption[];
}

export interface WebUiPendingQuestion {
  id: string;
  turn_id: string;
  questions: QuestionEventData[];
}

/** Opaque display metadata for full tool or terminal output. Pass `id` back
 * verbatim to the tool-output endpoint; never encode/decode it. */
export interface WebUiOutputRef {
  id: string;
  byte_len: number;
  line_count: number;
  complete: boolean;
}

export interface WebUiHistoryEntry {
  sequence: number;
  event: AgentEvent;
  output?: WebUiOutputRef | null;
}

export interface WebUiSnapshot {
  stream_id: string;
  session_id: string;
  watermark: number;
  session: WebUiSessionState;
  metadata: WebUiSessionMetadata;
  history: WebUiHistoryEntry[];
  pending_approval?: WebUiPendingApproval | null;
  pending_question?: WebUiPendingQuestion | null;
  todos?: TodoEventData[];
}

/** One workspace group of the cross-workspace session aggregation. The
 * workspace path never leaves the service; `label` is display-only. */
export interface WebUiWorkspaceGroup {
  label: string;
  current: boolean;
  sessions: WebUiSessionSummary[];
}

export type WebUiServerMessage =
  | {
      type: "workspace_snapshot";
      stream_id: string;
      workspace_sequence: number;
      workspaces: WebUiWorkspaceGroup[];
    }
  | { type: "session_snapshot"; snapshot: WebUiSnapshot }
  | {
      type: "session_summary_changed";
      stream_id: string;
      workspace_sequence: number;
      event: WebUiSessionSummary;
    }
  | {
      type: "session_event";
      stream_id: string;
      session_id: string;
      sequence: number;
      event: AgentEvent;
      output?: WebUiOutputRef | null;
    }
  | {
      type: "session_state";
      stream_id: string;
      session_id: string;
      sequence: number;
      event: WebUiSessionState;
    }
  | {
      type: "session_metadata_changed";
      stream_id: string;
      session_id: string;
      sequence: number;
      event: WebUiSessionMetadata;
    };

export interface WebUiCursor {
  stream_id: string;
  sequence: number;
}

export type WebUiWatchRequest =
  | { type: "watch_workspace"; after?: WebUiCursor }
  | { type: "watch_session"; session_id: string; after?: WebUiCursor };

export interface WebUiQuestionAnswer {
  selections: string[];
  text?: string;
}

/** One entry of the bootstrap model catalog (WebUiModelInfo). */
export interface WebUiModelInfo {
  alias: string;
  provider: string;
  context_window?: number | null;
  capabilities?: string[];
}

export interface WebUiBootstrap {
  workspace_label?: string | null;
  models?: WebUiModelInfo[];
  permission_modes?: PermissionMode[];
  development_modes?: WebUiDevelopmentMode[];
  sessions?: WebUiSessionSummary[];
}

export interface WebUiSessionStarted {
  session_id: string;
  turn_id: string;
  state: WebUiSessionState;
  stream_id: string;
  sequence: number;
}

export interface WebUiInputAccepted {
  turn_id: string;
}

export interface WebUiCancelling {
  turn_id: string;
}

/** ToolOutputRange from neo-agent-core (tool output read response). */
export interface ToolOutputRange {
  text: string;
  start_line: number;
  next_line: number;
  reached_end: boolean;
}

export type WebUiErrorCode =
  | "invalid_request"
  | "unauthorized"
  | "not_found"
  | "session_busy"
  | "turn_transition"
  | "no_active_turn"
  | "stale_turn"
  | "stale_control"
  | "too_large"
  | "output_not_in_session"
  | "internal";

export interface WebUiErrorBody {
  code: WebUiErrorCode;
}
