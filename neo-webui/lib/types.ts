// ── Messages ──────────────────────────────────────────────────────────────

export interface Message {
  id: string;
  parentId: string | null;
  role: "user" | "assistant" | "system";
  content: MessageContentPart[];
  toolCalls: ToolCall[];
}

export type MessageContentPart =
  | { Text: { text: string } }
  | { ToolCall: { id: string; name: string; arguments: string } }
  | { ToolResult: { id: string; name: string; content: ToolResultContent[] } }
  | { Thinking: { text: string; signature?: string; redacted?: boolean } };

export interface ToolCall {
  id: string;
  name: string;
  arguments: string;
}

// ── Tool results ──────────────────────────────────────────────────────────

export type ToolResultContent =
  | { text: string; is_diff?: boolean }
  | { path: string; diff: string };

export interface ToolResult {
  content: string;
  isError: boolean;
  details?: unknown;
  terminate: boolean;
}

// ── Approval ──────────────────────────────────────────────────────────────

export type ApprovalPresentation =
  | { kind: "command"; title: string; command: string; cwd?: string }
  | { kind: "tool"; title: string; details: string[] }
  | { kind: "edit"; title: string; edit: unknown }
  | { kind: "write"; title: string; write: unknown }
  | { kind: "plan"; title: string; path?: string; markdown: string; summary?: string }
  | { kind: "goal"; title: string; objective: string; completion_criterion?: string; phases: string[] }
  | { kind: "workflow"; title: string; workflow: unknown };

export type ApprovalAction =
  | { kind: "permit_once" }
  | { kind: "permit_for_session"; scope: unknown }
  | { kind: "permit_for_prefix"; rule: unknown }
  | { kind: "reject" }
  | { kind: "approve_plan"; selection?: { label: string; description?: string } }
  | { kind: "revise_plan"; preset_feedback?: string }
  | { kind: "reject_plan" }
  | { kind: "start_goal" }
  | { kind: "revise_goal"; preset_feedback?: string }
  | { kind: "reject_goal" }
  | { kind: "launch_workflow" }
  | { kind: "revise_workflow"; preset_feedback?: string }
  | { kind: "cancel_workflow" };

export interface ApprovalOption {
  label: string;
  description?: string;
  action: ApprovalAction;
}

export interface ApprovalRequest {
  turn: number;
  id: string;
  operation: string;
  presentation: ApprovalPresentation;
  options: ApprovalOption[];
}

export type ApprovalResolution =
  | { kind: "selected"; action: ApprovalAction; label: string; feedback?: string }
  | { kind: "cancelled"; reason: string };

// ── Agent events (notification payloads from "agent.event") ───────────────

export type AgentEvent =
  | { RunStarted: { turn: number } }
  | { TurnStarted: { turn: number } }
  | { RetryScheduled: { turn: number; retry: number; max_retries: number; delay_ms: number; error_code: string; message: string } }
  | { RetryStarted: { turn: number; retry: number; max_retries: number } }
  | { RetryResumed: { turn: number; retry: number } }
  | { RetrySucceeded: { turn: number; retries_used: number } }
  | { RetryExhausted: { turn: number; retries_used: number; error_code: string; message: string } }
  | { MessageStarted: { turn: number; id: string } }
  | { MessageFinished: { turn: number; id: string; stop_reason: string } }
  | { TextDelta: { turn: number; text: string } }
  | { ThinkingStarted: { turn: number; id: string } }
  | { ThinkingDelta: { turn: number; text: string } }
  | { ThinkingFinished: { turn: number; signature?: string; redacted: boolean } }
  | { ToolCallStarted: { turn: number; id: string; name: string } }
  | { ToolCallArgumentsDelta: { turn: number; id: string; json_fragment: string } }
  | { ToolCallFinished: { turn: number; tool_call: ToolCallWire } }
  | { ToolExecutionStarted: { turn: number; id: string; name: string; arguments: unknown } }
  | { ToolExecutionQueued: { turn: number; id: string; name: string; arguments: unknown } }
  | { ToolExecutionQueueUpdated: { turn: number; id: string; position: number; waiting_ms: number } }
  | { ToolExecutionFinished: { turn: number; id: string; name: string; result: ToolResult } }
  | { ToolExecutionUpdate: { turn: number; id: string; name: string; partial_result: ToolResult } }
  | { ApprovalRequested: { request: ApprovalRequest } }
  | { ApprovalResolved: { turn: number; request_id: string; resolution: ApprovalResolution } }
  | { ContextWindowUpdated: { turn: number; used_tokens: number; projected_tokens?: number; max_tokens?: number; trigger_tokens?: number; remaining_tokens?: number } }
  | { CompactionStarted: { reason: string; tokens_before: number; message_count: number } }
  | { CompactionProgress: { phase: string; percent: number } }
  | { CompactionApplied: { summary: string; tokens_before: number; tokens_after: number; first_kept_message_index: number } }
  | { Error: { turn: number; message: string; code?: string; retry_after?: number } }
  | { TurnFinished: { turn: number; stop_reason: string } }
  | { RunFinished: { turn: number; stop_reason: string } };

interface ToolCallWire {
  id: string;
  name: string;
  arguments: string;
}

// ── Session state ─────────────────────────────────────────────────────────

export interface SessionState {
  id: string;
  name?: string;
  title?: string;
  workspace?: string;
  updated_at?: string;
}
