/**
 * Transcript scroll pane (redesign §4). Items between one user bubble and the
 * next form one turn. Earlier assistant messages and process rows collapse
 * into a TurnFold; only the final assistant message gets the answer footer.
 *
 * Leaving the bottom disables follow; new events never steal the scroll
 * position. A floating jump-to-latest action appears above the composer
 * while away from the bottom.
 */

import { ArrowDown, ChevronRight, FilePenLine } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useAppActions, useAppState } from "../state/store";
import type {
  AssistantMessageItem,
  TranscriptItem,
  ToolItem,
  UserMessageItem,
} from "../state/transcript";
import { CopyButton } from "./codeBlock";
import { useLineExpanded } from "./collapsible";
import { AssistantBody, ReadGroup, TranscriptItemView } from "./transcriptItems";

// ---------------------------------------------------------------------------
// Turn grouping
// ---------------------------------------------------------------------------

const PROCESS_KINDS = new Set([
  "thinking",
  "tool",
  "shell",
  "terminal",
  "workflow",
  "delegate",
  "swarm",
]);

type TurnGroup =
  | { kind: "user"; item: UserMessageItem }
  | {
      kind: "assist";
      process: TranscriptItem[];
      activity: TranscriptItem[];
      msg: AssistantMessageItem;
    }
  | { kind: "process"; items: TranscriptItem[] }
  | { kind: "inline"; item: TranscriptItem };

function processTurn(item: TranscriptItem): number | null {
  if (!PROCESS_KINDS.has(item.kind) || !("turn" in item) || typeof item.turn !== "number") {
    return null;
  }
  return item.turn;
}

function attachProcessItemsToTheirTurn(groups: TurnGroup[]): TurnGroup[] {
  type AssistGroup = Extract<TurnGroup, { kind: "assist" }>;
  const foldsByTurn = new Map<number, AssistGroup>();
  for (const group of groups) {
    if (group.kind === "assist" && typeof group.msg.turn === "number") {
      foldsByTurn.set(group.msg.turn, group);
    }
  }

  const moveItems = (source: TranscriptItem[], current: TurnGroup) => {
    const remaining: TranscriptItem[] = [];
    const moved = new Set<TranscriptItem>();
    for (const item of source) {
      const turn = processTurn(item);
      const target = turn === null ? undefined : foldsByTurn.get(turn);
      if (!target || target === current) {
        remaining.push(item);
        continue;
      }
      target.process.push(item);
      target.activity.push(item);
      moved.add(item);
    }
    return { remaining, moved };
  };

  for (const group of groups) {
    if (group.kind === "assist") {
      const { remaining, moved } = moveItems(group.process, group);
      if (moved.size > 0) {
        group.process = remaining;
        group.activity = group.activity.filter((item) => !moved.has(item));
      }
    }
    if (group.kind === "process") {
      group.items = moveItems(group.items, group).remaining;
    }
  }
  return groups.filter((group) => group.kind !== "process" || group.items.length > 0);
}

export function groupTurns(items: TranscriptItem[]): TurnGroup[] {
  const groups: TurnGroup[] = [];
  let turn: TranscriptItem[] = [];
  const appendUngrouped = (activity: TranscriptItem[]) => {
    let pending: TranscriptItem[] = [];
    const flushPending = () => {
      if (pending.length > 0) {
        groups.push({ kind: "process", items: pending });
        pending = [];
      }
    };
    for (const item of activity) {
      if (PROCESS_KINDS.has(item.kind)) {
        pending.push(item);
      } else {
        // Approvals, questions, retry and status lines stay in the flow.
        flushPending();
        groups.push({ kind: "inline", item });
      }
    }
    flushPending();
  };
  const flushTurn = () => {
    let finalIndex = -1;
    for (let index = turn.length - 1; index >= 0; index -= 1) {
      if (turn[index]?.kind === "assistant_message") {
        finalIndex = index;
        break;
      }
    }
    if (finalIndex < 0) {
      appendUngrouped(turn);
    } else {
      const msg = turn[finalIndex] as AssistantMessageItem;
      const process = turn.slice(0, finalIndex);
      const trailing = turn.slice(finalIndex + 1);
      groups.push({
        kind: "assist",
        process,
        activity: [...process],
        msg,
      });
      appendUngrouped(trailing);
    }
    turn = [];
  };
  for (const item of items) {
    // A resolved approval remains in the append-only session history, but it
    // is no longer actionable or useful in a replayed conversation.
    if (item.kind === "approval" && item.resolution !== undefined) continue;
    if (item.kind === "user_message") {
      flushTurn();
      groups.push({ kind: "user", item });
    } else {
      turn.push(item);
    }
  }
  flushTurn();
  return attachProcessItemsToTheirTurn(groups);
}

function callId(item: TranscriptItem): string {
  const separator = item.id.indexOf(":");
  return separator < 0 ? item.id : item.id.slice(separator + 1);
}

function commandId(item: TranscriptItem): string | null {
  if (item.kind === "tool") {
    const name = item.name.toLowerCase();
    if (name !== "bash" && name !== "terminal") return null;
  } else if (item.kind !== "shell" && item.kind !== "terminal") {
    return null;
  }
  return "command:" + callId(item);
}

/** Runtime shell/terminal events contain the same execution as their tool
 * call. Keep the richer runtime row and suppress only its paired tool row. */
const DELEGATE_CONTROL_TOOL_NAMES = new Set([
  "delegate",
  "delegategroup",
  "delegateswarm",
  "waitdelegate",
]);

type DelegateCardItem = Extract<TranscriptItem, { kind: "delegate" | "swarm" }>;

function isDelegateCard(item: TranscriptItem): item is DelegateCardItem {
  return item.kind === "delegate" || item.kind === "swarm";
}

function delegateCardMatchesToolTurn(
  item: ToolItem,
  localItems: TranscriptItem[],
  allItems: TranscriptItem[],
): boolean {
  if (typeof item.turn === "number") {
    return allItems.some(
      (candidate) => isDelegateCard(candidate) && candidate.turn === item.turn,
    );
  }
  return localItems.some(
    (candidate) => isDelegateCard(candidate) && candidate.turn === undefined,
  );
}

/** Keep only the rows that add information beyond an existing process card. */
export function presentationItems(
  items: TranscriptItem[],
  allItems: TranscriptItem[] = items,
): TranscriptItem[] {
  return items.filter((item) => {
    if (item.kind !== "tool") return true;
    const name = item.name.toLowerCase();
    if (DELEGATE_CONTROL_TOOL_NAMES.has(name) && delegateCardMatchesToolTurn(item, items, allItems)) {
      return false;
    }
    if (name === "askuserquestion" && (item.status === "finished" || item.status === "running")) {
      return false;
    }
    const runtimeKind = name === "bash" ? "shell" : name === "terminal" ? "terminal" : null;
    const id = commandId(item);
    return runtimeKind === null || id === null || !items.some(
      (candidate) => candidate.kind === runtimeKind && commandId(candidate) === id,
    );
  });
}

export type ProcessPresentationItem =
  | { kind: "item"; item: TranscriptItem }
  | { kind: "read_group"; items: ToolItem[] };

function isActiveReadInTurn(item: TranscriptItem): item is ToolItem {
  return item.kind === "tool" &&
    item.name.trim().toLowerCase() === "read" &&
    (item.status === "running" || item.status === "finished") &&
    typeof item.turn === "number";
}

/**
 * Preserve every process boundary, then fold only direct runs of active or
 * completed Read calls from one explicit turn. This happens before hidden paired rows
 * are removed so an intervening tool can never join two independent reads.
 */
export function processPresentationItems(
  items: TranscriptItem[],
  allItems: TranscriptItem[] = items,
): ProcessPresentationItem[] {
  const visibleItems = new Set(presentationItems(items, allItems));
  const presentation: ProcessPresentationItem[] = [];
  for (let index = 0; index < items.length;) {
    const item = items[index];
    if (!isActiveReadInTurn(item)) {
      if (visibleItems.has(item)) presentation.push({ kind: "item", item });
      index += 1;
      continue;
    }

    const reads = [item];
    let next = index + 1;
    while (next < items.length) {
      const candidate = items[next];
      if (!candidate || !isActiveReadInTurn(candidate) || candidate.turn !== item.turn) break;
      reads.push(candidate);
      next += 1;
    }
    if (reads.length > 1) {
      presentation.push({ kind: "read_group", items: reads });
    } else if (visibleItems.has(item)) {
      presentation.push({ kind: "item", item });
    }
    index = next;
  }
  return presentation;
}

function ProcessRows({
  sessionId,
  items,
  allItems = items,
}: {
  sessionId: string;
  items: TranscriptItem[];
  allItems?: TranscriptItem[];
}) {
  return processPresentationItems(items, allItems).map((item) => {
    if (item.kind === "read_group") {
      return <ReadGroup key={`read-group:${item.items[0].id}`} sessionId={sessionId} items={item.items} />;
    }
    return <TranscriptItemView key={item.item.id} sessionId={sessionId} item={item.item} />;
  });
}

function failedProcessItem(item: TranscriptItem): boolean {
  if (item.kind === "tool" || item.kind === "shell") return item.status === "failed";
  return item.kind === "terminal" && item.finished && typeof item.exitCode === "number" && item.exitCode !== 0;
}

function processSummary(process: TranscriptItem[]): string {
  let searches = 0;
  let reads = 0;
  let edits = 0;
  const commands = new Set<string>();
  const steps = new Set<string>();
  const failures = new Set<string>();

  for (const item of process) {
    if (!PROCESS_KINDS.has(item.kind) && item.kind !== "assistant_message") continue;
    const command = commandId(item);
    const step = command ?? item.id;
    steps.add(step);
    if (command) commands.add(command);
    if (failedProcessItem(item)) failures.add(step);
    if (item.kind !== "tool") continue;
    switch (item.name.toLowerCase()) {
      case "grep":
      case "find":
      case "glob":
        searches += 1;
        break;
      case "read":
      case "list":
        reads += 1;
        break;
      case "edit":
      case "write":
        edits += 1;
        break;
    }
  }

  const parts = [
    searches > 0 ? "搜索 " + searches : null,
    reads > 0 ? "读取 " + reads : null,
    edits > 0 ? "编辑 " + edits : null,
    commands.size > 0 ? "命令 " + commands.size : null,
    failures.size > 0 ? "失败 " + failures.size : null,
    steps.size + " 个步骤",
  ];
  return parts.filter((part): part is string => part !== null).join(" · ");
}

// ---------------------------------------------------------------------------
// File-change derivation for the answer footer. The completed tool result is
// the source of truth: it records which writes reached disk, exact line
// counts and preview data. Tool input is never used to guess a change, so
// cancelled and failed writes cannot appear as edits.
// ---------------------------------------------------------------------------

interface FilePreviewLine {
  content: string;
  kind: "add" | "del" | "context" | "separator" | "created";
}

interface FileChange {
  path: string;
  added: number;
  removed: number;
  preview: FilePreviewLine[];
  previewOmitted: boolean;
  hasPreview: boolean;
  created: boolean;
}

const COMMITTED_FILE_CHANGE_STATUSES = new Set(["committed", "committed_unsynced"]);
const INITIAL_FILE_CHANGE_COUNT = 3;
const HOVER_FILE_PREVIEW_LINE_COUNT = 6;
const EXPANDED_FILE_PREVIEW_LINE_COUNT = 28;

function objectValue(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function countFromResult(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) && value > 0
    ? Math.floor(value)
    : 0;
}

function splitPreviewLines(value: string): string[] {
  const lines = value.split(/\r?\n/);
  if (lines[lines.length - 1] === "") lines.pop();
  return lines;
}

function diffLineKind(line: string): FilePreviewLine["kind"] {
  if (line.startsWith("+")) return "add";
  if (line.startsWith("-")) return "del";
  return "context";
}

function diffHeaderMatches(line: string, marker: "---" | "+++", path: string): boolean {
  if (!line.startsWith(`${marker} `)) return false;
  const headerPath = line.slice(marker.length + 1).trim().split("\t", 1)[0] ?? "";
  const normalizedPath = path.split("\\").join("/");
  return headerPath === "/dev/null" || headerPath === normalizedPath ||
    headerPath === `a/${normalizedPath}` || headerPath === `b/${normalizedPath}`;
}

function localDiffPreviewLines(path: string, diff: string): FilePreviewLine[] {
  const sourceLines = splitPreviewLines(diff);
  const hasFileHeaders = sourceLines.length >= 3 &&
    diffHeaderMatches(sourceLines[0] ?? "", "---", path) &&
    diffHeaderMatches(sourceLines[1] ?? "", "+++", path) &&
    sourceLines[2]?.startsWith("@@") === true;
  const previewLines = hasFileHeaders ? sourceLines.slice(2) : sourceLines;
  const lines: FilePreviewLine[] = [];
  for (const line of previewLines) {
    if (line.startsWith("@@")) {
      lines.push({ content: "...", kind: "separator" });
    } else {
      lines.push({ content: line, kind: diffLineKind(line) });
    }
  }
  return lines;
}

function createdFilePreviewLines(content: string): FilePreviewLine[] {
  return splitPreviewLines(content).map((line) => ({ content: line, kind: "created" }));
}

function appendPreview(entry: FileChange, lines: FilePreviewLine[]): void {
  const remaining = EXPANDED_FILE_PREVIEW_LINE_COUNT - entry.preview.length;
  if (remaining <= 0) {
    entry.previewOmitted ||= lines.length > 0;
    return;
  }
  entry.preview.push(...lines.slice(0, remaining));
  entry.previewOmitted ||= lines.length > remaining;
}

function deriveFileChanges(process: TranscriptItem[]): FileChange[] {
  const byPath = new Map<string, FileChange>();
  for (const item of process) {
    if (item.kind !== "tool") continue;
    const details = objectValue(item.result?.details);
    const changes = details?.changes;
    if (!Array.isArray(changes)) continue;
    for (const rawChange of changes) {
      const change = objectValue(rawChange);
      if (
        !change ||
        typeof change.status !== "string" ||
        !COMMITTED_FILE_CHANGE_STATUSES.has(change.status)
      ) continue;
      const path = change.path;
      if (typeof path !== "string" || path.trim() === "") continue;
      const entry = byPath.get(path) ?? {
        path,
        added: 0,
        removed: 0,
        preview: [],
        previewOmitted: false,
        hasPreview: false,
        created: false,
      };
      entry.added += countFromResult(change.added);
      entry.removed += countFromResult(change.removed);
      if (change.operation === "created") {
        entry.created = true;
        entry.hasPreview = true;
        if (typeof change.content === "string") {
          appendPreview(entry, createdFilePreviewLines(change.content));
        }
      } else if (typeof change.diff === "string" && change.diff !== "") {
        entry.hasPreview = true;
        appendPreview(entry, localDiffPreviewLines(path, change.diff));
      }
      byPath.set(path, entry);
    }
  }
  return [...byPath.values()];
}

/** Best available wall-clock hint for a fold: the slowest delegate/swarm
 * elapsed time, when one was reported on the wire. */
function foldElapsedSecs(process: TranscriptItem[]): number | null {
  let secs = 0;
  for (const item of process) {
    if (item.kind === "delegate") {
      secs = Math.max(secs, item.agent.elapsed?.secs ?? 0);
    }
    if (item.kind === "swarm") {
      for (const child of item.swarm.children) {
        secs = Math.max(secs, child.agent.elapsed?.secs ?? 0);
      }
    }
  }
  return secs > 0 ? secs : null;
}

// ---------------------------------------------------------------------------
// TurnFold + answer footer
// ---------------------------------------------------------------------------

function TurnFold({
  sessionId,
  msg,
  process,
  activity,
  allItems,
}: {
  sessionId: string;
  msg: AssistantMessageItem;
  process: TranscriptItem[];
  activity: TranscriptItem[];
  allItems: TranscriptItem[];
}) {
  // All completed activity stays behind its summary. In-progress turns remain
  // open so streaming progress is visible; an explicit choice wins later.
  const [open, toggle] = useLineExpanded(
    sessionId,
    `fold:${msg.id}`,
    !msg.finished,
  );
  const secs = foldElapsedSecs(activity);
  const detail = processSummary(activity);
  const summary = msg.finished
    ? (secs !== null ? "工作了 " + secs + "s · " : "") + detail
    : "工作中 · " + detail;
  return (
    <div className={`turn-fold ${open ? "open" : ""}`}>
      <button
        type="button"
        className="tf-head"
        aria-expanded={open}
        aria-label={`${open ? "收起" : "展开"}工作过程（${summary}）`}
        disabled={!msg.finished}
        onClick={toggle}
      >
        <span className="tf-caret" aria-hidden>
          <ChevronRight size={13} />
        </span>
        <span className="tf-sum">{summary}</span>
      </button>
      <div className="tf-body">
        <div className="tf-body-inner">
          <ProcessRows sessionId={sessionId} items={process} allItems={allItems} />
        </div>
      </div>
    </div>
  );
}

function LocalPreview({ change, expanded }: { change: FileChange; expanded: boolean }) {
  const lineCount = expanded ? EXPANDED_FILE_PREVIEW_LINE_COUNT : HOVER_FILE_PREVIEW_LINE_COUNT;
  const lines = change.preview.slice(0, lineCount);
  const omitted = change.previewOmitted || change.preview.length > lines.length;
  return (
    <pre className="ft-local-diff">
      {lines.map((line, index) => (
        <span className={`ft-preview-line ft-diff-${line.kind}`} key={`${index}:${line.content}`}>
          <span className="ft-line-no" aria-hidden>{index + 1}</span>
          <span className="ft-line-gutter" aria-hidden />
          <span className="ft-line-content">{line.content || " "}</span>
        </span>
      ))}
      {lines.length === 0 ? (
        <span className="ft-preview-line ft-diff-empty">
          <span className="ft-line-no" aria-hidden>·</span>
          <span className="ft-line-gutter" aria-hidden />
          <span className="ft-line-content">{change.created ? "新建空文件" : "没有可显示的局部内容"}</span>
        </span>
      ) : null}
      {omitted ? (
        <span className="ft-preview-omitted">其余内容未显示</span>
      ) : null}
    </pre>
  );
}

function FilePreviewHeader({ change }: { change: FileChange }) {
  return (
    <div className="ft-preview-header">
      <span className="ft-preview-path" aria-label={change.path}>{change.path}</span>
      <span className="ft-diff" aria-label={`新增 ${change.added} 行，删除 ${change.removed} 行`}>
        {change.added > 0 ? <span className="ft-add">+{change.added}</span> : null}
        {change.removed > 0 ? <span className="ft-del">−{change.removed}</span> : null}
      </span>
    </div>
  );
}

function filePreviewId(messageId: string, path: string): string {
  return `file-preview-${encodeURIComponent(`${messageId}\u0000${path}`)}`;
}

function filePathParts(path: string): { directory: string | null; basename: string } {
  const separator = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  if (separator < 0) return { directory: null, basename: path };
  return { directory: path.slice(0, separator + 1), basename: path.slice(separator + 1) };
}

function SummaryChangeRatio({ added, removed }: { added: number; removed: number }) {
  const total = added + removed;
  const addedPercent = total === 0 ? 0 : (added / total) * 100;
  const removedPercent = total === 0 ? 0 : (removed / total) * 100;
  return (
    <span className="ft-summary-ratio" role="img" aria-label={`新增 ${added} 行，删除 ${removed} 行`}>
      <span className="ft-summary-ratio-add" style={{ width: `${addedPercent}%` }} />
      <span className="ft-summary-ratio-del" style={{ width: `${removedPercent}%` }} />
    </span>
  );
}

function FileChangeRow({
  sessionId,
  messageId,
  change,
}: {
  sessionId: string;
  messageId: string;
  change: FileChange;
}) {
  const [hovering, setHovering] = useState(false);
  const rowRef = useRef<HTMLLIElement>(null);
  const scrollAfterOpenRef = useRef(false);
  const hasPreview = change.hasPreview;
  const path = filePathParts(change.path);
  const [open, toggle] = useLineExpanded(sessionId, `file:${messageId}:${change.path}`, false);
  const previewId = filePreviewId(messageId, change.path);
  const label = `${open ? "收起" : "展开"} ${change.path} 的${change.created ? "新建文件内容" : "局部差异"}`;
  const previewVisible = open || hovering;

  // Hover previews are intentionally passive. Only an explicit expansion
  // requests room above the fixed composer.
  useLayoutEffect(() => {
    if (!open || !scrollAfterOpenRef.current) return;
    scrollAfterOpenRef.current = false;
    const row = rowRef.current;
    if (typeof row?.scrollIntoView === "function") {
      row.scrollIntoView({ block: "nearest" });
    }
  }, [open]);

  const togglePreview = () => {
    if (!open) scrollAfterOpenRef.current = true;
    toggle();
  };

  const contents = (
    <>
      {hasPreview ? <ChevronRight className={`ft-file-caret ${open ? "open" : ""}`} size={13} aria-hidden /> : null}
      <span className="ft-path" title={change.path}>
        {path.directory ? <span className="ft-directory">{path.directory}</span> : null}
        <span className="ft-basename">{path.basename}</span>
      </span>
      <span className="ft-diff">
        {change.added > 0 ? <span className="ft-add">+{change.added}</span> : null}
        {change.removed > 0 ? <span className="ft-del">−{change.removed}</span> : null}
      </span>
    </>
  );
  return (
    <li
      ref={rowRef}
      className={`ft-file ${hasPreview ? "has-preview" : ""}`}
      onMouseEnter={hasPreview ? () => setHovering(true) : undefined}
      onMouseLeave={hasPreview ? () => setHovering(false) : undefined}
    >
      {hasPreview ? (
        <button
          type="button"
          className="ft-file-head"
          aria-controls={previewId}
          aria-expanded={open}
          aria-label={label}
          onFocus={hasPreview ? () => setHovering(true) : undefined}
          onBlur={hasPreview ? () => setHovering(false) : undefined}
          onClick={togglePreview}
        >
          {contents}
        </button>
      ) : (
        <div className="ft-file-head">{contents}</div>
      )}
      {hasPreview ? (
        <div
          id={previewId}
          className={`ft-file-preview ${open ? "open" : ""}`}
          hidden={!previewVisible}
          role="region"
          aria-label={`${change.path} 的${change.created ? "文件内容" : "局部差异"}`}
        >
          <FilePreviewHeader change={change} />
          <LocalPreview change={change} expanded={open} />
        </div>
      ) : null}
    </li>
  );
}

function AnswerFooter({
  sessionId,
  msg,
  changes,
}: {
  sessionId: string;
  msg: AssistantMessageItem;
  changes: FileChange[];
}) {
  const hasAnswer = msg.text.trim() !== "";
  const [showAll, toggleShowAll] = useLineExpanded(sessionId, `files:${msg.id}`, false);
  if (!hasAnswer && changes.length === 0) return null;
  const visibleChanges = showAll ? changes : changes.slice(0, INITIAL_FILE_CHANGE_COUNT);
  const hiddenCount = changes.length - visibleChanges.length;
  const totalAdded = changes.reduce((total, change) => total + change.added, 0);
  const totalRemoved = changes.reduce((total, change) => total + change.removed, 0);
  const fileListId = `file-list:${msg.id}`;
  return (
    <div className="answer-ft">
      {changes.length > 0 ? (
        <div className="ft-changes">
          <div className="ft-summary">
            <FilePenLine className="ft-summary-icon" size={14} aria-hidden />
            <span className="ft-summary-title">已编辑 {changes.length} 个文件</span>
            {totalAdded > 0 ? <span className="ft-add">+{totalAdded}</span> : null}
            {totalRemoved > 0 ? <span className="ft-del">−{totalRemoved}</span> : null}
            <SummaryChangeRatio added={totalAdded} removed={totalRemoved} />
          </div>
          <ul className="ft-files" id={fileListId}>
            {visibleChanges.map((change) => (
              <FileChangeRow key={change.path} sessionId={sessionId} messageId={msg.id} change={change} />
            ))}
          </ul>
          {hiddenCount > 0 ? (
            <button
              type="button"
              className="ft-more"
              aria-controls={fileListId}
              aria-expanded={showAll}
              onClick={toggleShowAll}
            >
              显示其余 {hiddenCount} 个文件
            </button>
          ) : showAll && changes.length > INITIAL_FILE_CHANGE_COUNT ? (
            <button
              type="button"
              className="ft-more"
              aria-controls={fileListId}
              aria-expanded={showAll}
              onClick={toggleShowAll}
            >
              收起其余文件
            </button>
          ) : null}
        </div>
      ) : null}
      {hasAnswer ? <CopyButton text={msg.text} label="复制回答" /> : null}
    </div>
  );
}

function AssistGroup({
  sessionId,
  group,
  allItems,
}: {
  sessionId: string;
  group: { process: TranscriptItem[]; activity: TranscriptItem[]; msg: AssistantMessageItem };
  allItems: TranscriptItem[];
}) {
  const { process, activity, msg } = group;
  const changes = msg.finished ? deriveFileChanges(activity) : [];
  return (
    <div className="a-msg t-item">
      {process.length > 0 ? (
        <TurnFold sessionId={sessionId} msg={msg} process={process} activity={activity} allItems={allItems} />
      ) : null}
      <AssistantBody item={msg} />
      {msg.finished ? <AnswerFooter sessionId={sessionId} msg={msg} changes={changes} /> : null}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Scroll pane
// ---------------------------------------------------------------------------

export function TranscriptPane({ sessionId }: { sessionId: string }) {
  const state = useAppState();
  const actions = useAppActions();
  const view = state.sessions[sessionId];
  const items = view?.projection.items ?? [];
  const isAtBottom = view?.isAtBottom ?? true;
  const scrollRef = useRef<HTMLDivElement | null>(null);

  const scrollToBottom = (behavior: ScrollBehavior) => {
    const element = scrollRef.current;
    if (element && typeof element.scrollTo === "function") {
      element.scrollTo({ top: element.scrollHeight, behavior });
    }
  };

  // Restore follow when at bottom: keep pinned to the latest content.
  useLayoutEffect(() => {
    if (isAtBottom) {
      scrollToBottom("auto");
    }
  }, [items, isAtBottom]);

  // Switching sessions restores the bottom anchor.
  useEffect(() => {
    if (isAtBottom) {
      scrollToBottom("auto");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  const onScroll = () => {
    const element = scrollRef.current;
    if (!element) return;
    const distance = element.scrollHeight - element.scrollTop - element.clientHeight;
    const atBottom = distance < 24;
    if (atBottom !== isAtBottom) {
      actions.setAtBottom(sessionId, atBottom);
    }
  };

  const groups = groupTurns(items);

  return (
    <>
      <div
        className="transcript-scroll"
        ref={scrollRef}
        onScroll={onScroll}
        aria-label="会话转录"
      >
        <div className="transcript-column">
          {groups.map((group) => {
            switch (group.kind) {
              case "user":
                return (
                  <TranscriptItemView key={group.item.id} sessionId={sessionId} item={group.item} />
                );
              case "assist":
                return <AssistGroup key={group.msg.id} sessionId={sessionId} group={group} allItems={items} />;
              case "process":
                // In-progress process rows without a following message stay
                // visible as-is.
                return (
                  <div className="a-msg t-item" key={group.items[0].id}>
                    <ProcessRows sessionId={sessionId} items={group.items} allItems={items} />
                  </div>
                );
              case "inline":
                return (
                  <div className="t-item" key={group.item.id}>
                    <TranscriptItemView sessionId={sessionId} item={group.item} />
                  </div>
                );
            }
          })}
        </div>
      </div>
      {!isAtBottom ? (
        <button
          type="button"
          className="jump-latest"
          aria-label="回到最新内容"
          title="回到最新内容"
          onClick={() => {
            actions.setAtBottom(sessionId, true);
            scrollToBottom("smooth");
          }}
        >
          <ArrowDown size={16} aria-hidden />
        </button>
      ) : null}
    </>
  );
}
