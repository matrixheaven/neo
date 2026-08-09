/**
 * Compact collapsible bar used by thinking, tools, terminals, workflows,
 * delegate items and unknown records. The summary line always carries the
 * real state in text (never color alone); the expanded body shows details.
 */

import { ChevronDown, ChevronRight } from "lucide-react";
import type { ReactNode } from "react";
import { useAppActions, useAppState } from "../state/store";

export function CollapsibleBar({
  sessionId,
  itemId,
  icon,
  title,
  status,
  defaultExpanded = false,
  children,
  className = "",
}: {
  sessionId: string;
  itemId: string;
  icon: ReactNode;
  title: string;
  status: string;
  defaultExpanded?: boolean;
  children: ReactNode;
  className?: string;
}) {
  const state = useAppState();
  const actions = useAppActions();
  const expanded = state.sessions[sessionId]?.expandedItemIds.includes(itemId) ?? defaultExpanded;
  const label = expanded ? `收起${title}` : `展开${title}`;
  return (
    <div className={`collapsible ${className} ${expanded ? "expanded" : ""}`}>
      <button
        type="button"
        className="collapsible-summary"
        aria-expanded={expanded}
        aria-label={`${label}，状态：${status}`}
        onClick={() => actions.toggleItemExpanded(sessionId, itemId)}
      >
        <span className="collapsible-chevron" aria-hidden>
          {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        </span>
        <span className="collapsible-icon" aria-hidden>
          {icon}
        </span>
        <span className="collapsible-title">{title}</span>
        <span className="collapsible-status">{status}</span>
      </button>
      {expanded ? <div className="collapsible-body">{children}</div> : null}
    </div>
  );
}
