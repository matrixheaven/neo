/**
 * React binding: owns the one long connection (two subscriptions), the
 * auth/bootstrap flow, reconnect with bounded backoff, and all API actions
 * with typed error handling (401 / 409 / 413 / network).
 */

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useReducer,
  useRef,
  type ReactNode,
} from "react";
import {
  ApiError,
  cancelTurn,
  claimAccessToken,
  createSession,
  fetchBootstrap,
  fetchSnapshot,
  openEventsSocket,
  resolveApproval,
  resolveQuestion,
  sendInput,
  startTurn,
  updateMetadata,
  type EventsSocket,
} from "../api";
import type {
  ApprovalAction,
  WebUiComposer,
  WebUiQuestionAnswer,
  WebUiServerMessage,
} from "../protocol";
import {
  SIDEBAR_DEFAULT,
  initialAppState,
  type AppState,
} from "./appState";
import { appReducer } from "./reducer";

const SIDEBAR_WIDTH_KEY = "neo-webui.sidebar-width";

function loadSidebarWidth(): number {
  try {
    const raw = window.localStorage.getItem(SIDEBAR_WIDTH_KEY);
    if (raw === null) return SIDEBAR_DEFAULT;
    const value = Number.parseInt(raw, 10);
    return Number.isFinite(value) ? value : SIDEBAR_DEFAULT;
  } catch {
    return SIDEBAR_DEFAULT;
  }
}

function saveSidebarWidth(width: number): void {
  try {
    window.localStorage.setItem(SIDEBAR_WIDTH_KEY, String(width));
  } catch {
    // Local preference only; storage may be unavailable.
  }
}

export interface AppActions {
  selectSession(sessionId: string | null): void;
  setSidebarWidth(width: number): void;
  setDrawerOpen(open: boolean): void;
  setContextMenu(sessionId: string | null): void;
  setDraft(sessionId: string, text: string): void;
  toggleItemExpanded(sessionId: string, itemId: string): void;
  setAtBottom(sessionId: string, atBottom: boolean): void;
  sendMessage(text: string, composer?: WebUiComposer): void;
  steer(text: string): void;
  stop(): void;
  submitApproval(requestId: string, action: ApprovalAction, feedback?: string): void;
  submitQuestion(questionId: string, answer: WebUiQuestionAnswer): void;
  patchMetadata(sessionId: string, change: { title?: string; pinned?: boolean; archived?: boolean }): void;
  dismissNotice(): void;
}

const AppStateContext = createContext<AppState | null>(null);
const AppActionsContext = createContext<AppActions | null>(null);

const RECONNECT_DELAYS_MS = [500, 1000, 2000, 4000, 8000];

export function AppProvider({ children }: { children: ReactNode }) {
  const [state, dispatch] = useReducer(appReducer, undefined, () =>
    initialAppState(loadSidebarWidth()),
  );
  const stateRef = useRef(state);
  stateRef.current = state;
  const socketRef = useRef<EventsSocket | null>(null);
  const reconnectAttemptRef = useRef(0);
  const reconnectTimerRef = useRef<number | null>(null);
  const disposedRef = useRef(false);
  // Connection generation: bumped on every (re)connect. Watch requests are
  // deduplicated per generation so effects never re-send the same
  // subscription.
  const connectionGenRef = useRef(0);
  const freshWatchRef = useRef<{ sessionId: string; gen: number } | null>(null);
  const resyncWatchRef = useRef<{ sessionId: string; gen: number; key: string } | null>(null);

  // -- Auth + bootstrap ------------------------------------------------------
  useEffect(() => {
    disposedRef.current = false;
    let cancelled = false;
    (async () => {
      const hasToken = /(?:^|#|&)access=([^&]+)/.test(window.location.hash);
      if (hasToken) {
        const claimed = await claimAccessToken();
        if (!claimed) {
          if (!cancelled) dispatch({ type: "auth_result", ok: false });
          return;
        }
      }
      try {
        const bootstrap = await fetchBootstrap();
        if (cancelled) return;
        dispatch({ type: "auth_result", ok: true });
        dispatch({ type: "bootstrap_loaded", bootstrap });
      } catch {
        if (cancelled) return;
        dispatch({ type: "auth_result", ok: false });
      }
    })();
    return () => {
      cancelled = true;
      disposedRef.current = true;
    };
  }, []);

  // -- Long connection --------------------------------------------------------
  useEffect(() => {
    if (state.auth !== "ok") return;

    const connect = () => {
      if (disposedRef.current) return;
      connectionGenRef.current += 1;
      dispatch({ type: "connection_changed", connection: reconnectAttemptRef.current === 0 ? "connecting" : "reconnecting" });
      const socket = openEventsSocket({
        onMessage(message: WebUiServerMessage) {
          dispatch({ type: "server_message", message });
        },
        onClose() {
          socketRef.current = null;
          if (disposedRef.current) return;
          // Any close (including 1013 slow-consumer) reconnects without a
          // cursor: both subscriptions restart from fresh snapshots.
          const attempt = Math.min(reconnectAttemptRef.current, RECONNECT_DELAYS_MS.length - 1);
          const delay = RECONNECT_DELAYS_MS[attempt];
          reconnectAttemptRef.current += 1;
          dispatch({ type: "connection_changed", connection: "reconnecting" });
          reconnectTimerRef.current = window.setTimeout(connect, delay);
        },
        onOpen() {
          // The backoff counter resets only once the connection is actually
          // established — while the service is unreachable the delays keep
          // escalating.
          reconnectAttemptRef.current = 0;
          dispatch({ type: "connection_changed", connection: "open" });
          // Fresh subscriptions without cursors after (re)connect.
          socket.send({ type: "watch_workspace" });
          const selected = stateRef.current.selectedSessionId;
          if (selected !== null) {
            socket.send({ type: "watch_session", session_id: selected });
            freshWatchRef.current = {
              sessionId: selected,
              gen: connectionGenRef.current,
            };
          }
        },
      });
      socketRef.current = socket;
    };

    connect();
    return () => {
      if (reconnectTimerRef.current !== null) {
        window.clearTimeout(reconnectTimerRef.current);
        reconnectTimerRef.current = null;
      }
      socketRef.current?.close();
      socketRef.current = null;
    };
  }, [state.auth]);

  // -- Session switching replaces only the full-session subscription ---------
  useEffect(() => {
    if (state.auth !== "ok" || state.connection !== "open") return;
    const selected = state.selectedSessionId;
    if (selected === null) return;
    const view = state.sessions[selected];
    const gen = connectionGenRef.current;
    if (view && !view.resyncNeeded && resyncWatchRef.current?.sessionId === selected) {
      // Snapshot resolved the resync: the next failure is a new generation.
      resyncWatchRef.current = null;
    }
    if (!view || view.streamId === null) {
      // Fresh subscription for this connection generation: send once.
      const sent = freshWatchRef.current;
      if (sent && sent.sessionId === selected && sent.gen === gen) return;
      socketRef.current?.send({ type: "watch_session", session_id: selected });
      freshWatchRef.current = { sessionId: selected, gen };
      return;
    }
    if (view.resyncNeeded) {
      // Cursor cannot resume: re-subscribe without a cursor, once per
      // distinct (stream, cursor) failure marker.
      const key = `${view.streamId ?? ""}:${view.cursor}`;
      const sent = resyncWatchRef.current;
      if (sent && sent.sessionId === selected && sent.gen === gen && sent.key === key) return;
      socketRef.current?.send({ type: "watch_session", session_id: selected });
      resyncWatchRef.current = { sessionId: selected, gen, key };
    }
  }, [state.auth, state.connection, state.selectedSessionId, state.sessions]);

  // -- Error mapping ----------------------------------------------------------
  const handleApiError = useCallback((error: unknown, sessionId?: string) => {
    if (error instanceof ApiError) {
      if (error.status === 401) {
        dispatch({ type: "auth_result", ok: false });
        return;
      }
      if (error.status === 409) {
        // Keep the draft, refresh the session: the snapshot is authoritative.
        dispatch({ type: "notice", text: "会话状态已变化，已刷新最新状态。" });
        if (sessionId) {
          fetchSnapshot(sessionId)
            .then((snapshot) =>
              dispatch({
                type: "server_message",
                message: { type: "session_snapshot", snapshot },
              }),
            )
            .catch(() => {});
        }
        return;
      }
      if (error.status === 413) {
        dispatch({ type: "notice", text: "输入内容过大，请缩短后再发送。" });
        return;
      }
    }
    dispatch({ type: "notice", text: "网络请求失败，请稍后重试。" });
  }, []);

  // -- Actions ----------------------------------------------------------------
  const actions = useMemo<AppActions>(() => ({
    selectSession(sessionId) {
      dispatch({ type: "select_session", sessionId });
    },
    setSidebarWidth(width) {
      saveSidebarWidth(width);
      dispatch({ type: "set_sidebar_width", width });
    },
    setDrawerOpen(open) {
      dispatch({ type: "set_drawer_open", open });
    },
    setContextMenu(sessionId) {
      dispatch({ type: "set_context_menu", sessionId });
    },
    setDraft(sessionId, text) {
      dispatch({ type: "draft_changed", sessionId, text });
    },
    toggleItemExpanded(sessionId, itemId) {
      dispatch({ type: "toggle_item_expanded", sessionId, itemId });
    },
    setAtBottom(sessionId, atBottom) {
      dispatch({ type: "set_at_bottom", sessionId, atBottom });
    },

    sendMessage(text, composer) {
      const current = stateRef.current;
      const trimmed = text.trim();
      if (trimmed === "") return;
      const selected = current.selectedSessionId;
      if (selected === null) {
        // Double-submit guard: one in-flight create at a time. The draft is
        // cleared only after the create succeeds (the composer clears its
        // local draft when the new session becomes selected); 409/413 and
        // network failures keep it.
        if (current.creatingSession) return;
        dispatch({ type: "create_started" });
        createSession(trimmed, composer)
          .then((started) => {
            dispatch({ type: "session_started", started });
          })
          .catch((error: unknown) => {
            dispatch({ type: "create_finished" });
            handleApiError(error);
          });
        return;
      }
      const view = current.sessions[selected];
      const running =
        view &&
        (view.phase === "running" || view.phase === "starting" || view.phase === "finishing") &&
        view.currentTurnId;
      dispatch({ type: "send_started", sessionId: selected });
      const clearDraft = () =>
        dispatch({ type: "draft_changed", sessionId: selected, text: "" });
      if (running && view) {
        sendInput(selected, view.currentTurnId as string, "follow_up", trimmed)
          .then(() => {
            dispatch({ type: "send_finished", sessionId: selected });
            clearDraft();
          })
          .catch((error: unknown) => {
            dispatch({ type: "send_finished", sessionId: selected });
            handleApiError(error, selected);
          });
      } else {
        startTurn(selected, trimmed, composer)
          .then((started) => {
            dispatch({ type: "session_started", started });
          })
          .catch((error: unknown) => {
            dispatch({ type: "send_finished", sessionId: selected });
            handleApiError(error, selected);
          });
      }
    },

    steer(text) {
      const current = stateRef.current;
      const trimmed = text.trim();
      const selected = current.selectedSessionId;
      if (selected === null || trimmed === "") return;
      const view = current.sessions[selected];
      if (!view || !view.currentTurnId) return;
      dispatch({ type: "send_started", sessionId: selected });
      sendInput(selected, view.currentTurnId, "steer", trimmed)
        .then(() => {
          dispatch({ type: "send_finished", sessionId: selected });
          dispatch({ type: "draft_changed", sessionId: selected, text: "" });
        })
        .catch((error: unknown) => {
          dispatch({ type: "send_finished", sessionId: selected });
          handleApiError(error, selected);
        });
    },

    stop() {
      const current = stateRef.current;
      const selected = current.selectedSessionId;
      if (selected === null) return;
      const view = current.sessions[selected];
      if (!view || !view.currentTurnId) return;
      cancelTurn(selected, view.currentTurnId).catch((error: unknown) =>
        handleApiError(error, selected),
      );
    },

    submitApproval(requestId, action, feedback) {
      const current = stateRef.current;
      const selected = current.selectedSessionId;
      if (selected === null) return;
      const view = current.sessions[selected];
      const turnId = view?.currentTurnId;
      if (!turnId) return;
      dispatch({ type: "approval_submitted", sessionId: selected, requestId });
      resolveApproval(selected, turnId, requestId, action, feedback)
        .then(() => {})
        .catch((error: unknown) => {
          if (error instanceof ApiError && error.status === 409) {
            dispatch({ type: "approval_stale", sessionId: selected, requestId });
          }
          handleApiError(error, selected);
        });
    },

    submitQuestion(questionId, answer) {
      const current = stateRef.current;
      const selected = current.selectedSessionId;
      if (selected === null) return;
      const view = current.sessions[selected];
      const turnId = view?.currentTurnId;
      if (!turnId) return;
      dispatch({ type: "question_submitted", sessionId: selected, questionId });
      resolveQuestion(selected, turnId, questionId, answer)
        .then(() => {})
        .catch((error: unknown) => {
          if (error instanceof ApiError && error.status === 409) {
            dispatch({ type: "question_stale", sessionId: selected, questionId });
          }
          handleApiError(error, selected);
        });
    },

    patchMetadata(sessionId, change) {
      updateMetadata(sessionId, change).catch((error: unknown) =>
        handleApiError(error, sessionId),
      );
    },

    dismissNotice() {
      dispatch({ type: "clear_notice" });
    },
  }), [handleApiError]);

  return (
    <AppStateContext.Provider value={state}>
      <AppActionsContext.Provider value={actions}>
        {children}
      </AppActionsContext.Provider>
    </AppStateContext.Provider>
  );
}

export function useAppState(): AppState {
  const state = useContext(AppStateContext);
  if (!state) throw new Error("AppProvider missing");
  return state;
}

export function useAppActions(): AppActions {
  const actions = useContext(AppActionsContext);
  if (!actions) throw new Error("AppProvider missing");
  return actions;
}
