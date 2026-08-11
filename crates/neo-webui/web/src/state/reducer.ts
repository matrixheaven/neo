/**
 * Pure app reducer driven by the fixed sample. Snapshot rebuilds, sequence
 * dedup, watermark resume and stream replacement are all decided here; the
 * connection layer only transports messages.
 */

import type { WebUiServerMessage, WebUiSnapshot } from "../protocol";
import {
  SIDEBAR_MAX,
  SIDEBAR_MIN,
  type AppAction,
  type AppState,
  type SessionViewState,
  dropTranscript,
  emptySessionView,
} from "./appState";
import { applyAgentEvent, buildFromHistory } from "./transcript";

function clampSidebar(width: number): number {
  return Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, width));
}

function updateSession(
  state: AppState,
  sessionId: string,
  update: (view: SessionViewState) => SessionViewState,
): AppState {
  const current = state.sessions[sessionId] ?? emptySessionView(sessionId);
  return {
    ...state,
    sessions: { ...state.sessions, [sessionId]: update(current) },
  };
}

function applySnapshot(view: SessionViewState, snapshot: WebUiSnapshot): SessionViewState {
  // Rebuild: clear the derived transcript, pending items, tasks and cursor,
  // then rebuild from history, waiting items, tasks and the watermark.
  const base = emptySessionView(snapshot.session_id);
  let projection = buildFromHistory(snapshot.history, snapshot.todos ?? []);
  // The snapshot's cached usage values are host-side latest-wins; they seed
  // the projection so a reconnect/switch restores them before new traffic.
  if (snapshot.session.token_usage) {
    projection = { ...projection, latestUsage: snapshot.session.token_usage };
  }
  if (snapshot.session.context_window) {
    projection = { ...projection, contextWindow: snapshot.session.context_window };
  }
  // Pending questions arrive over the host-control channel rather than the
  // canonical event stream. Project the snapshot payload into the derived
  // transcript so an already-waiting session remains answerable after a
  // reconnect or first open without altering its recorded history.
  const pendingQuestions = snapshot.pending_questions ?? [];
  for (const pendingQuestion of pendingQuestions) {
    projection = applyAgentEvent(projection, {
      QuestionRequested: {
        turn: projection.latestTurn ?? 0,
        id: pendingQuestion.id,
        questions: pendingQuestion.questions,
      },
    });
  }
  // The snapshot's waiting items are the final arbiter of pending state.
  // History cards stay in place, but a card that is no longer pending (and
  // has no explicit resolution) must not remain actionable.
  const pendingApprovalId = snapshot.pending_approval?.request_id ?? null;
  const pendingQuestionIds = pendingQuestions.map((question) => question.id);
  const pendingQuestionIdSet = new Set(pendingQuestionIds);
  projection = {
    ...projection,
    pendingApprovalId,
    pendingQuestionIds,
    items: projection.items.map((item) => {
      if (item.kind === "approval") {
        const cardId = item.request.id;
        if (item.resolution === undefined && cardId !== pendingApprovalId) {
          return { ...item, resolution: { kind: "no_longer_pending" } };
        }
        return item;
      }
      if (item.kind === "question") {
        const cardId = item.id.replace(/^question:/, "");
        return { ...item, resolved: !pendingQuestionIdSet.has(cardId) };
      }
      return item;
    }),
  };
  return {
    ...base,
    draft: view.draft,
    lineOverrides: view.lineOverrides,
    isAtBottom: view.isAtBottom,
    streamId: snapshot.stream_id,
    cursor: snapshot.watermark,
    projection,
    phase: snapshot.session.phase,
    waitingApproval: snapshot.session.waiting_approval,
    waitingQuestion: snapshot.session.waiting_question,
    currentTurnId: snapshot.session.current_turn_id ?? null,
    metadata: snapshot.metadata,
    resyncNeeded: false,
  };
}

function applySessionMessage(state: AppState, message: WebUiServerMessage): AppState {
  switch (message.type) {
    case "workspace_snapshot":
      // Grouped cross-workspace aggregation is the only shape; the flat
      // summary list is derived from it (grouped sidebar UI is R5).
      return {
        ...state,
        workspaces: message.workspaces,
        summaries: message.workspaces.flatMap((group) => group.sessions),
        workspaceStreamId: message.stream_id,
        workspaceCursor: message.workspace_sequence,
        selectedWorkspaceId:
          state.selectedWorkspaceId ?? message.workspaces.find((group) => group.current)?.id ?? null,
      };

    case "session_summary_changed": {
      // Summary layer has its own cursor; dedup by workspace_sequence.
      if (
        state.workspaceStreamId !== null &&
        message.stream_id !== state.workspaceStreamId
      ) {
        // Service restarted: the server will send a fresh workspace snapshot.
        return state;
      }
      if (
        state.workspaceStreamId === message.stream_id &&
        message.workspace_sequence <= state.workspaceCursor
      ) {
        return state;
      }
      const summary = message.event;
      const index = state.summaries.findIndex(
        (entry) => entry.session_id === summary.session_id,
      );
      const summaries = state.summaries.slice();
      if (index >= 0) {
        summaries[index] = summary;
      } else {
        summaries.push(summary);
      }
      // Best-effort sync of the grouped view (source of truth for R5): the
      // session keeps the workspace label recorded on its summary.
      const known = state.workspaces.some((group) =>
        group.sessions.some((entry) => entry.session_id === summary.session_id),
      );
      let workspaces: AppState["workspaces"];
      if (known) {
        workspaces = state.workspaces.map((group) => {
          const sessionIndex = group.sessions.findIndex(
            (entry) => entry.session_id === summary.session_id,
          );
          if (sessionIndex < 0) return group;
          const sessions = group.sessions.slice();
          sessions[sessionIndex] = summary;
          return { ...group, sessions };
        });
      } else {
        // A session the snapshot never listed (e.g. just created): insert it
        // into the group matching its recorded workspace label, falling back
        // to the current workspace group, keeping the group's recency order.
        const targetLabel =
          summary.workspace_label !== undefined &&
          state.workspaces.some((group) => group.label === summary.workspace_label)
            ? summary.workspace_label
            : (state.workspaces.find((group) => group.current)?.label ??
              state.workspaces[0]?.label);
        workspaces = state.workspaces.map((group) => {
          if (group.label !== targetLabel) return group;
          const sessions = group.sessions.slice();
          const insertAt = sessions.findIndex(
            (entry) => (entry.updated_at ?? "") < (summary.updated_at ?? ""),
          );
          if (insertAt < 0) {
            sessions.push(summary);
          } else {
            sessions.splice(insertAt, 0, summary);
          }
          return { ...group, sessions };
        });
      }
      return {
        ...state,
        summaries,
        workspaces,
        workspaceStreamId: message.stream_id,
        workspaceCursor: Math.max(state.workspaceCursor, message.workspace_sequence),
      };
    }

    case "session_snapshot":
      return updateSession(state, message.snapshot.session_id, (view) =>
        applySnapshot(view, message.snapshot),
      );

    case "session_event": {
      const view = state.sessions[message.session_id];
      if (!view) return state;
      if (view.streamId === null) {
        // No snapshot yet: the snapshot is authoritative; ignore early events.
        return state;
      }
      if (message.stream_id !== view.streamId) {
        return updateSession(state, message.session_id, (current) => ({
          ...current,
          resyncNeeded: true,
        }));
      }
      if (message.sequence <= view.cursor) {
        return state; // duplicate: ignore
      }
      if (message.sequence > view.cursor + 1) {
        return updateSession(state, message.session_id, (current) => ({
          ...current,
          resyncNeeded: true,
        }));
      }
      return updateSession(state, message.session_id, (current) => {
        const projection = applyAgentEvent(
          current.projection,
          message.event,
          message.output ?? null,
        );
        return {
          ...current,
          projection,
          cursor: message.sequence,
          waitingApproval: projection.pendingApprovalId !== null ? true : current.waitingApproval,
          waitingQuestion:
            projection.pendingQuestionIds.length > 0 ? true : current.waitingQuestion,
        };
      });
    }

    case "session_state": {
      const view = state.sessions[message.session_id];
      if (!view || view.streamId === null) return state;
      if (message.stream_id !== view.streamId) {
        return updateSession(state, message.session_id, (current) => ({
          ...current,
          resyncNeeded: true,
        }));
      }
      if (message.sequence <= view.cursor) return state;
      if (message.sequence > view.cursor + 1) {
        return updateSession(state, message.session_id, (current) => ({
          ...current,
          resyncNeeded: true,
        }));
      }
      return updateSession(state, message.session_id, (current) => {
        let projection =
          message.event.token_usage || message.event.context_window
            ? {
                ...current.projection,
                latestUsage: message.event.token_usage ?? current.projection.latestUsage,
                contextWindow:
                  message.event.context_window ?? current.projection.contextWindow,
              }
            : current.projection;
        const pendingQuestionIds = projection.pendingQuestionIds;
        if (!message.event.waiting_question && pendingQuestionIds.length > 0) {
          const pendingQuestionIdSet = new Set(pendingQuestionIds);
          projection = {
            ...projection,
            pendingQuestionIds: [],
            items: projection.items.map((item) =>
              item.kind === "question" &&
              pendingQuestionIdSet.has(item.id.replace(/^question:/, ""))
                ? { ...item, resolved: true }
                : item,
            ),
          };
        }
        return {
          ...current,
          cursor: message.sequence,
          phase: message.event.phase,
          waitingApproval: message.event.waiting_approval,
          waitingQuestion: message.event.waiting_question,
          currentTurnId: message.event.current_turn_id ?? null,
          // A live waiting state has no question payload. Reuse the existing
          // no-cursor watch path to obtain its authoritative snapshot.
          resyncNeeded:
            current.resyncNeeded || message.event.waiting_question,
          projection,
        };
      });
    }

    case "session_metadata_changed": {
      const view = state.sessions[message.session_id];
      if (!view || view.streamId === null) return state;
      if (message.stream_id !== view.streamId) {
        return updateSession(state, message.session_id, (current) => ({
          ...current,
          resyncNeeded: true,
        }));
      }
      if (message.sequence <= view.cursor) return state;
      if (message.sequence > view.cursor + 1) {
        return updateSession(state, message.session_id, (current) => ({
          ...current,
          resyncNeeded: true,
        }));
      }
      return updateSession(state, message.session_id, (current) => ({
        ...current,
        cursor: message.sequence,
        metadata: message.event,
      }));
    }
  }
}

export function appReducer(state: AppState, action: AppAction): AppState {
  switch (action.type) {
    case "auth_result":
      return { ...state, auth: action.ok ? "ok" : "failed" };

    case "bootstrap_loaded":
      return {
        ...state,
        bootstrap: action.bootstrap,
        summaries: action.bootstrap.sessions ?? [],
      };

    case "connection_changed":
      return { ...state, connection: action.connection };

    case "server_message":
      return applySessionMessage(state, action.message);

    case "select_session": {
      const previousId = state.selectedSessionId;
      if (previousId === action.sessionId) {
        return { ...state, activeContextMenu: null };
      }
      let next: AppState = {
        ...state,
        selectedSessionId: action.sessionId,
        activeContextMenu: null,
        sidebarDrawerOpen: false,
        // Session switching changes only the observed UI. It closes the
        // secondary workspace without touching the background turn.
        informationPanel: {
          ...state.informationPanel,
          open: false,
          tab: "subagents",
          selectedAgent: null,
          review: null,
        },
      };
      // The previous session keeps its draft/expansion/scroll anchor but
      // drops the full transcript — full transcripts live only on the
      // current session.
      if (previousId !== null && next.sessions[previousId]) {
        next = updateSession(next, previousId, dropTranscript);
      }
      if (action.sessionId !== null && !next.sessions[action.sessionId]) {
        next = updateSession(next, action.sessionId, (view) => view);
      }
      return next;
    }

    case "select_workspace":
      return {
        ...state,
        selectedWorkspaceId: action.workspaceId,
        selectedSessionId: null,
        activeContextMenu: null,
        sidebarDrawerOpen: false,
      };

    case "workspace_added": {
      const exists = state.workspaces.some((group) => group.id === action.workspace.id);
      return {
        ...state,
        workspaces: exists ? state.workspaces : [...state.workspaces, action.workspace],
        selectedWorkspaceId: action.workspace.id,
        selectedSessionId: null,
      };
    }

    case "set_sidebar_width":
      return { ...state, sidebarWidth: clampSidebar(action.width) };

    case "set_drawer_open":
      return { ...state, sidebarDrawerOpen: action.open };

    case "set_sidebar_collapsed":
      return { ...state, sidebarCollapsed: action.collapsed };

    case "theme_changed":
      return { ...state, theme: action.theme };

    case "set_context_menu":
      return { ...state, activeContextMenu: action.sessionId };

    case "draft_changed":
      return updateSession(state, action.sessionId, (view) => ({
        ...view,
        draft: action.text,
      }));

    case "set_line_expanded":
      return updateSession(state, action.sessionId, (view) => ({
        ...view,
        lineOverrides: { ...view.lineOverrides, [action.itemId]: action.expanded },
      }));

    case "set_at_bottom":
      return updateSession(state, action.sessionId, (view) => ({
        ...view,
        isAtBottom: action.atBottom,
      }));

    case "session_started": {
      // The response sequence is NOT a transcript watermark. Select the
      // session and let the snapshot arrive before any event is applied.
      const sessionId = action.started.session_id;
      let next: AppState = { ...state, creatingSession: false };
      next = next.selectedSessionId === sessionId
        ? next
        : appReducer(next, { type: "select_session", sessionId });
      next = updateSession(next, sessionId, (view) => ({
        ...view,
        phase: action.started.state.phase,
        waitingApproval: action.started.state.waiting_approval,
        waitingQuestion: action.started.state.waiting_question,
        currentTurnId: action.started.state.current_turn_id ?? action.started.turn_id,
        draft: "",
        sending: false,
      }));
      return next;
    }

    case "create_started":
      return { ...state, creatingSession: true };

    case "create_finished":
      return { ...state, creatingSession: false };

    case "send_started":
      return updateSession(state, action.sessionId, (view) => ({
        ...view,
        sending: true,
      }));

    case "send_finished":
      return updateSession(state, action.sessionId, (view) => ({
        ...view,
        sending: false,
      }));

    case "approval_submitted":
      return updateSession(state, action.sessionId, (view) => ({
        ...view,
        submittedApprovalIds: [...view.submittedApprovalIds, action.requestId],
      }));

    case "approval_stale":
      // Stale response: restore the editable state; the snapshot refresh
      // (issued by the caller) is the final arbiter.
      return updateSession(state, action.sessionId, (view) => ({
        ...view,
        submittedApprovalIds: view.submittedApprovalIds.filter(
          (id) => id !== action.requestId,
        ),
      }));

    case "question_submitted":
      return updateSession(state, action.sessionId, (view) => ({
        ...view,
        submittedQuestionIds: [...view.submittedQuestionIds, action.questionId],
      }));

    case "question_stale":
      return updateSession(state, action.sessionId, (view) => ({
        ...view,
        submittedQuestionIds: view.submittedQuestionIds.filter(
          (id) => id !== action.questionId,
        ),
      }));

    case "notice":
      return { ...state, notice: action.text };

    case "clear_notice":
      return { ...state, notice: null };

    case "auto_open_information_panel": {
      if (state.informationPanel.autoOpenedSessionIds.includes(action.sessionId)) {
        return state;
      }
      return {
        ...state,
        informationPanel: {
          ...state.informationPanel,
          open: !state.informationPanel.fixedSummaryOpen,
          tab: state.informationPanel.open ? state.informationPanel.tab : "subagents",
          autoOpenedSessionIds: [
            ...state.informationPanel.autoOpenedSessionIds,
            action.sessionId,
          ],
        },
      };
    }

    case "open_information_panel":
      return {
        ...state,
        informationPanel: {
          ...state.informationPanel,
          open: true,
          fixedSummaryOpen: false,
          tab: action.tab,
          focusNonce: state.informationPanel.focusNonce + 1,
        },
      };

    case "close_information_panel":
      return {
        ...state,
        informationPanel: {
          ...state.informationPanel,
          open: false,
          selectedAgent: null,
          review: null,
        },
      };

    case "set_fixed_summary_open":
      return {
        ...state,
        informationPanel: {
          ...state.informationPanel,
          fixedSummaryOpen: action.open,
          open: action.open ? false : state.informationPanel.open,
          selectedAgent: action.open ? null : state.informationPanel.selectedAgent,
        },
      };

    case "select_information_agent":
      return {
        ...state,
        informationPanel: {
          ...state.informationPanel,
          open: true,
          fixedSummaryOpen: false,
          tab: "subagents",
          selectedAgent: action.agent,
          focusNonce: state.informationPanel.focusNonce + 1,
        },
      };

    case "set_information_panel_tab":
      return {
        ...state,
        informationPanel: { ...state.informationPanel, tab: action.tab },
      };

    case "open_review":
      return {
        ...state,
        informationPanel: {
          ...state.informationPanel,
          open: true,
          fixedSummaryOpen: false,
          tab: "review",
          review: action.target,
          focusNonce: state.informationPanel.focusNonce + 1,
        },
      };
  }
}
