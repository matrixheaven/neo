/**
 * Queue panel (queue & steer parity with the TUI pending-input panel): shows
 * the running turn's pending steers (injected at the next break point) and
 * follow-ups (FIFO, start later turns), mirrored purely from the canonical
 * queue events. Head-of-queue follow-up actions — 立即引导 and 编辑 — ride
 * the queue control API; the panel updates from the emitted queue events.
 */

import { ListStart, Pencil, Zap } from "lucide-react";
import { useAppActions, useAppState } from "../state/store";

function QueueRow({
  text,
  badge,
  icon,
  actions,
}: {
  text: string;
  badge: string;
  icon: React.ReactNode;
  actions?: React.ReactNode;
}) {
  return (
    <li className="queue-row">
      <span className="queue-row-icon" aria-hidden>
        {icon}
      </span>
      <span className="queue-row-badge">{badge}</span>
      <span className="queue-row-text" title={text}>
        {text}
      </span>
      {actions ? <span className="queue-row-actions">{actions}</span> : null}
    </li>
  );
}

export function QueuePanel({ sessionId }: { sessionId: string }) {
  const state = useAppState();
  const actions = useAppActions();
  const view = state.sessions[sessionId];
  if (!view) return null;
  const { pendingSteers, pendingFollowUps } = view.projection;
  if (pendingSteers.length === 0 && pendingFollowUps.length === 0) return null;
  const head = pendingFollowUps[0];
  return (
    <div className="queue-panel" role="region" aria-label="待处理消息队列">
      <ul className="queue-rows">
        {pendingSteers.map((item) => (
          <QueueRow
            key={item.id}
            text={item.text}
            badge="引导"
            icon={<Zap size={13} aria-hidden />}
          />
        ))}
        {pendingFollowUps.map((item, index) => (
          <QueueRow
            key={item.id}
            text={item.text}
            badge="排队"
            icon={<ListStart size={13} aria-hidden />}
            actions={
              index === 0 && head !== undefined ? (
                <>
                  <button
                    type="button"
                    className="icon-button"
                    aria-label={`立即引导：${item.text}`}
                    title="立即引导（下一个断点注入）"
                    onClick={() => actions.promoteFollowUpToSteer()}
                  >
                    <Zap size={13} aria-hidden />
                  </button>
                  <button
                    type="button"
                    className="icon-button"
                    aria-label={`编辑：${item.text}`}
                    title="取回输入框编辑"
                    onClick={() => actions.dequeueFollowUpForEdit(item.text)}
                  >
                    <Pencil size={13} aria-hidden />
                  </button>
                </>
              ) : undefined
            }
          />
        ))}
      </ul>
    </div>
  );
}
