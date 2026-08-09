/**
 * Draggable sidebar separator with keyboard support (ArrowLeft/ArrowRight).
 * Width is the only locally persisted preference.
 */

import { useCallback, useRef } from "react";
import { SIDEBAR_MAX, SIDEBAR_MIN } from "../state/appState";
import { useAppActions, useAppState } from "../state/store";

const KEY_STEP = 16;

export function SidebarResizer() {
  const state = useAppState();
  const actions = useAppActions();
  const dragState = useRef<{ startX: number; startWidth: number } | null>(null);

  const onPointerDown = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      event.preventDefault();
      event.currentTarget.setPointerCapture(event.pointerId);
      dragState.current = { startX: event.clientX, startWidth: state.sidebarWidth };
    },
    [state.sidebarWidth],
  );

  const onPointerMove = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      if (!dragState.current) return;
      const delta = event.clientX - dragState.current.startX;
      actions.setSidebarWidth(dragState.current.startWidth + delta);
    },
    [actions],
  );

  const onPointerUp = useCallback(() => {
    dragState.current = null;
  }, []);

  const onKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      if (event.key === "ArrowLeft") {
        event.preventDefault();
        actions.setSidebarWidth(state.sidebarWidth - KEY_STEP);
      } else if (event.key === "ArrowRight") {
        event.preventDefault();
        actions.setSidebarWidth(state.sidebarWidth + KEY_STEP);
      }
    },
    [actions, state.sidebarWidth],
  );

  return (
    <div
      className="sidebar-resizer"
      role="separator"
      aria-orientation="vertical"
      aria-label="调整会话列表宽度"
      aria-valuenow={state.sidebarWidth}
      aria-valuemin={SIDEBAR_MIN}
      aria-valuemax={SIDEBAR_MAX}
      tabIndex={0}
      title="拖拽或用左右方向键调整宽度"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onKeyDown={onKeyDown}
    />
  );
}
