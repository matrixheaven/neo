/**
 * Transcript scroll pane (redesign §4). Items between one user bubble and the
 * next form one turn. Earlier assistant messages and process rows collapse
 * into a TurnFold; only the final assistant message gets the answer footer.
 *
 * Leaving the bottom disables follow; new events never steal the scroll
 * position. A floating jump-to-latest action appears above the composer
 * while away from the bottom.
 */

import { ArrowDown, ChevronRight } from "lucide-react";
import { useEffect, useLayoutEffect, useRef } from "react";
import { useAppActions, useAppState } from "../state/store";
import type {
  AssistantMessageItem,
  TranscriptItem,
  UserMessageItem,
} from "../state/transcript";
import { CopyButton } from "./codeBlock";
import { useLineExpanded } from "./collapsible";
import { AssistantBody, TranscriptItemView } from "./transcriptItems";

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
function presentationItems(items: TranscriptItem[]): TranscriptItem[] {
  return items.filter((item) => {
    if (item.kind !== "tool") return true;
    const name = item.name.toLowerCase();
    const runtimeKind = name === "bash" ? "shell" : name === "terminal" ? "terminal" : null;
    const id = commandId(item);
    return runtimeKind === null || id === null || !items.some(
      (candidate) => candidate.kind === runtimeKind && commandId(candidate) === id,
    );
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
// counts and a bounded unified diff. Tool input is never used to guess a
// change, so cancelled and failed writes cannot appear as edits.
// ---------------------------------------------------------------------------

export interface FileChange {
  path: string;
  added: number;
  removed: number;
  diffs: string[];
}

const COMMITTED_FILE_CHANGE_STATUSES = new Set(["committed", "committed_unsynced"]);
const INITIAL_FILE_CHANGE_COUNT = 3;
const DIFF_PREVIEW_LINE_COUNT = 24;

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
      const entry = byPath.get(path) ?? { path, added: 0, removed: 0, diffs: [] };
      entry.added += countFromResult(change.added);
      entry.removed += countFromResult(change.removed);
      if (typeof change.diff === "string" && change.diff !== "") {
        entry.diffs.push(change.diff);
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
}: {
  sessionId: string;
  msg: AssistantMessageItem;
  process: TranscriptItem[];
  activity: TranscriptItem[];
}) {
  // Finished turns default to collapsed behind the summary line; in-progress
  // turns default to open (the head is not a toggle then). An explicit user
  // choice overrides the phase default.
  const displayedProcess = presentationItems(process);
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
          {displayedProcess.map((item) => (
            <TranscriptItemView key={item.id} sessionId={sessionId} item={item} />
          ))}
        </div>
      </div>
    </div>
  );
}

function diffLineKind(line: string): "add" | "del" | "hunk" | "context" {
  if (line.startsWith("+") && !line.startsWith("+++")) return "add";
  if (line.startsWith("-") && !line.startsWith("---")) return "del";
  if (line.startsWith("@@")) return "hunk";
  return "context";
}

function LocalDiff({ diff }: { diff: string }) {
  const allLines = diff.split("\n");
  if (allLines[allLines.length - 1] === "") allLines.pop();
  const lines = allLines.slice(0, DIFF_PREVIEW_LINE_COUNT);
  const omitted = allLines.length > lines.length;
  return (
    <pre className="ft-local-diff">
      {lines.map((line, index) => (
        <span className={`ft-diff-${diffLineKind(line)}`} key={`${index}:${line}`}>
          {line || " "}
        </span>
      ))}
      {omitted ? <span className="ft-diff-omitted">其余差异未显示</span> : null}
    </pre>
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
  const hasDiff = change.diffs.length > 0;
  const [open, toggle] = useLineExpanded(sessionId, `file:${messageId}:${change.path}`, false);
  const label = `${open ? "收起" : "展开"} ${change.path} 的局部差异`;
  const contents = (
    <>
      {hasDiff ? <ChevronRight className={`ft-file-caret ${open ? "open" : ""}`} size={13} aria-hidden /> : null}
      <span className="ft-path" title={change.path}>{change.path}</span>
      <span className="ft-diff">
        {change.added > 0 ? <span className="ft-add">+{change.added}</span> : null}
        {change.removed > 0 ? <span className="ft-del">−{change.removed}</span> : null}
      </span>
    </>
  );
  return (
    <li className={`ft-file ${hasDiff ? "has-diff" : ""}`}>
      {hasDiff ? (
        <button type="button" className="ft-file-head" aria-expanded={open} aria-label={label} onClick={toggle}>
          {contents}
        </button>
      ) : (
        <div className="ft-file-head">{contents}</div>
      )}
      {hasDiff && open ? (
        <div className="ft-file-preview">
          {change.diffs.map((diff, index) => <LocalDiff key={`${index}:${diff}`} diff={diff} />)}
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
  return (
    <div className="answer-ft">
      {changes.length > 0 ? (
        <div className="ft-changes">
          <div className="ft-summary">
            已编辑 {changes.length} 个文件
            {totalAdded > 0 ? <span className="ft-add">+{totalAdded}</span> : null}
            {totalRemoved > 0 ? <span className="ft-del">−{totalRemoved}</span> : null}
          </div>
          <ul className="ft-files">
            {visibleChanges.map((change) => (
              <FileChangeRow key={change.path} sessionId={sessionId} messageId={msg.id} change={change} />
            ))}
          </ul>
          {hiddenCount > 0 ? (
            <button type="button" className="ft-more" onClick={toggleShowAll}>
              显示其余 {hiddenCount} 个文件
            </button>
          ) : showAll && changes.length > INITIAL_FILE_CHANGE_COUNT ? (
            <button type="button" className="ft-more" onClick={toggleShowAll}>
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
}: {
  sessionId: string;
  group: { process: TranscriptItem[]; activity: TranscriptItem[]; msg: AssistantMessageItem };
}) {
  const { process, activity, msg } = group;
  const changes = msg.finished ? deriveFileChanges(activity) : [];
  return (
    <div className="a-msg t-item">
      {process.length > 0 ? (
        <TurnFold sessionId={sessionId} msg={msg} process={process} activity={activity} />
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
                return <AssistGroup key={group.msg.id} sessionId={sessionId} group={group} />;
              case "process":
                // In-progress process rows without a following message stay
                // visible as-is.
                return (
                  <div className="a-msg t-item" key={group.items[0].id}>
                    {presentationItems(group.items).map((item) => (
                      <TranscriptItemView key={item.id} sessionId={sessionId} item={item} />
                    ))}
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
