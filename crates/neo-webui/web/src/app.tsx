/**
 * App shell: auth gate, top bar, sidebar + resizer, main chat canvas with
 * transcript, floating composer and task list. No marketing surfaces, no
 * bottom bar, no model/permission strip.
 */

import { NeoMark } from "./components/neoMark";
import { AgentPanelHost } from "./components/agentPanel";
import { Composer } from "./components/composer";
import { Sidebar } from "./components/sidebar";
import { SidebarResizer } from "./components/sidebarResizer";
import { TopBar } from "./components/topBar";
import { TranscriptPane } from "./components/transcript";
import { useAppActions, useAppState } from "./state/store";

function AccessFailed() {
  return (
    <div className="gate" role="alert">
      <NeoMark size={40} />
      <h1>无法打开 Neo 工作区</h1>
      <p>访问链接无效或已过期。请回到终端，重新运行 <code>neo webui</code> 获取新的访问地址。</p>
    </div>
  );
}

function Loading() {
  return (
    <div className="gate" role="status">
      <NeoMark size={40} />
      <p>正在连接工作区…</p>
    </div>
  );
}

function NewSessionView() {
  return (
    <div className="new-session-view">
      <NeoMark size={44} />
      <Composer centered />
    </div>
  );
}

function SessionView({ sessionId }: { sessionId: string }) {
  const state = useAppState();
  const view = state.sessions[sessionId];
  const hasUserMessage =
    view?.projection.items.some((item) => item.kind === "user_message") ?? false;
  if (!hasUserMessage) {
    return <NewSessionView />;
  }
  return (
    <div className="session-view">
      <TranscriptPane sessionId={sessionId} />
      <Composer centered={false} />
    </div>
  );
}

export function App() {
  const state = useAppState();
  const actions = useAppActions();

  if (state.auth === "pending") {
    return <Loading />;
  }
  if (state.auth === "failed") {
    return <AccessFailed />;
  }

  const sessionId = state.selectedSessionId;
  const selectedSession = sessionId === null ? undefined : state.sessions[sessionId];
  const connectionBannerState =
    state.connection === "reconnecting" &&
    selectedSession?.streamId === null &&
    !selectedSession.resyncNeeded
      ? "connecting"
      : state.connection;

  return (
    <div className="app-shell">
      <TopBar />
      <div className={`app-body ${state.sidebarCollapsed ? "sidebar-collapsed" : ""}`}>
        <Sidebar />
        <SidebarResizer />
        {state.sidebarDrawerOpen ? (
          <div
            className="drawer-scrim"
            aria-hidden
            onClick={() => actions.setDrawerOpen(false)}
          />
        ) : null}
        <main className="main-area">
          {connectionBannerState === "connecting" ? (
            <div className="connection-banner" role="status">
              正在连接…
            </div>
          ) : connectionBannerState === "reconnecting" ? (
            <div className="connection-banner" role="status">
              连接已断开，正在重连…
            </div>
          ) : null}
          {state.notice !== null ? (
            <div className="notice" role="status">
              <span>{state.notice}</span>
              <button type="button" className="notice-dismiss" onClick={() => actions.dismissNotice()}>
                知道了
              </button>
            </div>
          ) : null}
          {sessionId === null ? <NewSessionView /> : <SessionView sessionId={sessionId} />}
        </main>
      </div>
      <AgentPanelHost />
    </div>
  );
}
