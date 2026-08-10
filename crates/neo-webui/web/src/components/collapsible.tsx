/**
 * Single-line fold primitive of the de-carded transcript: a 24-28px head row
 * (caret + icon + title + dim summary + right tail) and a body that expands
 * with the pure-CSS `grid-template-rows 0fr→1fr` transition. Used by think,
 * tool-line, agent-line, swarm-block and the unknown-event record. Expansion
 * state is per-session UI state (never part of the transcript projection).
 */

import type { ReactNode } from "react";
import { useAppActions, useAppState } from "../state/store";

/** Expansion state for one transcript line: absent override follows the
 * line's phase-dependent default; a user click records an explicit override
 * that survives later phase changes (e.g. think finishing). */
export function useLineExpanded(
  sessionId: string,
  itemId: string,
  defaultOpen: boolean,
): [boolean, () => void] {
  const state = useAppState();
  const actions = useAppActions();
  const override = state.sessions[sessionId]?.lineOverrides[itemId];
  const open = override ?? defaultOpen;
  return [open, () => actions.setLineExpanded(sessionId, itemId, !open)];
}

export function Line({
  className = "",
  label,
  open,
  onToggle,
  head,
  children,
}: {
  className?: string;
  /** Accessible name of the fold target, e.g. "思考，状态：思考中". */
  label: string;
  open: boolean;
  onToggle(): void;
  head: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className={`line ${className} ${open ? "open" : ""}`}>
      <button
        type="button"
        className="line-head"
        aria-expanded={open}
        aria-label={`${open ? "收起" : "展开"}${label}`}
        onClick={onToggle}
      >
        {head}
      </button>
      <div className="line-body">
        <div className="line-body-inner">{children}</div>
      </div>
    </div>
  );
}
