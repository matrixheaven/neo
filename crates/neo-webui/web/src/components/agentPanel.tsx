/**
 * Child-agent drill-down panel (R4 §5.2): a fixed right-side overlay that
 * lazy-loads the agent's persisted wire history and renders it through the
 * same transcript projection and item components as the main pane — read
 * only (no composer, approvals/questions forced to a non-actionable state).
 * The fetched data lives in local component state only: closing the panel
 * discards it; it never enters the session transcript cache.
 */

import { X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError, fetchAgentHistory } from "../api";
import { useAppActions, useAppState } from "../state/store";
import type { AgentSnapshot } from "../protocol";
import type { TranscriptItem } from "../state/transcript";
import { buildFromHistory } from "../state/transcript";
import { AgentStatePill, TranscriptItemView, agentStateText, formatElapsed } from "./transcriptItems";

type PanelResult =
  | { status: "loading" }
  | { status: "ok"; items: TranscriptItem[] }
  | { status: "error"; notFound: boolean };

/** Read-only projection for the panel: item ids are namespaced so expansion
 * state never collides with the main transcript's per-line overrides, and
 * unresolved approvals/questions render as settled (non-actionable) rows. */
function readonlyItems(items: TranscriptItem[], agentId: string): TranscriptItem[] {
  const prefix = `agent-panel:${agentId}:`;
  return items.map((item) => {
    const id = prefix + item.id;
    if (item.kind === "approval" && item.resolution === undefined) {
      return { ...item, id, resolution: { kind: "no_longer_pending" } };
    }
    if (item.kind === "question" && !item.resolved) {
      return { ...item, id, resolved: true };
    }
    return { ...item, id };
  });
}

function tokenParts(agent: AgentSnapshot): string[] {
  const parts: string[] = [];
  if (agent.token_count !== undefined && agent.token_count !== null) {
    parts.push(`token ${agent.token_count}`);
  }
  if (agent.input_token_count !== undefined && agent.input_token_count !== null) {
    parts.push(`输入 ${agent.input_token_count}`);
  }
  if (agent.cache_read_token_count !== undefined && agent.cache_read_token_count !== null) {
    parts.push(`缓存读 ${agent.cache_read_token_count}`);
  }
  if (agent.cache_write_token_count !== undefined && agent.cache_write_token_count !== null) {
    parts.push(`缓存写 ${agent.cache_write_token_count}`);
  }
  return parts;
}

function AgentPanel({ sessionId, agentId, agent }: { sessionId: string; agentId: string; agent: AgentSnapshot }) {
  const actions = useAppActions();
  const panelRef = useRef<HTMLElement | null>(null);
  const [result, setResult] = useState<PanelResult>({ status: "loading" });

  // Lazy load on open; discarded when the panel closes or retargets.
  useEffect(() => {
    let cancelled = false;
    setResult({ status: "loading" });
    fetchAgentHistory(sessionId, agentId)
      .then((history) => {
        if (cancelled) return;
        const projection = buildFromHistory(history.history, []);
        setResult({ status: "ok", items: readonlyItems(projection.items, agentId) });
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        setResult({
          status: "error",
          notFound: error instanceof ApiError && error.status === 404,
        });
      });
    return () => {
      cancelled = true;
    };
  }, [sessionId, agentId]);

  // Close runs a reverse transition first: requestClose only marks the panel
  // `.closing`; the actual close (and unmount, focus restore, data discard)
  // fires when the slide-out animation ends on the panel element itself.
  // Esc, scrim click and the close button all go through requestClose; under
  // prefers-reduced-motion the global 0.01ms rule makes animationend fire
  // effectively instantly. Session switching bypasses the animation (the
  // reducer clears the panel state directly).
  const [closing, setClosing] = useState(false);
  const requestClose = useCallback(() => setClosing(true), []);

  // Focus handoff: capture the triggering agent-line, focus the panel so Esc
  // lands here, and give focus back to the trigger when the panel unmounts
  // (Esc, scrim click, close button or session switch). A stale trigger
  // (e.g. transcript dropped on session switch) is skipped.
  useEffect(() => {
    const trigger = document.activeElement;
    panelRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        requestClose();
        return;
      }
      // Focus trap: aria-modal is a hint only, so Tab/Shift+Tab cycle through
      // the panel's focusable controls and never escape to the scrim's back
      // side while the panel is open.
      if (event.key === "Tab") {
        const panel = panelRef.current;
        if (!panel) return;
        const focusables = panel.querySelectorAll<HTMLElement>(
          'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
        );
        if (focusables.length === 0) {
          event.preventDefault();
          panel.focus();
          return;
        }
        const first = focusables[0];
        const last = focusables[focusables.length - 1];
        const active = document.activeElement;
        const escaped = active === null || !panel.contains(active);
        if (event.shiftKey) {
          if (escaped || active === first) {
            event.preventDefault();
            last.focus();
          }
        } else if (escaped || active === last) {
          event.preventDefault();
          first.focus();
        }
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      if (trigger instanceof HTMLElement && trigger.isConnected) {
        trigger.focus();
      }
    };
  }, [actions, requestClose]);

  const title = agent.task_title ?? agent.display_name;
  const elapsed = formatElapsed(agent.elapsed?.secs);
  const tokens = tokenParts(agent);
  return (
    <div className={`ap-root${closing ? " closing" : ""}`}>
      <div className="ap-scrim" aria-hidden onClick={requestClose} />
      <aside
        className={`agent-panel${closing ? " closing" : ""}`}
        role="dialog"
        aria-modal="true"
        aria-label={`子代理详情：${title}`}
        ref={panelRef}
        tabIndex={-1}
        onAnimationEnd={(event) => {
          if (closing && event.target === event.currentTarget) {
            actions.closeAgentPanel();
          }
        }}
      >
        <header className="ap-head">
          <h2 className="ap-title">{title}</h2>
          <AgentStatePill state={agent.state} />
          <button
            type="button"
            className="ap-close"
            aria-label="关闭子代理详情"
            onClick={requestClose}
          >
            <X size={14} aria-hidden />
          </button>
        </header>
        <div className="ap-meta">
          <span>状态：{agentStateText(agent.state)}</span>
          {elapsed !== null ? <span>累计耗时 {elapsed}</span> : null}
          {tokens.map((part) => (
            <span key={part}>{part}</span>
          ))}
        </div>
        {agent.state === "running" ? (
          <p className="ap-note" role="note">
            运行中面板显示截至上次落盘点的内容
          </p>
        ) : null}
        <div className="ap-body">
          {result.status === "loading" ? (
            <div className="ap-skel-list" role="status" aria-label="正在加载子代理历史">
              <div className="ap-skel" />
              <div className="ap-skel" />
              <div className="ap-skel" />
            </div>
          ) : result.status === "error" ? (
            <p className="ap-error" role="alert">
              {result.notFound
                ? "未找到该子代理的历史记录（可能尚未落盘或已过期）。"
                : "子代理历史加载失败，请稍后重试。"}
            </p>
          ) : result.items.length === 0 ? (
            <p className="ap-empty">（该子代理暂无已落盘的历史内容）</p>
          ) : (
            result.items.map((item) => (
              <TranscriptItemView key={item.id} sessionId={sessionId} item={item} />
            ))
          )}
        </div>
      </aside>
    </div>
  );
}

export function AgentPanelHost() {
  const state = useAppState();
  const panel = state.agentPanel;
  if (panel === null) return null;
  return (
    <AgentPanel
      key={`${panel.sessionId}:${panel.agentId}`}
      sessionId={panel.sessionId}
      agentId={panel.agentId}
      agent={panel.agent}
    />
  );
}
