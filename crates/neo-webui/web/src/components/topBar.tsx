/**
 * Fixed 48px top bar. Left: workspace label, current session summary and run
 * state. Right: change entry and branch status — only when the backend
 * provides them (it does not yet, so nothing renders there). Never models,
 * permission modes or development modes.
 */

import { PanelLeft } from "lucide-react";
import { useAppActions, useAppState } from "../state/store";
import type { WebUiPhase } from "../protocol";

function phaseText(phase: WebUiPhase | null): string {
  switch (phase) {
    case "starting":
      return "正在启动";
    case "running":
      return "运行中";
    case "finishing":
      return "正在收尾";
    case "idle":
      return "空闲";
    case "cancelled":
      return "已取消";
    case "failed":
      return "失败";
    default:
      return "";
  }
}

export function TopBar() {
  const state = useAppState();
  const actions = useAppActions();
  const sessionId = state.selectedSessionId;
  const view = sessionId !== null ? state.sessions[sessionId] : undefined;
  const summary =
    sessionId !== null
      ? state.summaries.find((entry) => entry.session_id === sessionId)
      : undefined;
  const title = view?.metadata?.title ?? summary?.title ?? null;
  const running =
    view && (view.phase === "running" || view.phase === "starting" || view.phase === "finishing");

  return (
    <header className="topbar">
      <button
        type="button"
        className="icon-button drawer-toggle"
        aria-label="打开会话列表"
        aria-expanded={state.sidebarDrawerOpen}
        onClick={() => actions.setDrawerOpen(!state.sidebarDrawerOpen)}
      >
        <PanelLeft size={16} aria-hidden />
      </button>
      <span className="topbar-workspace">{state.bootstrap?.workspace_label ?? "Neo"}</span>
      {title !== null ? (
        <>
          <span className="topbar-separator" aria-hidden>
            /
          </span>
          <span className="topbar-session" title={title}>
            {title}
          </span>
          {running || (view && view.phase) ? (
            <span
              className={`topbar-state ${running ? "state-running" : ""}`}
              role="status"
            >
              {phaseText(view?.phase ?? null)}
              {view?.waitingApproval ? " · 等待确认" : ""}
              {view?.waitingQuestion ? " · 等待回答" : ""}
            </span>
          ) : null}
        </>
      ) : null}
      <span className="topbar-spacer" />
      {/* Change entry and branch status render only when the backend provides
          structured workspace change data; until then this area stays empty. */}
    </header>
  );
}
