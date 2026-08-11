/**
 * Fixed 48px top bar. Left: workspace label, current session summary and run
 * state. Right: change entry and branch status — only when the backend
 * provides them (it does not yet, so nothing renders there). Never models,
 * permission modes or development modes.
 */

import { Moon, PanelLeft, PanelRight, Sun } from "lucide-react";
import { useEffect, useState } from "react";
import { useAppActions, useAppState } from "../state/store";
import type { WebUiPhase } from "../protocol";

const DRAWER_MEDIA_QUERY = "(max-width: 980px)";

function isDrawerViewport(): boolean {
  return typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia(DRAWER_MEDIA_QUERY).matches;
}

function useDrawerViewport(): boolean {
  const [matches, setMatches] = useState(isDrawerViewport);

  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const media = window.matchMedia(DRAWER_MEDIA_QUERY);
    const update = () => setMatches(media.matches);
    update();
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);

  return matches;
}

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
  const drawerViewport = useDrawerViewport();
  const sidebarOpen = drawerViewport ? state.sidebarDrawerOpen : !state.sidebarCollapsed;
  const sidebarLabel = drawerViewport
    ? state.sidebarDrawerOpen
      ? "关闭会话列表"
      : "打开会话列表"
    : state.sidebarCollapsed
      ? "展开会话列表"
      : "收起会话列表";

  return (
    <header className="topbar">
      <button
        type="button"
        className="icon-button drawer-toggle"
        aria-controls="session-sidebar"
        aria-label={sidebarLabel}
        aria-expanded={sidebarOpen}
        onClick={() => {
          if (drawerViewport) {
            actions.setDrawerOpen(!state.sidebarDrawerOpen);
          } else {
            actions.setSidebarCollapsed(!state.sidebarCollapsed);
          }
        }}
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
      <button
        type="button"
        className="icon-button information-toggle fixed-summary-toggle"
        aria-controls={drawerViewport ? "information-panel" : "fixed-summary"}
        aria-label="切换固定摘要"
        aria-expanded={drawerViewport
          ? state.informationPanel.open
          : state.informationPanel.fixedSummaryOpen}
        title="切换固定摘要"
        onClick={() => {
          if (drawerViewport) {
            if (state.informationPanel.open) actions.closeInformationPanel();
            else actions.openInformationPanel("subagents");
          } else {
            actions.setFixedSummaryOpen(!state.informationPanel.fixedSummaryOpen);
          }
        }}
      >
        <PanelRight size={16} aria-hidden />
      </button>
      <button
        type="button"
        className="icon-button theme-toggle"
        aria-label="切换主题"
        title="切换主题"
        onClick={() => actions.setTheme(state.theme === "dark" ? "light" : "dark")}
      >
        {state.theme === "dark" ? <Sun size={16} aria-hidden /> : <Moon size={16} aria-hidden />}
      </button>
    </header>
  );
}
