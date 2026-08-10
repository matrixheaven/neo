/**
 * Floating task list projected from the latest TodoUpdated event. Anchored
 * directly above the composer at the same width; grows upward; read-only;
 * hidden entirely when there are no tasks.
 */

import { Check, ChevronDown, ChevronUp } from "lucide-react";
import { useState } from "react";
import type { TodoEventData } from "../protocol";

function todoIcon(status: string) {
  return status === "done"
    ? <Check size={13} aria-hidden />
    : <span className="task-status-dot" aria-hidden />;
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
              <span
                className="task-status-indicator"
                aria-label={todoStatusText(todo.status)}
                title={todoStatusText(todo.status)}
              >
                {todoIcon(todo.status)}
              </span>
              <span className="task-title">{todo.title}</span>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
