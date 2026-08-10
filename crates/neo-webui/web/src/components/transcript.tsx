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

function groupTurns(items: TranscriptItem[]): TurnGroup[] {
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
      groups.push({
        kind: "assist",
        process: turn.slice(0, finalIndex),
        activity: [...turn.slice(0, finalIndex), ...turn.slice(finalIndex + 1)],
        msg,
      });
      appendUngrouped(turn.slice(finalIndex + 1));
    }
    turn = [];
  };
  for (const item of items) {
    if (item.kind === "user_message") {
      flushTurn();
      groups.push({ kind: "user", item });
    } else {
      turn.push(item);
    }
  }
  flushTurn();
  return groups;
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
// File-change derivation for the answer footer: the turn's finished
// edit/write tool events carry workspace-relative paths and their content,
// so added/removed line counts are derived locally (no new backend surface).
// Field names mirror the real tool input schemas in neo-agent-core
// (edit: {path, old, new}; write: {path, content}) — events pass the raw
// tool arguments through verbatim.
// ---------------------------------------------------------------------------

export interface FileChange {
  path: string;
  added: number;
  removed: number;
}

const FILE_TOOL_NAMES = new Set(["edit", "write"]);

function countLines(value: unknown): number {
  return typeof value === "string" && value !== "" ? value.split("\n").length : 0;
}

function deriveFileChanges(process: TranscriptItem[]): FileChange[] {
  const byPath = new Map<string, FileChange>();
  for (const item of process) {
    if (item.kind !== "tool" || item.status !== "finished") continue;
    const name = item.name.toLowerCase();
    if (!FILE_TOOL_NAMES.has(name)) continue;
    const args = item.arguments as Record<string, unknown> | undefined;
    if (!args) continue;
    const rawPath = args.path;
    if (typeof rawPath !== "string" || rawPath.trim() === "") continue;
    const entry = byPath.get(rawPath) ?? { path: rawPath, added: 0, removed: 0 };
    if (name === "write") {
      entry.added += countLines(args.content);
    } else {
      entry.added += countLines(args.new);
      entry.removed += countLines(args.old);
    }
    byPath.set(rawPath, entry);
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
  const [open, toggle] = useLineExpanded(sessionId, `fold:${msg.id}`, !msg.finished);
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
          {process.map((item) => (
            <TranscriptItemView key={item.id} sessionId={sessionId} item={item} />
          ))}
        </div>
      </div>
    </div>
  );
}

function AnswerFooter({
  msg,
  changes,
}: {
  msg: AssistantMessageItem;
  changes: FileChange[];
}) {
  const hasAnswer = msg.text.trim() !== "";
  if (!hasAnswer && changes.length === 0) return null;
  return (
    <div className="answer-ft">
      {changes.length > 0 ? (
        <ul className="ft-files">
          {changes.map((change) => (
            <li key={change.path} className="ft-file">
              <span className="ft-path">{change.path}</span>
              <span className="ft-diff">
                {change.added > 0 ? <span className="ft-add">+{change.added}</span> : null}
                {change.removed > 0 ? <span className="ft-del">−{change.removed}</span> : null}
              </span>
            </li>
          ))}
        </ul>
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
      {msg.finished ? <AnswerFooter msg={msg} changes={changes} /> : null}
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
                    {group.items.map((item) => (
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
