/**
 * Transcript item renderers. User messages right, assistant body left (no
 * big cards); thinking/tools/terminals/workflows/delegates are compact
 * collapsible bars with real states; approvals and questions stay in place.
 */

import {
  Bot,
  Brain,

  CircleHelp,
  Hammer,
  ListChecks,
  Loader2,
  Network,
  ShieldQuestion,
  SquareTerminal,
  Workflow,
  XCircle,
} from "lucide-react";
import { useState } from "react";
import type { ApprovalOption, QuestionEventData } from "../protocol";
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
  TranscriptItem,
  UnknownItem,
  UserMessageItem,
  WorkflowItem,
} from "../state/transcript";
import { CodeBlock, OutputBlock } from "./codeBlock";
import { CollapsibleBar } from "./collapsible";
import { FullOutput } from "./fullOutput";
import { Markdown } from "./markdown";

function formatElapsed(secs: number | undefined): string | null {
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

// ---------------------------------------------------------------------------

function UserMessage({ item }: { item: UserMessageItem }) {
  return (
    <div className="row row-user">
      <div className="user-bubble">{item.text}</div>
    </div>
  );
}

function AssistantMessage({ item }: { item: AssistantMessageItem }) {
  return (
    <div className="row row-assistant">
      <div className="assistant-body">
        <Markdown text={item.text} />
        {!item.finished ? <span className="streaming-caret" aria-label="正在生成" /> : null}
      </div>
    </div>
  );
}

function Thinking({ sessionId, item }: { sessionId: string; item: ThinkingItem }) {
  return (
    <CollapsibleBar
      sessionId={sessionId}
      itemId={item.id}
      icon={<Brain size={14} />}
      title="思考"
      status={item.finished ? (item.redacted ? "已完成（已隐藏部分内容）" : "已完成") : "思考中"}
      className="kind-thinking"
    >
      <pre className="thinking-text">
        <code>{item.text}</code>
      </pre>
    </CollapsibleBar>
  );
}

function Tool({ sessionId, item }: { sessionId: string; item: ToolItem }) {
  return (
    <CollapsibleBar
      sessionId={sessionId}
      itemId={item.id}
      icon={<Hammer size={14} />}
      title={`工具 ${item.name}`}
      status={toolStatusText(item)}
      className={`kind-tool status-${item.status}`}
    >
      <div className="detail-stack">
        {item.arguments !== undefined ? (
          <CodeBlock code={argumentsPreview(item.arguments)} language="参数" />
        ) : null}
        {item.partialResult && item.status === "running" ? (
          <OutputBlock text={item.partialResult.content} />
        ) : null}
        {item.result ? (
          <OutputBlock text={item.result.content || (item.result.is_error ? "（错误，无内容）" : "（无内容）")} />
        ) : null}
        {item.output ? (
          <FullOutput sessionId={sessionId} itemId={item.id} outputRef={item.output} />
        ) : null}
      </div>
    </CollapsibleBar>
  );
}

function Shell({ sessionId, item }: { sessionId: string; item: ShellItem }) {
  return (
    <CollapsibleBar
      sessionId={sessionId}
      itemId={item.id}
      icon={<SquareTerminal size={14} />}
      title={`命令 ${item.command || "shell"}`}
      status={
        toolStatusText(item) +
        (item.exitCode !== undefined && item.exitCode !== null
          ? ` · 退出码 ${item.exitCode}`
          : "") +
        (item.truncated ? " · 输出已截断" : "")
      }
      className={`kind-shell status-${item.status}`}
    >
      <div className="detail-stack">
        {item.cwd ? <p className="muted">目录：{item.cwd}</p> : null}
        {item.stdout ? <OutputBlock text={item.stdout} /> : null}
        {item.stderr ? <OutputBlock text={item.stderr} /> : null}
        {item.truncated ? <p className="muted">服务端标记：输出已截断。</p> : null}
        {item.output ? (
          <FullOutput sessionId={sessionId} itemId={item.id} outputRef={item.output} />
        ) : null}
      </div>
    </CollapsibleBar>
  );
}

function Terminal({ sessionId, item }: { sessionId: string; item: TerminalItem }) {
  const status = item.finished
    ? `已结束${item.exitCode !== undefined && item.exitCode !== null ? ` · 退出码 ${item.exitCode}` : ""}${item.truncated ? " · 输出已截断" : ""}`
    : `运行中${item.truncated ? " · 输出已截断" : ""}`;
  return (
    <CollapsibleBar
      sessionId={sessionId}
      itemId={item.id}
      icon={<SquareTerminal size={14} />}
      title={`终端 ${item.command ?? item.handle}`}
      status={status}
      className="kind-terminal"
    >
      <div className="detail-stack">
        {item.cwd ? <p className="muted">目录：{item.cwd}</p> : null}
        {item.output ? <OutputBlock text={item.output} /> : <p className="muted">（暂无输出）</p>}
        {item.outputRef ? (
          <FullOutput sessionId={sessionId} itemId={item.id} outputRef={item.outputRef} />
        ) : null}
      </div>
    </CollapsibleBar>
  );
}

function WorkflowCard({ sessionId, item }: { sessionId: string; item: WorkflowItem }) {
  const workflow = item.workflow;
  const elapsed = formatElapsed(
    workflow.started_at_ms && workflow.updated_at_ms
      ? Math.round((workflow.updated_at_ms - workflow.started_at_ms) / 1000)
      : undefined,
  );
  const status =
    (item.finished ? `已完成${workflow.terminal_reason ? `（${workflow.terminal_reason}）` : ""}` : `运行中${workflow.current_phase ? ` · ${workflow.current_phase}` : ""}`) +
    (elapsed ? ` · ${elapsed}` : "");
  return (
    <CollapsibleBar
      sessionId={sessionId}
      itemId={item.id}
      icon={<Workflow size={14} />}
      title={`工作流 ${workflow.title}`}
      status={status}
      className="kind-workflow"
    >
      <div className="detail-stack">
        {workflow.purpose ? <p className="muted">{workflow.purpose}</p> : null}
        {workflow.latest_log_summary ? <p>{workflow.latest_log_summary}</p> : null}
        <p className="muted">
          调用 {workflow.invocation_count ?? 0} 次 · 失败 {workflow.failure_count ?? 0} 次
        </p>
      </div>
    </CollapsibleBar>
  );
}

function ApprovalCard({ sessionId, item }: { sessionId: string; item: ApprovalItem }) {
  const state = useAppState();
  const actions = useAppActions();
  const view = state.sessions[sessionId];
  const submitted = view?.submittedApprovalIds.includes(item.request.id) ?? false;
  const resolved = item.resolution !== undefined;
  const disabled = resolved || submitted;
  const presentation = item.request.presentation;
  const resolutionLabel =
    item.resolution?.label ??
    (item.resolution?.kind === "no_longer_pending"
      ? "已失效"
      : (item.resolution?.kind ?? ""));
  return (
    <div className="card approval-card" role="group" aria-label={`审批请求：${presentation.title ?? item.request.operation}`}>
      <div className="card-header">
        <ShieldQuestion size={14} aria-hidden />
        <span className="card-title">{presentation.title ?? "审批请求"}</span>
        <span className="card-state">
          {resolved ? `已处理：${resolutionLabel}` : submitted ? "已提交，等待确认" : "等待确认"}
        </span>
      </div>
      {presentation.command ? (
        <CodeBlock code={presentation.command} language={presentation.cwd ? `目录 ${presentation.cwd}` : "command"} />
      ) : null}
      {!resolved ? (
        <div className="card-actions">
          {item.request.options.map((option: ApprovalOption) => (
            <button
              key={option.label}
              type="button"
              className="action-button"
              disabled={disabled}
              title={option.description ?? option.label}
              onClick={() => actions.submitApproval(item.request.id, option.action)}
            >
              {option.label}
            </button>
          ))}
        </div>
      ) : null}
      {item.resolution?.feedback ? <p className="muted">备注：{item.resolution.feedback}</p> : null}
    </div>
  );
}

function QuestionFields({
  question,
  selections,
  onToggle,
  disabled,
}: {
  question: QuestionEventData;
  selections: string[];
  onToggle(label: string): void;
  disabled: boolean;
}) {
  return (
    <fieldset className="question-fields" disabled={disabled}>
      <legend className="question-title">
        <CircleHelp size={14} aria-hidden /> {question.question}
      </legend>
      {question.body ? <p className="muted">{question.body}</p> : null}
      {question.options.map((option) => (
        <label key={option.label} className="question-option">
          <input
            type={question.multi_select ? "checkbox" : "radio"}
            name={question.header}
            checked={selections.includes(option.label)}
            disabled={disabled}
            onChange={() => onToggle(option.label)}
          />
          <span>{option.label}</span>
          {option.description ? <span className="muted">（{option.description}）</span> : null}
        </label>
      ))}
    </fieldset>
  );
}

function QuestionCard({ sessionId, item }: { sessionId: string; item: QuestionItem }) {
  const state = useAppState();
  const actions = useAppActions();
  const questionId = item.id.replace(/^question:/, "");
  const view = state.sessions[sessionId];
  const submitted = view?.submittedQuestionIds.includes(questionId) ?? false;
  const disabled = item.resolved || submitted;
  const [selections, setSelections] = useState<string[]>([]);
  const [note, setNote] = useState("");
  const multi = item.questions.some((question) => question.multi_select);
  const toggle = (label: string) => {
    setSelections((current) =>
      multi
        ? current.includes(label)
          ? current.filter((entry) => entry !== label)
          : [...current, label]
        : [label],
    );
  };
  return (
    <div className="card question-card" role="group" aria-label="提问">
      <div className="card-header">
        <CircleHelp size={14} aria-hidden />
        <span className="card-title">提问</span>
        <span className="card-state">
          {item.resolved ? "已回答" : submitted ? "已提交，等待确认" : "等待回答"}
        </span>
      </div>
      {item.questions.map((question) => (
        <QuestionFields
          key={question.header}
          question={question}
          selections={selections}
          onToggle={toggle}
          disabled={disabled}
        />
      ))}
      {!item.resolved ? (
        <>
          <label className="question-note">
            <span className="muted">补充说明（可选）</span>
            <input
              type="text"
              value={note}
              disabled={disabled}
              onChange={(event) => setNote(event.target.value)}
            />
          </label>
          <div className="card-actions">
            <button
              type="button"
              className="action-button primary"
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

function agentStateText(state: string): string {
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
    case "timed_out":
      return "超时";
    default:
      return state;
  }
}

function DelegateCard({ sessionId, item }: { sessionId: string; item: DelegateItem }) {
  const agent = item.agent;
  const elapsed = formatElapsed(agent.elapsed?.secs);
  const status =
    agentStateText(agent.state) +
    (elapsed ? ` · ${elapsed}` : "") +
    (agent.terminal_reason ? ` · ${agent.terminal_reason}` : "");
  return (
    <CollapsibleBar
      sessionId={sessionId}
      itemId={item.id}
      icon={<Bot size={14} />}
      title={`子代理 ${agent.task_title ?? agent.display_name}`}
      status={status}
      className="kind-delegate"
    >
      <div className="detail-stack">
        {agent.task ? <p>{agent.task}</p> : null}
        <p className="muted">
          工具 {agent.tool_count ?? 0} 次 · 消息 {agent.live_messages_received ?? 0} 条 · token{" "}
          {agent.token_count ?? 0}
        </p>
      </div>
    </CollapsibleBar>
  );
}

function SwarmCard({ sessionId, item }: { sessionId: string; item: SwarmItem }) {
  const swarm = item.swarm;
  const aggregate = swarm.aggregate;
  const status = `${agentStateText(swarm.state)} · 共 ${aggregate.total} 项：运行 ${aggregate.running} · 排队 ${aggregate.queued} · 完成 ${aggregate.completed} · 失败 ${aggregate.failed}`;
  return (
    <CollapsibleBar
      sessionId={sessionId}
      itemId={item.id}
      icon={<Network size={14} />}
      title={`并行子代理 ${swarm.description}`}
      status={status}
      className="kind-swarm"
    >
      <ul className="swarm-children">
        {swarm.children.map((child) => (
          <li key={child.item_index} className="swarm-child">
            <span className="swarm-child-state">{agentStateText(child.agent.state)}</span>
            <span className="swarm-child-item">{child.item}</span>
            <span className="muted">
              {formatElapsed(child.agent.elapsed?.secs) ?? ""}
              {child.agent.terminal_reason ? ` · ${child.agent.terminal_reason}` : ""}
            </span>
          </li>
        ))}
      </ul>
    </CollapsibleBar>
  );
}

function RetryCard({ item }: { item: RetryItem }) {
  const text =
    item.phase === "exhausted"
      ? `重试失败：${item.message}（${item.errorCode}）`
      : item.phase === "connecting"
        ? `正在重连（第 ${item.retry}/${item.maxRetries} 次）`
        : `服务暂不可用，${item.delayMs}ms 后重试（第 ${item.retry}/${item.maxRetries} 次）：${item.message}`;
  return (
    <div className={`status-line ${item.phase === "exhausted" ? "severity-error" : "severity-info"}`} role="status">
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
  return (
    <CollapsibleBar
      sessionId={sessionId}
      itemId={item.id}
      icon={<ListChecks size={14} />}
      title={`未识别事件 ${item.tag}`}
      status="已保留原始内容"
      className="kind-unknown"
    >
      <pre className="unknown-raw">
        <code>{item.raw}</code>
      </pre>
    </CollapsibleBar>
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
      return <AssistantMessage item={item} />;
    case "thinking":
      return <Thinking sessionId={sessionId} item={item} />;
    case "tool":
      return <Tool sessionId={sessionId} item={item} />;
    case "shell":
      return <Shell sessionId={sessionId} item={item} />;
    case "terminal":
      return <Terminal sessionId={sessionId} item={item} />;
    case "workflow":
      return <WorkflowCard sessionId={sessionId} item={item} />;
    case "approval":
      return <ApprovalCard sessionId={sessionId} item={item} />;
    case "question":
      return <QuestionCard sessionId={sessionId} item={item} />;
    case "delegate":
      return <DelegateCard sessionId={sessionId} item={item} />;
    case "swarm":
      return <SwarmCard sessionId={sessionId} item={item} />;
    case "retry":
      return <RetryCard item={item} />;
    case "status":
      return <StatusLine item={item} />;
    case "unknown":
      return <Unknown sessionId={sessionId} item={item} />;
  }
}
