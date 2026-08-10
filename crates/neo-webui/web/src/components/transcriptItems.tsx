/**
 * Transcript item renderers (redesign §4): typography is the hierarchy, not
 * cards. Only the user message is a bubble (.u-turn); thinking, tools,
 * shells, terminals, workflows, delegates and unknown records are single
 * 24px lines (.think / .tool-line / .agent-line) that expand in place;
 * approvals and questions are in-place rows with a 2px accent bar
 * (.approval-row). Terminal results use a left status icon and retain text in
 * the expanded detail for screen-reader parity.
 */

import {
  Bot,
  Brain,
  Check,
  ChevronRight,
  CircleHelp,
  CircleStop,
  Clock,
  Eye,
  File,
  FilePenLine,
  FilePlus,
  FolderSearch,
  ListChecks,
  Loader2,
  MessageCircle,
  Network,
  Search,
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
          {resultIcon(item.finished ? "finished" : "running")}
          {lineCaret()}
          <Brain size={14} aria-hidden />
          <span className="think-title">思考</span>
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
  secondary?: string;
  icon: ToolIconName;
}

type ToolIconName =
  | "file"
  | "media"
  | "file-edit"
  | "file-plus"
  | "search"
  | "folder-search"
  | "terminal"
  | "delegate"
  | "swarm"
  | "todo"
  | "wait"
  | "stop"
  | "message"
  | "skill"
  | "question"
  | "workflow"
  | "goal"
  | "unknown";

function normalizedToolName(name: string): string {
  return name.trim().toLowerCase();
}

function argumentText(args: unknown, fields: readonly string[]): string | null {
  if (!args || typeof args !== "object") return null;
  const record = args as Record<string, unknown>;
  for (const field of fields) {
    const value = record[field];
    if (typeof value === "string" && value !== "") return value;
    if (typeof value === "number" || typeof value === "boolean") return String(value);
    if (Array.isArray(value)) {
      const entries = value.filter(
        (entry): entry is string | number => typeof entry === "string" || typeof entry === "number",
      );
      if (entries.length > 0) return entries.join(", ");
    }
  }
  return null;
}

function pathPresentation(path: string): Pick<ToolPresentation, "target" | "secondary"> {
  const trimmed = path.replace(/[\\/]+$/, "");
  const separator = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  if (trimmed === "" || separator < 0) {
    return { target: path };
  }
  return {
    target: trimmed.slice(separator + 1),
    secondary: trimmed.slice(0, separator + 1),
  };
}

function pathTarget(args: unknown): Pick<ToolPresentation, "target" | "secondary"> {
  const path = argumentText(args, ["path"]);
  return path === null ? { target: "" } : pathPresentation(path);
}

function workflowPresentation(args: unknown): ToolPresentation {
  const action = argumentText(args, ["action"]);
  const target = argumentText(args, ["name", "workflow", "id"]) ?? "";
  switch (action) {
    case "list":
    case "show":
      return { action: "查看工作流", target, icon: "workflow" };
    case "save":
      return { action: "保存工作流", target, icon: "workflow" };
    case "validate_inline":
    case "validate_saved":
      return { action: "校验工作流", target, icon: "workflow" };
    case "run_inline":
    case "run_saved":
      return { action: "运行工作流", target, icon: "workflow" };
    default:
      return { action: "处理工作流", target, icon: "workflow" };
  }
}

export function toolPresentation(name: string, args: unknown): ToolPresentation {
  const toolName = normalizedToolName(name);
  switch (toolName) {
    case "grep":
      return {
        action: "搜索",
        target: argumentText(args, ["pattern", "query", "path"]) ?? "",
        secondary: argumentText(args, ["directory", "cwd", "root"]) ?? undefined,
        icon: "search",
      };
    case "glob":
    case "find":
      return {
        action: "搜索",
        target: argumentText(args, ["pattern", "query", "path"]) ?? "",
        secondary: argumentText(args, ["directory", "cwd", "root"]) ?? undefined,
        icon: "folder-search",
      };
    case "read":
    case "list":
      return { action: "读取", ...pathTarget(args), icon: "file" };
    case "readmediafile":
      return { action: "观察", ...pathTarget(args), icon: "media" };
    case "skill":
      return {
        action: "使用技能",
        target: argumentText(args, ["skill"]) ?? "",
        icon: "skill",
      };
    case "askuserquestion":
      return { action: "询问用户", target: "", icon: "question" };
    case "edit":
      return { action: "编辑", target: argumentText(args, ["path"]) ?? "", icon: "file-edit" };
    case "write":
      return { action: "创建", target: argumentText(args, ["path"]) ?? "", icon: "file-plus" };
    case "bash":
    case "shell":
      return { action: "运行", target: argumentText(args, ["command"]) ?? "", icon: "terminal" };
    case "terminal":
      return {
        action: "启动终端",
        target: argumentText(args, ["command", "handle"]) ?? "",
        icon: "terminal",
      };
    case "tasklist":
      return { action: "查看后台任务", target: "", icon: "todo" };
    case "taskoutput":
      return {
        action: "查看任务输出",
        target: argumentText(args, ["task_id", "id"]) ?? "",
        icon: "todo",
      };
    case "taskstop":
      return {
        action: "停止后台任务",
        target: argumentText(args, ["task_id", "id"]) ?? "",
        icon: "stop",
      };
    case "taskpause":
      return {
        action: "暂停后台任务",
        target: argumentText(args, ["task_id", "id"]) ?? "",
        icon: "wait",
      };
    case "taskresume":
      return {
        action: "恢复后台任务",
        target: argumentText(args, ["task_id", "id"]) ?? "",
        icon: "wait",
      };
    case "taskanswer":
      return {
        action: "回答后台任务",
        target: argumentText(args, ["task_id", "id"]) ?? "",
        icon: "message",
      };
    case "enterplanmode":
      return { action: "进入计划模式", target: "", icon: "workflow" };
    case "exitplanmode":
      return { action: "退出计划模式", target: "", icon: "workflow" };
    case "delegate":
    case "delegategroup":
    case "delegateswarm":
    case "task":
      return {
        action: "派发子代理",
        target: argumentText(args, ["task", "prompt", "description", "name"]) ?? "",
        icon: toolName === "delegateswarm" ? "swarm" : "delegate",
      };
    case "waitdelegate":
      return {
        action: "等待子代理",
        target: argumentText(args, ["ids", "id", "agent_id", "swarm_id"]) ?? "",
        icon: "wait",
      };
    case "listdelegates":
      return { action: "查看子代理", target: "", icon: "delegate" };
    case "interruptdelegate":
    case "stopdelegate":
      return { action: "停止子代理", target: "", icon: "stop" };
    case "messagedelegate":
      return {
        action: "联系子代理",
        target: argumentText(args, ["message", "text", "agent_id"]) ?? "",
        icon: "message",
      };
    case "todolist":
    case "settodolist":
      return { action: "更新任务清单", target: "", icon: "todo" };
    case "sleep":
      return {
        action: "等待",
        target: argumentText(args, ["reason", "duration_seconds"]) ?? "",
        icon: "wait",
      };
    case "workflow":
      return workflowPresentation(args);
    case "startgoal":
      return {
        action: "开始目标",
        target: argumentText(args, ["objective"]) ?? "",
        icon: "goal",
      };
    case "exitgoalmode":
      return {
        action: "退出目标模式",
        target: argumentText(args, ["objective"]) ?? "",
        icon: "goal",
      };
    case "updategoalstatus":
      return {
        action: "更新目标状态",
        target: argumentText(args, ["status", "reason"]) ?? "",
        icon: "goal",
      };
    case "getgoalstatus":
      return { action: "查看目标状态", target: "", icon: "goal" };
    case "listskills":
      return { action: "查看技能", target: "", icon: "skill" };
    case "createskill":
      return {
        action: "创建技能",
        target: argumentText(args, ["name", "skill"]) ?? "",
        icon: "skill",
      };
    case "moveskill":
      return {
        action: "移动技能",
        target: argumentText(args, ["name", "skill", "source"]) ?? "",
        secondary: argumentText(args, ["destination_parent"]) ?? undefined,
        icon: "skill",
      };
    case "summarizesessions":
      return {
        action: "整理会话",
        target: argumentText(args, ["session_id", "days"]) ?? "",
        icon: "message",
      };
    case "themedraft":
      return {
        action: "起草主题",
        target: argumentText(args, ["name", "theme"]) ?? "",
        icon: "skill",
      };
    default:
      return { action: name, target: argumentsPreview(args), icon: "unknown" };
  }
}

function toolIcon(icon: ToolIconName) {
  switch (icon) {
    case "file":
      return <File size={13} aria-hidden />;
    case "media":
      return <Eye size={13} aria-hidden />;
    case "file-edit":
      return <FilePenLine size={13} aria-hidden />;
    case "file-plus":
      return <FilePlus size={13} aria-hidden />;
    case "search":
      return <Search size={13} aria-hidden />;
    case "folder-search":
      return <FolderSearch size={13} aria-hidden />;
    case "terminal":
      return <SquareTerminal size={13} aria-hidden />;
    case "delegate":
      return <Bot size={13} aria-hidden />;
    case "swarm":
      return <Network size={13} aria-hidden />;
    case "todo":
      return <ListChecks size={13} aria-hidden />;
    case "wait":
      return <Clock size={13} aria-hidden />;
    case "stop":
      return <CircleStop size={13} aria-hidden />;
    case "message":
      return <MessageCircle size={13} aria-hidden />;
    case "skill":
      return <Brain size={13} aria-hidden />;
    case "question":
      return <ShieldQuestion size={13} aria-hidden />;
    case "workflow":
      return <Workflow size={13} aria-hidden />;
    case "goal":
      return <Brain size={13} aria-hidden />;
    case "unknown":
      return <CircleHelp size={13} aria-hidden />;
  }
}

function resultIcon(status: ToolStatus) {
  return (
    <span className="tl-result-ic" data-status-icon={status} aria-hidden>
      {statusIcon(status)}
    </span>
  );
}

function toolIconView(icon: ToolIconName) {
  return (
    <span className="tl-tool-icon" data-tool-icon={icon} aria-hidden>
      {toolIcon(icon)}
    </span>
  );
}

function objectArgument(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function stringField(record: Record<string, unknown> | null, field: string): string | null {
  const value = record?.[field];
  return typeof value === "string" ? value : null;
}

function countField(record: Record<string, unknown>, field: string): number {
  const value = record[field];
  return typeof value === "number" && Number.isFinite(value) && value > 0
    ? Math.floor(value)
    : 0;
}

interface CommittedChange {
  path: string;
  added: number;
  removed: number;
  replacements: number;
  operation: string | null;
  diff: string | null;
  content: string | null;
}

function committedChanges(item: ToolItem): CommittedChange[] {
  const details = objectArgument(item.result?.details);
  if (!details || !Array.isArray(details.changes)) return [];
  const changes: CommittedChange[] = [];
  for (const rawChange of details.changes) {
    const change = objectArgument(rawChange);
    if (
      !change ||
      (change.status !== "committed" && change.status !== "committed_unsynced") ||
      typeof change.path !== "string" ||
      change.path === ""
    ) continue;
    changes.push({
      path: change.path,
      added: countField(change, "added"),
      removed: countField(change, "removed"),
      replacements: countField(change, "replacements"),
      operation: typeof change.operation === "string" ? change.operation : null,
      diff: typeof change.diff === "string" && change.diff.trim() !== ""
        ? change.diff
        : null,
      content: typeof change.content === "string" ? change.content : null,
    });
  }
  return changes;
}

function changeSummary(kind: "edit" | "write", changes: CommittedChange[]): string {
  return changes.map((change) => {
    const action = kind === "edit"
      ? (change.replacements > 0 ? `替换 ${change.replacements} 处` : "已编辑")
      : change.operation === "created" ? "已创建" : "已写入";
    const counts = [
      change.added > 0 ? `+${change.added}` : null,
      change.removed > 0 ? `−${change.removed}` : null,
    ].filter((part): part is string => part !== null);
    return [action, change.path, ...counts].join(" ");
  }).join(" · ");
}

function ChangeRatio({ change }: { change: CommittedChange }) {
  const total = change.added + change.removed;
  const addedPercent = total === 0 ? 0 : (change.added / total) * 100;
  const removedPercent = total === 0 ? 0 : (change.removed / total) * 100;
  return (
    <div className="tl-change-ratio-row">
      <span className="tl-change-ratio-path tl-mono">{change.path}</span>
      <span
        className="tl-change-ratio"
        role="img"
        aria-label={`新增 ${change.added} 行，删除 ${change.removed} 行`}
      >
        <span
          className="tl-change-ratio-add"
          style={{ width: `${addedPercent}%` }}
        />
        <span
          className="tl-change-ratio-remove"
          style={{ width: `${removedPercent}%` }}
        />
      </span>
      <span className="tl-change-ratio-counts" aria-hidden>
        <span className="tl-change-ratio-added">+{change.added}</span>
        <span className="tl-change-ratio-removed">−{change.removed}</span>
      </span>
    </div>
  );
}

type LocalDiffLineKind = "add" | "del" | "context" | "separator";

interface LocalDiffLine {
  content: string;
  kind: LocalDiffLineKind;
}

function editDiffHeaderMatches(line: string, marker: "---" | "+++", path: string): boolean {
  if (!line.startsWith(`${marker} `)) return false;
  const headerPath = line.slice(marker.length + 1).trim().split("\t", 1)[0] ?? "";
  const normalizedPath = path.split("\\").join("/");
  return headerPath === "/dev/null" || headerPath === normalizedPath ||
    headerPath === `a/${normalizedPath}` || headerPath === `b/${normalizedPath}`;
}

function localEditDiffLines(path: string, diff: string): LocalDiffLine[] {
  const sourceLines = diff.split(/\r?\n/);
  if (sourceLines[sourceLines.length - 1] === "") sourceLines.pop();
  const hasFileHeaders = sourceLines.length >= 3 &&
    editDiffHeaderMatches(sourceLines[0] ?? "", "---", path) &&
    editDiffHeaderMatches(sourceLines[1] ?? "", "+++", path) &&
    (sourceLines[2] ?? "").startsWith("@@");
  return sourceLines.slice(hasFileHeaders ? 2 : 0).map((line) => {
    if (line.startsWith("@@")) return { content: "...", kind: "separator" };
    if (line.startsWith("+")) return { content: line, kind: "add" };
    if (line.startsWith("-")) return { content: line, kind: "del" };
    return { content: line, kind: "context" };
  });
}

function LocalEditDiff({ path, diff }: { path: string; diff: string }) {
  const lines = localEditDiffLines(path, diff);
  return (
    <pre className="ft-local-diff tl-local-diff" aria-label={`${path} 的局部差异`}>
      {lines.map((line, index) => (
        <span className={`ft-diff-${line.kind}`} key={`${index}:${line.content}`}>
          {line.content || " "}
        </span>
      ))}
    </pre>
  );
}

interface TodoEntry {
  title: string;
  status: "pending" | "in_progress" | "done";
}

function todoEntries(value: unknown): TodoEntry[] | null {
  if (!Array.isArray(value)) return null;
  const entries: TodoEntry[] = [];
  for (const rawEntry of value) {
    const entry = objectArgument(rawEntry);
    const title = stringField(entry, "title");
    const status = stringField(entry, "status");
    if (
      title === null ||
      (status !== "pending" && status !== "in_progress" && status !== "done")
    ) return null;
    entries.push({ title, status });
  }
  return entries;
}

function todoEntriesFor(item: ToolItem): TodoEntry[] | null {
  const result = objectArgument(item.result?.details);
  const completed = todoEntries(result?.todos);
  if (completed !== null) return completed;
  return todoEntries(objectArgument(item.arguments)?.todos);
}

function todoStatusText(status: TodoEntry["status"]): string {
  switch (status) {
    case "pending":
      return "待处理";
    case "in_progress":
      return "进行中";
    case "done":
      return "已完成";
  }
}

function TodoProgress({ entries, compact = false }: { entries: TodoEntry[]; compact?: boolean }) {
  const completed = entries.filter((entry) => entry.status === "done").length;
  const total = entries.length;
  const progress = total === 0 ? 0 : Math.round((completed / total) * 100);
  return (
    <span
      className={`tl-todo-progress${compact ? " compact" : ""}`}
      role="progressbar"
      aria-label={`任务进度：${completed}/${total}`}
      aria-valuemin={0}
      aria-valuemax={total}
      aria-valuenow={completed}
    >
      <span className="tl-todo-progress-count">
        {compact ? `${completed}/${total}` : `已完成 ${completed}/${total}`}
      </span>
      <span className="tl-todo-progress-track" aria-hidden>
        <span className="tl-todo-progress-fill" style={{ width: `${progress}%` }} />
      </span>
    </span>
  );
}

function ToolResultView({ item, includeSuccess = true }: { item: ToolItem; includeSuccess?: boolean }) {
  return (
    <>
      {item.partialResult && item.status === "running" ? (
        <OutputBlock text={item.partialResult.content} />
      ) : null}
      {item.result && (includeSuccess || item.result.is_error) ? (
        <OutputBlock
          text={
            item.result.content || (item.result.is_error ? "（错误，无内容）" : "（无内容）")
          }
        />
      ) : null}
    </>
  );
}

function ToolMeta({ item, status }: { item: ToolItem; status: string }) {
  return (
    <p className="tl-meta">
      状态：{status}
      {item.result?.is_error ? " · 结果标记为错误" : ""}
    </p>
  );
}

function showsRawArguments(name: string): boolean {
  return toolPresentation(name, undefined).icon === "unknown";
}

function GenericToolDetails({ item, echo, status }: { item: ToolItem; echo: string | null; status: string }) {
  return (
    <>
      {echo !== null ? <div className="cmd-echo">$ {echo}</div> : null}
      {item.arguments !== undefined && showsRawArguments(item.name) ? (
        <CodeBlock code={argumentsPreview(item.arguments)} language="参数" />
      ) : null}
      <ToolResultView item={item} />
      <ToolMeta item={item} status={status} />
    </>
  );
}

function SkillToolDetails({ item, status }: { item: ToolItem; status: string }) {
  const skillName = argumentText(item.arguments, ["skill"]) ?? "未命名技能";
  return (
    <>
      <p className="tl-meta">已调用技能：{skillName}</p>
      {item.result?.is_error ? <p className="tl-meta">技能调用失败</p> : null}
      <ToolMeta item={item} status={status} />
    </>
  );
}

function FileToolDetails({ item, status, kind }: {
  item: ToolItem;
  status: string;
  kind: "edit" | "write";
}) {
  const argumentsRecord = objectArgument(item.arguments);
  const argumentPath = stringField(argumentsRecord, "path");
  const argumentContent = kind === "write"
    ? stringField(argumentsRecord, "content")
    : null;
  const changes = committedChanges(item);
  const hasLocalDiff = kind === "edit" && changes.some((change) => change.diff !== null);
  const resultContent = kind === "write"
    ? changes.find((change) => change.operation === "created" && change.content !== null)?.content ?? null
    : null;
  const content = argumentContent ?? resultContent;
  const contentPath = argumentPath ?? changes.find((change) => change.content !== null)?.path ?? "文件";
  if (changes.length === 0 && content === null) {
    return <GenericToolDetails item={item} echo={commandEchoOf(item.arguments)} status={status} />;
  }
  return (
    <>
      {hasLocalDiff ? changes.map((change) => change.diff !== null ? (
        <div className="tl-change-diff" key={change.path}>
          <p className="tl-meta">局部差异：{change.path}</p>
          <LocalEditDiff path={change.path} diff={change.diff} />
        </div>
      ) : null) : null}
      {kind === "write" && content !== null ? (
        <div className="tl-change-content" key={`content:${contentPath}`}>
          <p className="tl-meta">文件内容：{contentPath}</p>
          <CodeBlock code={content} language={contentPath} />
        </div>
      ) : null}
      {changes.length > 0 ? <p className="tl-meta">{changeSummary(kind, changes)}</p> : null}
      {changes.length > 0 ? (
        <div className="tl-change-ratios">
          {changes.map((change) => <ChangeRatio key={`ratio:${change.path}`} change={change} />)}
        </div>
      ) : null}
      <ToolResultView item={item} includeSuccess={false} />
      <ToolMeta item={item} status={status} />
    </>
  );
}

function TodoToolDetails({ item, status }: { item: ToolItem; status: string }) {
  const entries = todoEntriesFor(item);
  if (entries === null) {
    return <GenericToolDetails item={item} echo={commandEchoOf(item.arguments)} status={status} />;
  }
  return (
    <>
      <TodoProgress entries={entries} />
      {entries.length === 0 ? <p className="tl-meta">任务清单已清空</p> : (
        <ul className="tl-todos">
          {entries.map((entry, index) => (
            <li className={`tl-todo status-${entry.status}`} key={`${index}:${entry.title}`}>
              <span className="tl-todo-status">{todoStatusText(entry.status)}</span>
              <span>{entry.title}</span>
            </li>
          ))}
        </ul>
      )}
      <ToolResultView item={item} includeSuccess={false} />
      <ToolMeta item={item} status={status} />
    </>
  );
}

function Tool({ sessionId, item }: { sessionId: string; item: ToolItem }) {
  const [open, toggle] = useLineExpanded(sessionId, item.id, false);
  const status = toolStatusText(item);
  const echo = commandEchoOf(item.arguments);
  const presentation = toolPresentation(item.name, item.arguments);
  const toolName = normalizedToolName(item.name);
  const isTodoList = toolName === "todolist" || toolName === "settodolist";
  const todoEntries = isTodoList ? todoEntriesFor(item) : null;
  const completedTodos = todoEntries?.filter((entry) => entry.status === "done").length ?? 0;
  const summary = [
    todoEntries ? `${completedTodos}/${todoEntries.length} 已完成` : null,
    presentation.target,
    presentation.secondary,
  ].filter(Boolean).join(" · ");
  const heading = summary === ""
    ? presentation.action
    : `${presentation.action} ${summary}`;
  return (
    <Line
      className={`tool-line status-${item.status}`}
      label={`${heading}，状态：${status}`}
      open={open}
      onToggle={toggle}
      head={
        <>
          {resultIcon(item.status)}
          {toolIconView(presentation.icon)}
          <span className="tl-name">{presentation.action}</span>
          {presentation.target ? <span className="tl-mono">{presentation.target}</span> : null}
          {presentation.secondary ? <span className="tl-subtle">{presentation.secondary}</span> : null}
          {todoEntries ? <TodoProgress entries={todoEntries} compact /> : null}
          {lineCaret()}
        </>
      }
    >
      <div className="tl-detail">
        {toolName === "skill" ? (
          <SkillToolDetails item={item} status={status} />
        ) : toolName === "edit" ? (
          <FileToolDetails item={item} status={status} kind="edit" />
        ) : toolName === "write" ? (
          <FileToolDetails item={item} status={status} kind="write" />
        ) : isTodoList ? (
          <TodoToolDetails item={item} status={status} />
        ) : (
          <GenericToolDetails item={item} echo={echo} status={status} />
        )}
        {item.output ? (
          <FullOutput sessionId={sessionId} itemId={item.id} outputRef={item.output} />
        ) : null}
      </div>
    </Line>
  );
}

/** Adjacent, completed reads are one user-visible operation until expanded. */
export function readGroupStatusForItems(items: ToolItem[]): { status: ToolStatus; text: string } {
  if (items.some((item) => item.status === "failed")) {
    return { status: "failed", text: "部分失败" };
  }
  if (items.some((item) => item.status !== "finished")) {
    return { status: "running", text: "读取中" };
  }
  return { status: "finished", text: "已完成" };
}

export function ReadGroup({ sessionId, items }: { sessionId: string; items: ToolItem[] }) {
  const groupId = `read-group:${items.map((item) => item.id).join("|")}`;
  const [open, toggle] = useLineExpanded(sessionId, groupId, false);
  const count = items.length;
  const groupStatus = readGroupStatusForItems(items);
  return (
    <Line
      className={`tool-line kind-read-group status-${groupStatus.status}`}
      label={`连续读取 ${count} 个文件，状态：${groupStatus.text}`}
      open={open}
      onToggle={toggle}
      head={
        <>
          {resultIcon(groupStatus.status)}
          {toolIconView("file")}
          <span className="tl-name">连续读取</span>
          <span className="tl-mono">{count} 个文件</span>
          {lineCaret()}
        </>
      }
    >
      <div className="tl-detail">
        {items.map((item) => <Tool key={item.id} sessionId={sessionId} item={item} />)}
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
          {resultIcon(item.status)}
          {toolIconView("terminal")}
          <span className="tl-name">运行</span>
          <span className="tl-mono">{item.command}</span>
          {lineCaret()}
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
          {resultIcon(item.finished ? "finished" : "running")}
          {toolIconView("terminal")}
          <span className="tl-name">启动终端</span>
          <span className="tl-mono">{item.command ?? item.handle}</span>
          {lineCaret()}
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
          {resultIcon(item.finished ? "finished" : "running")}
          <span className="tl-ic">
            <Workflow size={13} aria-hidden />
          </span>
          <span className="tl-name">{workflow.title}</span>
          <span className="tl-mono">{workflow.latest_log_summary ?? ""}</span>
          {lineCaret()}
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

/** Status pill used by the agent detail panel: text plus an icon. */
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

function agentResultStatus(state: string): ToolStatus {
  switch (state) {
    case "running":
      return "running";
    case "queued":
      return "queued";
    case "completed":
      return "finished";
    case "failed":
    case "cancelled":
    case "aborted":
    case "timed_out":
      return "failed";
    default:
      return "running";
  }
}

function agentResultIcon(state: string) {
  return resultIcon(agentResultStatus(state));
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
    <div
      className={`line agent-line state-${agent.state} status-${agentResultStatus(agent.state)} ${className}`}
    >
      <button
        type="button"
        className="line-head"
        aria-label={`查看子代理详情：${title}，状态：${status}`}
        onClick={() => actions.openAgentPanel(sessionId, agent)}
      >
        {agentResultIcon(agent.state)}
        {agent.state === "running" ? <span className="pulse-dot" aria-hidden /> : null}
        <span className="tl-ic">
          <Bot size={13} aria-hidden />
        </span>
        <span className="tl-name">{title}</span>
        <span className="tl-mono">{agent.latest_text ?? ""}</span>
        {elapsed !== null ? (
          <span className="line-tail">
            <span className="agent-elapsed">{elapsed}</span>
          </span>
        ) : null}
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
      className={`swarm-block state-${swarm.state} status-${agentResultStatus(swarm.state)}`}
      label={`并行子代理 ${swarm.description}，状态：${agentStateText(swarm.state)}，完成 ${aggregate.completed}/${aggregate.total}`}
      open={open}
      onToggle={toggle}
      head={
        <>
          {agentResultIcon(swarm.state)}
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
            <span className="swarm-count" role="status">
              完成 {aggregate.completed}/{aggregate.total}
            </span>
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
