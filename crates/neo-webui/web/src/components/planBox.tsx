/**
 * Plan box: bordered panel presenting the plan produced by plan mode
 * (ExitPlanMode), mirroring the TUI plan box. Collapsed it shows only the
 * first lines with a bottom "显示全部" button; expanded it renders the full
 * markdown and offers a small collapse button. Safe markdown only — raw HTML
 * is disabled by the shared Markdown component.
 */

import { ChevronUp } from "lucide-react";
import { useState } from "react";
import { Markdown } from "./markdown";

export const PLAN_PREVIEW_LINES = 10;

/** The collapsed preview when the plan exceeds the preview budget, else
 * `null` when the whole plan already fits. */
export function planPreviewClamp(markdown: string): string | null {
  const lines = markdown.split("\n");
  if (lines.length <= PLAN_PREVIEW_LINES) return null;
  return lines.slice(0, PLAN_PREVIEW_LINES).join("\n");
}

export function planBoxBasename(path: string | null): string {
  const base = path?.split(/[\\/]/).pop() ?? "";
  return base === "" ? "plan" : base;
}

export function PlanBox({
  title,
  markdown,
  path,
}: {
  title?: string | null;
  markdown: string;
  path?: string | null;
}) {
  const [expanded, setExpanded] = useState(false);
  const clamp = planPreviewClamp(markdown);
  const collapsed = clamp !== null && !expanded;
  return (
    <div className="plan-box" data-expanded={expanded || clamp === null}>
      <div className="plan-box-head">
        <span className="plan-box-title">{title ?? `plan: ${planBoxBasename(path ?? null)}`}</span>
        {path ? <span className="plan-box-path tl-mono">{path}</span> : null}
      </div>
      <div className="plan-box-body">
        <Markdown text={collapsed ? (clamp ?? "") : markdown} />
      </div>
      {collapsed ? (
        <button
          type="button"
          className="plan-box-expand"
          aria-label="显示全部"
          onClick={() => setExpanded(true)}
        >
          显示全部
        </button>
      ) : null}
      {clamp !== null && expanded ? (
        <button
          type="button"
          className="plan-box-collapse icon-button"
          aria-label="收起计划"
          title="收起"
          onClick={() => setExpanded(false)}
        >
          <ChevronUp size={13} aria-hidden />
        </button>
      ) : null}
    </div>
  );
}
