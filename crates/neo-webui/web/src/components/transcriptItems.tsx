/**
 * Transcript item renderers (redesign §4): typography is the hierarchy, not
 * cards. Only the user message is a bubble (.u-turn); thinking, tools,
 * shells, terminals, workflows, delegates and unknown records are single
 * 24px lines (.think / .tool-line / .agent-line) that expand in place;
 * approvals and questions are in-place rows with a 2px accent bar
 * (.approval-row). State is always conveyed in text, never color alone.
 */

import {
  Bot,
  Brain,
  Check,
  ChevronRight,
  CircleHelp,
  Clock,
  Loader2,
  Network,
  ShieldQuestion,
  SquareTerminal,
  Workflow,
  XCircle,
} from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import type { AgentSnapshot, ApprovalOption, QuestionEventData } from "../protocol";
import { useAppActions, useAppState } from "../state/store";
import type {
  ApprovalItem,
  AssistantMessageItem,
  DelegateItem,
  QuestionItem,
  RetryItem,
  ShellItem,
  StatusLineItem,
  SwarmItem,
  TerminalItem,
  ThinkingItem,
  ToolItem,
  ToolStatus,
  TranscriptItem,
  UnknownItem,
  UserMessageItem,
  WorkflowItem,
} from "../state/transcript";
import { CodeBlock, CopyButton, OutputBlock } from "./codeBlock";
import { Line, useLineExpanded } from "./collapsible";
import { FullOutput } from "./fullOutput";
import { Markdown } from "./markdown";

export function formatElapsed(secs: number | undefined): string | null {
  if (secs === undefined || !Number.isFinite(secs) || secs <= 0) return null;
  if (secs < 60) return `${secs}s`;
  const minutes = Math.floor(secs / 60);
  return `${minutes}m${secs % 60}s`;
}

function argumentsPreview(value: unknown): string {
  if (value === undefined || value === null) return "";
  try {
    const text = JSON.stringify(value);
    return text.length > 160 ? `${text.slice(0, 160)}…` : text;
  } catch {
    return "";
  }
}

function toolStatusText(item: ToolItem | ShellItem): string {
  switch (item.status) {
    case "queued": {
      const position = item.queuePosition;
      const waiting = item.queueWaitingMs;
      const parts = ["排队等待"];
      if (position !== undefined) parts.push(`位置 ${position}`);
      if (waiting !== undefined) parts.push(`已等待 ${waiting}ms`);
      return parts.join(" · ");
    }
    case "running":
      return "运行中";
    case "finished":
      return "已完成";
    case "failed":
      return "失败";
  }
}

function statusIcon(status: ToolStatus) {
  switch (status) {
    case "queued":
      return <Clock size={13} aria-hidden />;
    case "running":
      return <Loader2 size={13} className="spin" aria-hidden />;
    case "finished":
      return <Check size={13} aria-hidden />;
    case "failed":
      return <XCircle size={13} aria-hidden />;
  }
}

function lineCaret() {
  return (
    <span className="line-caret" aria-hidden>
      <ChevronRight size={13} />
    </span>
  );
}

// ---------------------------------------------------------------------------
// User turn: the only bubble. Long messages clamp to ~8 lines with a bottom
// gradient fade and an expand/collapse toggle.
// ---------------------------------------------------------------------------

const USER_COLLAPSE_LINES = 8;

function UserMessage({ item }: { item: UserMessageItem }) {
  const textRef = useRef<HTMLDivElement | null>(null);
  const [expanded, setExpanded] = useState(false);
  const [overflowing, setOverflowing] = useState(
    () => item.text.split("\n").length > USER_COLLAPSE_LINES,
  );
  // Measure real overflow for long single-paragraph text that wraps past the
  // clamp. Once overflowing it stays expandable (jsdom has no layout, so the
  // newline heuristic above is the deterministic path there).
  useLayoutEffect(() => {
    const element = textRef.current;
    if (element && !expanded && element.scrollHeight > element.clientHeight + 1) {
      setOverflowing(true);
    }
  }, [item.text, expanded]);
  const clamped = overflowing && !expanded;
  return (
    <div className="u-turn t-item">
      <div className="u-bub">
        <div className={`u-text-wrap ${clamped ? "is-clamped" : ""}`}>
          <div className="u-text" ref={textRef}>
            {item.text}
          </div>
          {overflowing ? (
            <button
              type="button"
              className="u-text-toggle"
              aria-expanded={expanded}
              onClick={() => setExpanded((current) => !current)}
            >
              {expanded ? "收起" : "展开"}
            </button>
          ) : null}
        </div>
      </div>
      <div className="u-meta">
        <CopyButton text={item.text} label="复制消息" />
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Assistant prose body (inside .a-msg, grouping lives in transcript.tsx).
// ---------------------------------------------------------------------------

export function AssistantBody({ item }: { item: AssistantMessageItem }) {
  return (
    <div className={`msg ${item.finished ? "" : "streaming"}`}>
      <Markdown text={item.text} />
      {!item.finished ? <span className="streaming-caret" aria-label="正在生成" /> : null}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Think: single line with breathing title while streaming; expanded by
// default during streaming, auto-collapses when the stream finishes.
// ---------------------------------------------------------------------------

function Think({ sessionId, item }: { sessionId: string; item: ThinkingItem }) {
  // Phase default: streaming opens, finished collapses. A user click during
  // streaming records an explicit override that the finish must not invert.
  const [open, toggle] = useLineExpanded(sessionId, item.id, !item.finished);
  const live = !item.finished;
  const startRef = useRef<number | null>(null);
  const [elapsedSecs, setElapsedSecs] = useState(0);
  useEffect(() => {
    if (!live) return;
    if (startRef.current === null) startRef.current = Date.now();
    const timer = window.setInterval(() => {
      const start = startRef.current;
      if (start !== null) {
        setElapsedSecs(Math.max(0, Math.round((Date.now() - start) / 1000)));
      }
    }, 1000);
    return () => window.clearInterval(timer);
  }, [live]);
  const status = item.finished
    ? item.redacted
      ? "已完成（已隐藏部分内容）"
      : "已完成"
    : `思考中 · ${elapsedSecs}s`;
  return (
    <Line
      className={`think ${live ? "live" : ""}`}
      label={`思考，状态：${status}`}
      open={open}
      onToggle={toggle}
      head={
        <>
          {lineCaret()}
          <Brain size={14} aria-hidden />
          <span className="think-title">思考</span>
          <span className="line-tail">{item.finished ? status : `${elapsedSecs}s`}</span>
        </>
      }
    >
      <pre className="think-text">
        <code>{item.text}</code>
      </pre>
    </Line>
  );
}

// ---------------------------------------------------------------------------
// Tool lines: status icon + name + dim mono summary + right status pill;
// the body carries the command echo, arguments, output and status metadata.
// ---------------------------------------------------------------------------

function stringArgument(args: unknown, fields: readonly string[]): string | null {
  if (!args || typeof args !== "object") return null;
  const record = args as Record<string, unknown>;
  for (const field of fields) {
    const value = record[field];
    if (typeof value === "string" && value !== "") return value;
  }
  return null;
}

function commandEchoOf(args: unknown): string | null {
  return stringArgument(args, ["command"]);
}

interface ToolPresentation {
  action: string;
  target: string;
}

function toolPresentation(name: string, args: unknown): ToolPresentation {
  switch (name.toLowerCase()) {
    case "grep":
    case "glob":
    case "find":
      return { action: "搜索", target: stringArgument(args, ["pattern", "query", "path"]) ?? "" };
    case "read":
    case "list":
      return { action: "读取", target: stringArgument(args, ["path"]) ?? "" };
    case "readmediafile":
      return { action: "观察", target: stringArgument(args, ["path"]) ?? "" };
    case "edit":
      return { action: "编辑", target: stringArgument(args, ["path"]) ?? "" };
    case "write":
      return { action: "创建", target: stringArgument(args, ["path"]) ?? "" };
    case "bash":
    case "shell":
      return { action: "运行", target: stringArgument(args, ["command"]) ?? "" };
    case "terminal":
      return { action: "启动终端", target: stringArgument(args, ["command", "handle"]) ?? "" };
    default:
      return { action: name, target: argumentsPreview(args) };
  }
}

function Tool({ sessionId, item }: { sessionId: string; item: ToolItem }) {
  const [open, toggle] = useLineExpanded(sessionId, item.id, false);
  const status = toolStatusText(item);
  const echo = commandEchoOf(item.arguments);
  const presentation = toolPresentation(item.name, item.arguments);
  const heading = presentation.target === ""
    ? presentation.action
    : `${presentation.action} ${presentation.target}`;
  return (
    <Line
      className={`tool-line status-${item.status}`}
      label={`${heading}，状态：${status}`}
      open={open}
      onToggle={toggle}
      head={
        <>
          {lineCaret()}
          <span className="tl-ic">{statusIcon(item.status)}</span>
          <span className="tl-name">{presentation.action}</span>
          <span className="tl-mono">{presentation.target}</span>
          <span className="line-tail">
            <span className="tl-status" role="status">
              {status}
            </span>
          </span>
        </>
      }
    >
      <div className="tl-detail">
        {echo !== null ? <div className="cmd-echo">$ {echo}</div> : null}
        {item.arguments !== undefined ? (
          <CodeBlock code={argumentsPreview(item.arguments)} language="参数" />
        ) : null}
        {item.partialResult && item.status === "running" ? (
          <OutputBlock text={item.partialResult.content} />
        ) : null}
        {item.result ? (
          <OutputBlock
            text={
              item.result.content || (item.result.is_error ? "（错误，无内容）" : "（无内容）")
            }
          />
        ) : null}
        <p className="tl-meta">
          状态：{status}
          {item.result?.is_error ? " · 结果标记为错误" : ""}
        </p>
        {item.output ? (
          <FullOutput sessionId={sessionId} itemId={item.id} outputRef={item.output} />
        ) : null}
      </div>
    </Line>
  );
}

function Shell({ sessionId, item }: { sessionId: string; item: ShellItem }) {
  const [open, toggle] = useLineExpanded(sessionId, item.id, false);
  const status = toolStatusText(item);
  const meta = [
    `状态：${status}`,
    item.exitCode !== undefined && item.exitCode !== null ? `退出码 ${item.exitCode}` : null,
    item.truncated ? "输出已截断" : null,
  ]
    .filter((part) => part !== null)
    .join(" · ");
  return (
    <Line
      className={`tool-line kind-shell status-${item.status}`}
      label={`运行 ${item.command || "命令"}，状态：${status}`}
      open={open}
      onToggle={toggle}
      head={
        <>
          {lineCaret()}
          <span className="tl-ic">{statusIcon(item.status)}</span>
          <span className="tl-name">运行</span>
          <span className="tl-mono">{item.command}</span>
          <span className="line-tail">
            <span className="tl-status" role="status">
              {status}
            </span>
          </span>
        </>
      }
    >
      <div className="tl-detail">
        {item.command ? <div className="cmd-echo">$ {item.command}</div> : null}
        {item.cwd ? <p className="tl-meta">目录：{item.cwd}</p> : null}
        {item.stdout ? <OutputBlock text={item.stdout} /> : null}
        {item.stderr ? <OutputBlock text={item.stderr} /> : null}
        <p className="tl-meta">{meta}</p>
        {item.output ? (
          <FullOutput sessionId={sessionId} itemId={item.id} outputRef={item.output} />
        ) : null}
      </div>
    </Line>
  );
}

function Terminal({ sessionId, item }: { sessionId: string; item: TerminalItem }) {
  const [open, toggle] = useLineExpanded(sessionId, item.id, false);
  const status = item.finished ? "已结束" : "运行中";
  const meta = [
    `状态：${status}`,
    item.exitCode !== undefined && item.exitCode !== null ? `退出码 ${item.exitCode}` : null,
    item.statusText ?? null,
    item.truncated ? "输出已截断" : null,
  ]
    .filter((part) => part !== null)
    .join(" · ");
  return (
    <Line
      className={`tool-line kind-terminal ${item.finished ? "status-finished" : "status-running"}`}
      label={`启动终端 ${item.command ?? item.handle}，状态：${status}`}
      open={open}
      onToggle={toggle}
      head={
        <>
          {lineCaret()}
          <span className="tl-ic">
            <SquareTerminal size={13} aria-hidden />
          </span>
          <span className="tl-name">启动终端</span>
          <span className="tl-mono">{item.command ?? item.handle}</span>
          <span className="line-tail">
            <span className="tl-status" role="status">
              {status}
            </span>
          </span>
        </>
      }
    >
      <div className="tl-detail">
        {item.command ? <div className="cmd-echo">$ {item.command}</div> : null}
        {item.cwd ? <p className="tl-meta">目录：{item.cwd}</p> : null}
        {item.output ? <OutputBlock text={item.output} /> : <p className="tl-meta">（暂无输出）</p>}
        <p className="tl-meta">{meta}</p>
        {item.outputRef ? (
          <FullOutput sessionId={sessionId} itemId={item.id} outputRef={item.outputRef} />
        ) : null}
      </div>
    </Line>
  );
}

function WorkflowLine({ sessionId, item }: { sessionId: string; item: WorkflowItem }) {
  const [open, toggle] = useLineExpanded(sessionId, item.id, false);
  const workflow = item.workflow;
  const elapsed = formatElapsed(
    workflow.started_at_ms && workflow.updated_at_ms
      ? Math.round((workflow.updated_at_ms - workflow.started_at_ms) / 1000)
      : undefined,
  );
  const status =
    (item.finished
      ? `已完成${workflow.terminal_reason ? `（${workflow.terminal_reason}）` : ""}`
      : `运行中${workflow.current_phase ? ` · ${workflow.current_phase}` : ""}`) +
    (elapsed ? ` · ${elapsed}` : "");
  return (
    <Line
      className={`tool-line kind-workflow ${item.finished ? "status-finished" : "status-running"}`}
      label={`工作流 ${workflow.title}，状态：${status}`}
      open={open}
      onToggle={toggle}
      head={
        <>
          {lineCaret()}
          <span className="tl-ic">
            <Workflow size={13} aria-hidden />
          </span>
          <span className="tl-name">{workflow.title}</span>
          <span className="tl-mono">{workflow.latest_log_summary ?? ""}</span>
          <span className="line-tail">
            <span className="tl-status" role="status">
              {status}
            </span>
          </span>
        </>
      }
    >
      <div className="tl-detail">
        {workflow.purpose ? <p className="ar-desc">{workflow.purpose}</p> : null}
        {workflow.latest_log_summary ? <p className="tl-meta">{workflow.latest_log_summary}</p> : null}
        <p className="tl-meta">
          调用 {workflow.invocation_count ?? 0} 次 · 失败 {workflow.failure_count ?? 0} 次
        </p>
      </div>
    </Line>
  );
}

// ---------------------------------------------------------------------------
// Approval / question rows: in place, 2px accent bar, inline buttons/chips.
// ---------------------------------------------------------------------------

function ApprovalRow({ sessionId, item }: { sessionId: string; item: ApprovalItem }) {
  const state = useAppState();
  const actions = useAppActions();
  const view = state.sessions[sessionId];
  const submitted = view?.submittedApprovalIds.includes(item.request.id) ?? false;
  const resolved = item.resolution !== undefined;
  const disabled = resolved || submitted;
  const presentation = item.request.presentation;
  const stale = item.resolution?.kind === "no_longer_pending";
  const resolutionLabel = stale
    ? "已失效"
    : (item.resolution?.label ?? item.resolution?.kind ?? "");
  const stateText = resolved
    ? stale
      ? "已失效"
      : `已处理：${resolutionLabel}`
    : submitted
      ? "已提交，等待确认"
      : "等待确认";
  return (
    <div
      className={`approval-row ${resolved ? "resolved" : ""}`}
      role="group"
      aria-label={`审批请求：${presentation.title ?? item.request.operation}`}
    >
      <div className="ar-head">
        <ShieldQuestion size={14} aria-hidden />
        <span className="ar-title">{presentation.title ?? "审批请求"}</span>
        <span className="ar-state">{stateText}</span>
      </div>
      {presentation.command ? <div className="cmd-echo">$ {presentation.command}</div> : null}
      {typeof presentation.kind === "string" && presentation.kind !== "command" ? (
        <p className="ar-desc">{presentation.kind}</p>
      ) : null}
      {!resolved ? (
        <div className="ar-actions">
          {item.request.options.map((option: ApprovalOption) => (
            <button
              key={option.label}
              type="button"
              className="chip-button"
              disabled={disabled}
              title={option.description ?? option.label}
              onClick={() => actions.submitApproval(item.request.id, option.action)}
            >
              {option.label}
            </button>
          ))}
        </div>
      ) : null}
      {item.resolution?.feedback ? <p className="ar-desc">备注：{item.resolution.feedback}</p> : null}
    </div>
  );
}

function QuestionRow({ sessionId, item }: { sessionId: string; item: QuestionItem }) {
  const state = useAppState();
  const actions = useAppActions();
  const questionId = item.id.replace(/^question:/, "");
  const view = state.sessions[sessionId];
  const submitted = view?.submittedQuestionIds.includes(questionId) ?? false;
  const disabled = item.resolved || submitted;
  const [selections, setSelections] = useState<string[]>([]);
  const [note, setNote] = useState("");
  const toggleSelection = (question: QuestionEventData, label: string) => {
    setSelections((current) =>
      question.multi_select
        ? current.includes(label)
          ? current.filter((entry) => entry !== label)
          : [...current, label]
        : [label],
    );
  };
  return (
    <div
      className={`approval-row question-row ${item.resolved ? "resolved" : ""}`}
      role="group"
      aria-label="提问"
    >
      <div className="ar-head">
        <CircleHelp size={14} aria-hidden />
        <span className="ar-title">提问</span>
        <span className="ar-state">
          {item.resolved ? "已回答" : submitted ? "已提交，等待确认" : "等待回答"}
        </span>
      </div>
      {item.questions.map((question) => (
        <div className="ar-question" key={question.header}>
          <p className="ar-question-text">{question.question}</p>
          {question.body ? <p className="ar-desc">{question.body}</p> : null}
          <div className="ar-chips">
            {question.options.map((option) => {
              const selected = selections.includes(option.label);
              return (
                <button
                  key={option.label}
                  type="button"
                  className={`chip ${selected ? "on" : ""}`}
                  aria-pressed={selected}
                  disabled={disabled}
                  title={option.description ?? option.label}
                  onClick={() => toggleSelection(question, option.label)}
                >
                  {option.label}
                </button>
              );
            })}
          </div>
        </div>
      ))}
      {!item.resolved ? (
        <>
          <input
            type="text"
            className="ar-other"
            aria-label="补充说明（可选）"
            placeholder="补充说明（可选）"
            value={note}
            disabled={disabled}
            onChange={(event) => setNote(event.target.value)}
          />
          <div className="ar-actions">
            <button
              type="button"
              className="chip-button primary"
              disabled={disabled || selections.length === 0}
              onClick={() =>
                actions.submitQuestion(questionId, {
                  selections,
                  ...(note.trim() !== "" ? { text: note.trim() } : {}),
                })
              }
            >
              提交回答
            </button>
          </div>
        </>
      ) : null}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Delegate / swarm rows (R4 §5.1): a delegate is a single agent-line that
// opens the drill-down panel; a swarm is a header line with an aggregate
// progress bar whose member rows (same agent-line presentation, border-l
// connector, stagger-in) open the panel per child agent.
// ---------------------------------------------------------------------------

export function agentStateText(state: string): string {
  switch (state) {
    case "running":
      return "运行中";
    case "queued":
      return "排队等待";
    case "completed":
      return "已完成";
    case "failed":
      return "失败";
    case "cancelled":
      return "已取消";
    case "aborted":
      return "已中止";
    case "timed_out":
      return "超时";
    default:
      return state;
  }
}

/** Status pill of the agent rows: text plus an icon — never color alone. */
export function AgentStatePill({ state }: { state: string }) {
  const icon =
    state === "running" ? (
      <Loader2 size={11} className="spin" aria-hidden />
    ) : state === "completed" ? (
      <Check size={11} aria-hidden />
    ) : state === "failed" || state === "timed_out" || state === "cancelled" || state === "aborted" ? (
      <XCircle size={11} aria-hidden />
    ) : (
      <Clock size={11} aria-hidden />
    );
  return (
    <span className={`agent-pill st-${state}`} role="status">
      {icon}
      {agentStateText(state)}
    </span>
  );
}

/** One agent-line: pulse dot while running + icon + title + dim progress
 * summary + elapsed + state pill. Clicking opens the drill-down panel. */
function AgentRow({
  sessionId,
  agent,
  className = "",
}: {
  sessionId: string;
  agent: AgentSnapshot;
  className?: string;
}) {
  const actions = useAppActions();
  const title = agent.task_title ?? agent.display_name;
  const elapsed = formatElapsed(agent.elapsed?.secs);
  const status =
    agentStateText(agent.state) +
    (elapsed !== null ? ` · ${elapsed}` : "") +
    (agent.terminal_reason ? ` · ${agent.terminal_reason}` : "");
  return (
    <div className={`line agent-line state-${agent.state} ${className}`}>
      <button
        type="button"
        className="line-head"
        aria-label={`查看子代理详情：${title}，状态：${status}`}
        onClick={() => actions.openAgentPanel(sessionId, agent)}
      >
        {agent.state === "running" ? <span className="pulse-dot" aria-hidden /> : null}
        <span className="tl-ic">
          <Bot size={13} aria-hidden />
        </span>
        <span className="tl-name">{title}</span>
        <span className="tl-mono">{agent.latest_text ?? ""}</span>
        <span className="line-tail">
          {elapsed !== null ? <span className="agent-elapsed">{elapsed}</span> : null}
          <AgentStatePill state={agent.state} />
        </span>
      </button>
    </div>
  );
}

function DelegateLine({ sessionId, item }: { sessionId: string; item: DelegateItem }) {
  return <AgentRow sessionId={sessionId} agent={item.agent} className="kind-delegate" />;
}

function SwarmBlock({ sessionId, item }: { sessionId: string; item: SwarmItem }) {
  const [open, toggle] = useLineExpanded(sessionId, item.id, true);
  const swarm = item.swarm;
  const aggregate = swarm.aggregate;
  const settled =
    aggregate.completed + aggregate.failed + aggregate.cancelled + aggregate.timed_out;
  const percent = aggregate.total > 0 ? Math.round((settled / aggregate.total) * 100) : 0;
  return (
    <Line
      className={`swarm-block state-${swarm.state}`}
      label={`并行子代理 ${swarm.description}，状态：${agentStateText(swarm.state)}，完成 ${aggregate.completed}/${aggregate.total}`}
      open={open}
      onToggle={toggle}
      head={
        <>
          {lineCaret()}
          <span className="tl-ic">
            <Network size={13} aria-hidden />
          </span>
          <span className="tl-name">{swarm.description}</span>
          <span
            className="swarm-bar"
            role="progressbar"
            aria-valuemin={0}
            aria-valuemax={aggregate.total}
            aria-valuenow={settled}
            aria-label={`聚合进度：已结束 ${settled}/${aggregate.total}`}
          >
            <span className="swarm-bar-fill" style={{ width: `${percent}%` }} />
          </span>
          <span className="line-tail">
            <span className="tl-status" role="status">
              完成 {aggregate.completed}/{aggregate.total}
            </span>
            <AgentStatePill state={swarm.state} />
          </span>
        </>
      }
    >
      <ul className="swarm-members">
        {swarm.children.map((child, index) => (
          <li
            key={child.item_index}
            className="swarm-member"
            style={{ "--stagger": index } as CSSProperties}
          >
            <AgentRow sessionId={sessionId} agent={child.agent} className="kind-swarm-member" />
            <span className="swarm-member-item" title={child.item}>
              {child.item}
            </span>
          </li>
        ))}
      </ul>
    </Line>
  );
}

// ---------------------------------------------------------------------------
// Status lines and the unknown-event record.
// ---------------------------------------------------------------------------

function RetryCard({ item }: { item: RetryItem }) {
  const text =
    item.phase === "exhausted"
      ? `重试失败：${item.message}（${item.errorCode}）`
      : item.phase === "connecting"
        ? `正在重连（第 ${item.retry}/${item.maxRetries} 次）`
        : `服务暂不可用，${item.delayMs}ms 后重试（第 ${item.retry}/${item.maxRetries} 次）：${item.message}`;
  return (
    <div
      className={`status-line ${item.phase === "exhausted" ? "severity-error" : "severity-info"}`}
      role="status"
    >
      <Loader2 size={14} className={item.phase === "exhausted" ? "" : "spin"} aria-hidden />
      <span>{text}</span>
    </div>
  );
}

function StatusLine({ item }: { item: StatusLineItem }) {
  return (
    <div className={`status-line severity-${item.severity}`} role="status">
      <XCircle size={14} aria-hidden />
      <span>{item.text}</span>
    </div>
  );
}

function Unknown({ sessionId, item }: { sessionId: string; item: UnknownItem }) {
  const [open, toggle] = useLineExpanded(sessionId, item.id, false);
  return (
    <Line
      className="tool-line kind-unknown"
      label={`未识别事件 ${item.tag}，状态：已保留原始内容`}
      open={open}
      onToggle={toggle}
      head={
        <>
          {lineCaret()}
          <span className="tl-ic">
            <CircleHelp size={13} aria-hidden />
          </span>
          <span className="tl-name">未识别事件 {item.tag}</span>
          <span className="line-tail">
            <span className="tl-status" role="status">
              已保留原始内容
            </span>
          </span>
        </>
      }
    >
      <div className="tl-detail">
        <pre className="unknown-raw">
          <code>{item.raw}</code>
        </pre>
      </div>
    </Line>
  );
}

export function TranscriptItemView({
  sessionId,
  item,
}: {
  sessionId: string;
  item: TranscriptItem;
}) {
  switch (item.kind) {
    case "user_message":
      return <UserMessage item={item} />;
    case "assistant_message":
      return <AssistantBody item={item} />;
    case "thinking":
      return <Think sessionId={sessionId} item={item} />;
    case "tool":
      return <Tool sessionId={sessionId} item={item} />;
    case "shell":
      return <Shell sessionId={sessionId} item={item} />;
    case "terminal":
      return <Terminal sessionId={sessionId} item={item} />;
    case "workflow":
      return <WorkflowLine sessionId={sessionId} item={item} />;
    case "approval":
      return <ApprovalRow sessionId={sessionId} item={item} />;
    case "question":
      return <QuestionRow sessionId={sessionId} item={item} />;
    case "delegate":
      return <DelegateLine sessionId={sessionId} item={item} />;
    case "swarm":
      return <SwarmBlock sessionId={sessionId} item={item} />;
    case "retry":
      return <RetryCard item={item} />;
    case "status":
      return <StatusLine item={item} />;
    case "unknown":
      return <Unknown sessionId={sessionId} item={item} />;
  }
}
