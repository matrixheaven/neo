/**
 * Floating task list projected from the latest TodoUpdated event. Anchored
 * directly above the composer at the same width; grows upward; read-only;
 * hidden entirely when there are no tasks.
 */

import { CheckCircle2, ChevronDown, ChevronUp, Circle, Loader2 } from "lucide-react";
import { useState } from "react";
import type { TodoEventData } from "../protocol";

function todoIcon(status: string) {
  switch (status) {
    case "done":
      return <CheckCircle2 size={14} aria-hidden />;
    case "in_progress":
      return <Loader2 size={14} className="spin" aria-hidden />;
    default:
      return <Circle size={14} aria-hidden />;
  }
}

function todoStatusText(status: string): string {
  switch (status) {
    case "done":
      return "已完成";
    case "in_progress":
      return "进行中";
    default:
      return "待处理";
  }
}

export function TaskList({ todos }: { todos: TodoEventData[] }) {
  const [expanded, setExpanded] = useState(false);
  if (todos.length === 0) return null;
  const done = todos.filter((todo) => todo.status === "done").length;
  const current = todos.find((todo) => todo.status === "in_progress");
  return (
    <div className={`task-list ${expanded ? "expanded" : ""}`}>
      <button
        type="button"
        className="task-list-summary"
        aria-expanded={expanded}
        aria-label={expanded ? "收起任务清单" : "展开任务清单"}
        onClick={() => setExpanded((value) => !value)}
      >
        <span className="task-progress">
          任务 {done}/{todos.length}
        </span>
        {current ? <span className="task-current">{current.title}</span> : null}
        <span aria-hidden>{expanded ? <ChevronDown size={14} /> : <ChevronUp size={14} />}</span>
      </button>
      {expanded ? (
        <ul className="task-items">
          {todos.map((todo, index) => (
            <li key={`${index}-${todo.title}`} className={`task-item status-${todo.status}`}>
              {todoIcon(todo.status)}
              <span className="task-title">{todo.title}</span>
              <span className="task-state">{todoStatusText(todo.status)}</span>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
