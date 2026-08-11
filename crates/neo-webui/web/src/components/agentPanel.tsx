import {
  ArrowLeft,
  Bot,
  Check,
  Clock,
  Files,
  Loader2,
  X,
  XCircle,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { ApiError, fetchAgentHistory } from "../api";
import type { AgentSnapshot, WebUiPhase } from "../protocol";
import { useAppActions, useAppState } from "../state/store";
import type { TranscriptItem } from "../state/transcript";
import { buildFromHistory } from "../state/transcript";
import { reviewFilesForMessage, TranscriptDocument } from "./transcript";
import { AgentStatePill, agentStateText, formatElapsed } from "./transcriptItems";
import { ReviewPanel, type ReviewSourceState } from "./reviewPanel";

type PanelResult =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ok"; agentId: string; items: TranscriptItem[] }
  | { status: "error"; agentId: string; notFound: boolean };

const TERMINAL_AGENT_STATES = new Set([
  "completed",
  "failed",
  "cancelled",
  "aborted",
  "timed_out",
]);

/** Read-only projection: expansion ids cannot collide with the main
 * transcript, and unresolved controls are never actionable in a snapshot. */
function readonlyItems(items: TranscriptItem[], agentId: string): TranscriptItem[] {
  const prefix = `information-agent:${agentId}:`;
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

export function agentsFromTranscript(items: TranscriptItem[]): AgentSnapshot[] {
  const agents = new Map<string, AgentSnapshot>();
  for (const item of items) {
    if (item.kind === "delegate") {
      agents.set(item.agent.id, item.agent);
    } else if (item.kind === "swarm") {
      for (const child of item.swarm.children) {
        agents.set(child.agent.id, child.agent);
      }
    }
  }
  return [...agents.values()];
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

function AgentProgressIcon({ state }: { state: string }) {
  if (state === "running") return <Loader2 size={14} className="spin" aria-hidden />;
  if (state === "completed") return <Check size={14} aria-hidden />;
  if (TERMINAL_AGENT_STATES.has(state)) return <XCircle size={14} aria-hidden />;
  return <Clock size={14} aria-hidden />;
}

function AgentRosterRow({ sessionId, agent }: { sessionId: string; agent: AgentSnapshot }) {
  const actions = useAppActions();
  const title = agent.task_title ?? agent.display_name;
  const done = TERMINAL_AGENT_STATES.has(agent.state);
  const progressText = agent.latest_text?.trim() || (
    agent.tool_count !== undefined && agent.tool_count !== null
      ? `已使用 ${agent.tool_count} 个工具`
      : agentStateText(agent.state)
  );
  return (
    <button
      type="button"
      className={`information-agent-row state-${agent.state}`}
      aria-label={`查看子代理：${title}，状态：${agentStateText(agent.state)}`}
      onClick={() => actions.openAgentPanel(sessionId, agent)}
    >
      <span className="information-agent-icon"><AgentProgressIcon state={agent.state} /></span>
      <span className="information-agent-copy">
        <strong>{title}</strong>
        <small>{progressText}</small>
        <span
          className={`information-agent-progress${done ? " done" : " active"}`}
          role="progressbar"
          aria-label={`${title} 的进度`}
          aria-valuemin={0}
          aria-valuemax={100}
          {...(done ? { "aria-valuenow": 100 } : {})}
        >
          <span />
        </span>
      </span>
      <span className="information-agent-state">{agentStateText(agent.state)}</span>
    </button>
  );
}

function AgentRoster({ sessionId, agents }: { sessionId: string; agents: AgentSnapshot[] }) {
  const active = agents.filter((agent) => !TERMINAL_AGENT_STATES.has(agent.state));
  const done = agents.filter((agent) => TERMINAL_AGENT_STATES.has(agent.state));
  if (agents.length === 0) {
    return (
      <p className="information-empty">
        当前会话还没有代理活动。代理开始工作后会在这里显示。
      </p>
    );
  }
  return (
    <div className="information-roster">
      <section aria-labelledby="information-active-heading">
        <h3 id="information-active-heading">Active <span>{active.length}</span></h3>
        {active.length > 0 ? active.map((agent) => (
          <AgentRosterRow key={agent.id} sessionId={sessionId} agent={agent} />
        )) : <p className="information-section-empty">没有正在运行的代理</p>}
      </section>
      <section aria-labelledby="information-done-heading">
        <h3 id="information-done-heading">Done <span>{done.length}</span></h3>
        {done.length > 0 ? done.map((agent) => (
          <AgentRosterRow key={agent.id} sessionId={sessionId} agent={agent} />
        )) : <p className="information-section-empty">还没有已结束的代理</p>}
      </section>
    </div>
  );
}

function AgentDetail({
  sessionId,
  agent,
  result,
}: {
  sessionId: string;
  agent: AgentSnapshot;
  result: PanelResult;
}) {
  const actions = useAppActions();
  const title = agent.task_title ?? agent.display_name;
  const elapsed = formatElapsed(agent.elapsed?.secs);
  const tokens = tokenParts(agent);
  const snapshotText = agent.latest_text?.trim();
  return (
    <div className="information-agent-detail">
      <header className="information-agent-detail-head">
        <button
          type="button"
          className="icon-button"
          aria-label="返回子代理列表"
          title="返回子代理列表"
          onClick={() => actions.showAgentList()}
        >
          <ArrowLeft size={15} aria-hidden />
        </button>
        <h2>{title}</h2>
        <AgentStatePill state={agent.state} />
      </header>
      <div className="information-agent-meta">
        <span>状态：{agentStateText(agent.state)}</span>
        {elapsed !== null ? <span>累计耗时 {elapsed}</span> : null}
        {tokens.map((part) => <span key={part}>{part}</span>)}
      </div>
      {agent.state === "running" ? (
        <p className="information-snapshot-note" role="note">
          运行中仅显示上次落盘快照；进度状态仍来自当前会话。
        </p>
      ) : null}
      <div className="information-transcript">
        {result.status === "loading" || result.status === "idle" ? (
          <div className="information-skeleton" role="status" aria-label="正在加载子代理历史">
            <span /><span /><span />
          </div>
        ) : result.status === "error" ? (
          <div className="information-agent-fallback" role="note">
            <p className="information-empty">
              {result.notFound
                ? "未找到该子代理的逐条历史；当前仅显示代理结果快照。"
                : "子代理历史加载失败；当前仅显示代理结果快照。"}
            </p>
            {snapshotText ? <p className="information-snapshot-result">{snapshotText}</p> : null}
          </div>
        ) : result.items.length === 0 ? (
          <p className="information-empty">该子代理暂无已落盘的历史内容。</p>
        ) : (
          <TranscriptDocument sessionId={sessionId} items={result.items} agentId={agent.id} />
        )}
      </div>
    </div>
  );
}

function summaryPhaseText(phase: WebUiPhase | null): string {
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
      return "等待会话状态";
  }
}

function latestAssistantMessageId(items: TranscriptItem[]): string | null {
  for (let index = items.length - 1; index >= 0; index -= 1) {
    const item = items[index];
    if (item.kind === "assistant_message" && item.finished) return item.id;
  }
  return null;
}

export function FixedSummary({ sessionId }: { sessionId: string }) {
  const state = useAppState();
  const actions = useAppActions();
  const panel = state.informationPanel;
  const view = state.sessions[sessionId];
  const items = view?.projection.items ?? [];
  const agents = useMemo(() => agentsFromTranscript(items), [items]);
  const latestMessageId = latestAssistantMessageId(items);
  const files = useMemo(
    () => latestMessageId ? reviewFilesForMessage(items, latestMessageId) : [],
    [items, latestMessageId],
  );
  const title = view?.metadata?.title ??
    state.summaries.find((entry) => entry.session_id === sessionId)?.title ??
    "当前会话";
  const activeAgents = agents.filter((agent) => !TERMINAL_AGENT_STATES.has(agent.state)).length;
  const open = panel.fixedSummaryOpen && state.selectedSessionId === sessionId;
  const close = () => {
    const trigger = typeof document === "undefined"
      ? null
      : document.querySelector<HTMLElement>('[aria-controls="fixed-summary"]');
    actions.setFixedSummaryOpen(false);
    trigger?.focus();
  };

  return (
    <aside
      id="fixed-summary"
      className={`fixed-summary${open ? " open" : ""}`}
      aria-label="固定摘要"
      aria-hidden={!open}
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.stopPropagation();
          close();
        }
      }}
    >
      <header className="fixed-summary-header">
        <div>
          <h2>固定摘要</h2>
          <p title={title}>{title}</p>
        </div>
        <button
          type="button"
          className="icon-button"
          aria-label="关闭固定摘要"
          title="关闭固定摘要"
          onClick={close}
        >
          <X size={15} aria-hidden />
        </button>
      </header>
      <div className="fixed-summary-body">
        <p className="fixed-summary-status" role="status">
          状态：{summaryPhaseText(view?.phase ?? null)}
        </p>
        <button
          type="button"
          className="fixed-summary-action"
          aria-label={`查看子代理，共 ${agents.length} 个`}
          onClick={() => actions.openInformationPanel("subagents")}
        >
          <Bot size={15} aria-hidden />
          <span>
            <strong>子代理</strong>
            <small>{activeAgents > 0 ? `正在运行 ${activeAgents} 个` : "当前没有运行中的子代理"}</small>
          </span>
          <b>{agents.length}</b>
        </button>
        {latestMessageId !== null && files.length > 0 ? (
          <button
            type="button"
            className="fixed-summary-action"
            aria-label={`查看 ${files.length} 个修改文件`}
            onClick={() => actions.openReview(sessionId, latestMessageId, null)}
          >
            <Files size={15} aria-hidden />
            <span>
              <strong>修改文件</strong>
              <small>查看最终修改文件列表</small>
            </span>
            <b>{files.length}</b>
          </button>
        ) : (
          <p className="fixed-summary-empty"><Files size={15} aria-hidden />暂无修改文件</p>
        )}
      </div>
    </aside>
  );
}

export function InformationPanel() {
  const state = useAppState();
  const actions = useAppActions();
  const panel = state.informationPanel;
  const sessionId = state.selectedSessionId;
  const items = sessionId ? state.sessions[sessionId]?.projection.items ?? [] : [];
  const agents = useMemo(() => agentsFromTranscript(items), [items]);
  const historyAgentId = panel.tab === "review"
    ? panel.review?.agentId ?? null
    : panel.selectedAgent?.id ?? null;
  const [result, setResult] = useState<PanelResult>({ status: "idle" });
  const [refreshKey, setRefreshKey] = useState(0);
  const panelRef = useRef<HTMLElement | null>(null);
  const focusTriggerRef = useRef<HTMLElement | null>(null);
  const previousOpenRef = useRef(false);
  const previousFocusNonceRef = useRef(panel.focusNonce);

  useEffect(() => {
    if (
      sessionId &&
      agents.length > 0 &&
      !panel.autoOpenedSessionIds.includes(sessionId)
    ) {
      actions.autoOpenInformationPanel(sessionId);
    }
  }, [actions, agents.length, panel.autoOpenedSessionIds, sessionId]);

  useEffect(() => {
    if (!sessionId || !historyAgentId) {
      setResult({ status: "idle" });
      return;
    }
    let cancelled = false;
    setResult({ status: "loading" });
    fetchAgentHistory(sessionId, historyAgentId)
      .then((history) => {
        if (cancelled) return;
        const projection = buildFromHistory(history.history, []);
        setResult({
          status: "ok",
          agentId: historyAgentId,
          items: readonlyItems(projection.items, historyAgentId),
        });
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        setResult({
          status: "error",
          agentId: historyAgentId,
          notFound: error instanceof ApiError && error.status === 404,
        });
      });
    return () => {
      cancelled = true;
    };
  }, [historyAgentId, refreshKey, sessionId]);

  useEffect(() => {
    const opened = panel.open;
    const userRequestedFocus = opened && panel.focusNonce !== previousFocusNonceRef.current;
    if (userRequestedFocus) {
      const active = document.activeElement;
      focusTriggerRef.current = active instanceof HTMLElement ? active : null;
      window.requestAnimationFrame(() => {
        if (!panelRef.current?.classList.contains("open")) return;
        panelRef.current.querySelector<HTMLElement>("[role=tab][aria-selected=true]")?.focus();
      });
    }
    if (!opened && previousOpenRef.current) {
      const trigger = focusTriggerRef.current;
      if (
        trigger &&
        trigger !== document.body &&
        trigger.isConnected &&
        !panelRef.current?.contains(trigger)
      ) trigger.focus();
      else document.querySelector<HTMLElement>(
        '[aria-controls="information-panel"], [aria-controls="fixed-summary"]',
      )?.focus();
      focusTriggerRef.current = null;
    }
    previousOpenRef.current = opened;
    previousFocusNonceRef.current = panel.focusNonce;
  }, [panel.focusNonce, panel.open]);

  const reviewItems = panel.review?.agentId === null
    ? items
    : result.status === "ok" && result.agentId === panel.review?.agentId
      ? result.items
      : null;
  const reviewSourceState: ReviewSourceState = panel.review?.agentId === null
    ? "ok"
    : result.status === "loading" || result.status === "idle"
      ? "loading"
      : result.status === "error"
        ? result.notFound ? "missing" : "error"
        : "ok";

  return (
    <aside
      id="information-panel"
      ref={panelRef}
      className={`information-panel${panel.open ? " open" : ""}`}
      aria-label="会话信息区"
      aria-hidden={!panel.open}
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.stopPropagation();
          actions.closeInformationPanel();
        }
      }}
    >
      <header className="information-panel-header">
        <div className="information-tabs" role="tablist" aria-label="会话信息">
          <button
            type="button"
            role="tab"
            aria-selected={panel.tab === "subagents"}
            aria-controls="information-subagents"
            onClick={() => actions.setInformationPanelTab("subagents")}
          >
            <Bot size={14} aria-hidden />Subagents
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={panel.tab === "review"}
            aria-controls="information-review"
            onClick={() => actions.setInformationPanelTab("review")}
          >
            <Files size={14} aria-hidden />Review
          </button>
        </div>
        <button
          type="button"
          className="icon-button information-close"
          aria-label="关闭会话信息区"
          title="关闭会话信息区"
          onClick={() => actions.closeInformationPanel()}
        >
          <X size={15} aria-hidden />
        </button>
      </header>
      <div
        id="information-subagents"
        role="tabpanel"
        hidden={panel.tab !== "subagents"}
        className="information-panel-body"
      >
        {sessionId === null ? (
          <p className="information-empty">选择一个会话后查看子代理。</p>
        ) : panel.selectedAgent ? (
          <AgentDetail sessionId={sessionId} agent={panel.selectedAgent} result={result} />
        ) : (
          <AgentRoster sessionId={sessionId} agents={agents} />
        )}
      </div>
      <div
        id="information-review"
        role="tabpanel"
        hidden={panel.tab !== "review"}
        className="information-panel-body review-panel-body"
      >
        <ReviewPanel
          target={panel.review}
          items={reviewItems}
          sourceState={reviewSourceState}
          refreshKey={refreshKey}
          onRefresh={() => setRefreshKey((value) => value + 1)}
        />
      </div>
    </aside>
  );
}
