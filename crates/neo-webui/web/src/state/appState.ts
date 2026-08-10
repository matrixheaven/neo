/**
 * App-level state shards (handoff section 4). State is sharded per session;
 * there is no global isStreaming and no global transcript array.
 */

import type {
  AgentSnapshot,
  WebUiBootstrap,
  WebUiPhase,
  WebUiServerMessage,
  WebUiSessionMetadata,
  WebUiSessionSummary,
  WebUiSessionStarted,
  WebUiWorkspaceGroup,
} from "../protocol";
import type { TranscriptProjection } from "./transcript";
import { emptyProjection } from "./transcript";
import type { Theme } from "./theme";

// Design §3: sidebar 264px default, min 220, max 400 (drag + keyboard).
export const SIDEBAR_DEFAULT = 264;
export const SIDEBAR_MIN = 220;
export const SIDEBAR_MAX = 400;

export interface SessionViewState {
  sessionId: string;
  streamId: string | null;
  /** Last applied per-session sequence (snapshot watermark or event). */
  cursor: number;
  projection: TranscriptProjection;
  phase: WebUiPhase | null;
  waitingApproval: boolean;
  waitingQuestion: boolean;
  currentTurnId: string | null;
  metadata: WebUiSessionMetadata | null;
  draft: string;
  /** Explicit per-line expansion overrides. Absent = follow the line's
   * phase-dependent default (e.g. think streams open and auto-collapses on
   * finish); once the user clicks, their choice wins across phase changes. */
  lineOverrides: Record<string, boolean>;
  isAtBottom: boolean;
  /** Set when the cursor cannot resume (gap or stream change); the
   * connection layer re-subscribes without a cursor. */
  resyncNeeded: boolean;
  /** Controls submitted locally, disabled until the server confirms via an
   * event or snapshot. Stale (409) responses remove the marker. */
  submittedApprovalIds: string[];
  submittedQuestionIds: string[];
  sending: boolean;
}

export function emptySessionView(sessionId: string): SessionViewState {
  return {
    sessionId,
    streamId: null,
    cursor: 0,
    projection: emptyProjection(),
    phase: null,
    waitingApproval: false,
    waitingQuestion: false,
    currentTurnId: null,
    metadata: null,
    draft: "",
    lineOverrides: {},
    isAtBottom: true,
    resyncNeeded: false,
    submittedApprovalIds: [],
    submittedQuestionIds: [],
    sending: false,
  };
}

/** Drop the full transcript of a session that is no longer current. Draft,
 * expansion state and scroll anchor survive (tab memory only). */
export function dropTranscript(view: SessionViewState): SessionViewState {
  return {
    ...view,
    streamId: null,
    cursor: 0,
    projection: emptyProjection(),
    phase: view.phase,
    waitingApproval: false,
    waitingQuestion: false,
    currentTurnId: view.currentTurnId,
    resyncNeeded: true,
    submittedApprovalIds: [],
    submittedQuestionIds: [],
    sending: false,
  };
}

export type AuthState = "pending" | "ok" | "failed";
export type ConnectionState = "connecting" | "open" | "reconnecting";

export interface AppState {
  auth: AuthState;
  bootstrap: WebUiBootstrap | null;
  selectedSessionId: string | null;
  /** A create-session request is in flight (double-submit guard). */
  creatingSession: boolean;
  sidebarWidth: number;
  sidebarDrawerOpen: boolean;
  /** Active color theme; mirrored onto document.documentElement[data-theme]. */
  theme: Theme;
  /** Session id whose context menu is open, if any. */
  activeContextMenu: string | null;
  summaries: WebUiSessionSummary[];
  /** Grouped cross-workspace aggregation from the latest workspace_snapshot
   * (flat `summaries` above is derived from it; grouped sidebar UI is R5). */
  workspaces: WebUiWorkspaceGroup[];
  workspaceStreamId: string | null;
  workspaceCursor: number;
  sessions: Record<string, SessionViewState>;
  connection: ConnectionState;
  /** Non-sensitive one-line notice (e.g. input too large, stale refresh). */
  notice: string | null;
  /** Drill-down panel for a child agent (R4 §5.2). UI-only state: the panel
   * data is fetched lazily and discarded on close; switching sessions closes
   * the panel. */
  agentPanel: AgentPanelTarget | null;
}

/** Which child agent the drill-down panel shows. The snapshot is carried for
 * the header (title/state/elapsed/tokens) so the header never waits on the
 * history fetch. */
export interface AgentPanelTarget {
  sessionId: string;
  agentId: string;
  agent: AgentSnapshot;
}

export function initialAppState(sidebarWidth: number, theme: Theme): AppState {
  return {
    auth: "pending",
    bootstrap: null,
    selectedSessionId: null,
    creatingSession: false,
    sidebarWidth,
    sidebarDrawerOpen: false,
    theme,
    activeContextMenu: null,
    summaries: [],
    workspaces: [],
    workspaceStreamId: null,
    workspaceCursor: 0,
    sessions: {},
    connection: "connecting",
    notice: null,
    agentPanel: null,
  };
}

export type AppAction =
  | { type: "auth_result"; ok: boolean }
  | { type: "bootstrap_loaded"; bootstrap: WebUiBootstrap }
  | { type: "connection_changed"; connection: ConnectionState }
  | { type: "server_message"; message: WebUiServerMessage }
  | { type: "select_session"; sessionId: string | null }
  | { type: "set_sidebar_width"; width: number }
  | { type: "set_drawer_open"; open: boolean }
  | { type: "theme_changed"; theme: Theme }
  | { type: "set_context_menu"; sessionId: string | null }
  | { type: "draft_changed"; sessionId: string; text: string }
  | { type: "set_line_expanded"; sessionId: string; itemId: string; expanded: boolean }
  | { type: "set_at_bottom"; sessionId: string; atBottom: boolean }
  | { type: "session_started"; started: WebUiSessionStarted }
  | { type: "create_started" }
  | { type: "create_finished" }
  | { type: "send_started"; sessionId: string }
  | { type: "send_finished"; sessionId: string }
  | { type: "approval_submitted"; sessionId: string; requestId: string }
  | { type: "approval_stale"; sessionId: string; requestId: string }
  | { type: "question_submitted"; sessionId: string; questionId: string }
  | { type: "question_stale"; sessionId: string; questionId: string }
  | { type: "notice"; text: string }
  | { type: "clear_notice" }
  | { type: "open_agent_panel"; panel: AgentPanelTarget }
  | { type: "close_agent_panel" };
