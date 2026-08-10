/**
 * Draggable sidebar separator with keyboard support (ArrowLeft/ArrowRight).
 * Width is the only locally persisted preference.
 *
 * Drag performance (redesign §4.2): pointer movement writes the `--sidebar-w`
 * CSS variable directly, throttled to one write per animation frame, so a long
 * transcript never re-renders React mid-drag. While dragging, the root element
 * carries a `.resizing` class that disables transcript transition animations.
 * The final width is committed (state + localStorage) on pointer up; keyboard
 * adjustments commit immediately.
 */

import { useCallback, useEffect } from "react";
import { SIDEBAR_MAX, SIDEBAR_MIN } from "../state/appState";
import { useAppActions, useAppState } from "../state/store";

const KEY_STEP = 16;

function clampWidth(width: number): number {
  return Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, Math.round(width)));
}

export function SidebarResizer() {
  const state = useAppState();
  const actions = useAppActions();

  // Committed width (keyboard, restored preference) mirrors into the CSS var.
  // During a drag the var is written directly per frame and the commit on
  // pointer-up converges to the same value.
  useEffect(() => {
    document.documentElement.style.setProperty("--sidebar-w", `${state.sidebarWidth}px`);
  }, [state.sidebarWidth]);

  const onPointerDown = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      if (event.button !== 0) return;
      event.preventDefault();
      const drag = {
        startX: event.clientX,
        startWidth: state.sidebarWidth,
        latest: state.sidebarWidth,
        frame: null as number | null,
      };
      document.documentElement.classList.add("resizing");

      const onMove = (move: PointerEvent) => {
        drag.latest = clampWidth(drag.startWidth + (move.clientX - drag.startX));
        if (drag.frame === null) {
          drag.frame = window.requestAnimationFrame(() => {
            drag.frame = null;
            document.documentElement.style.setProperty("--sidebar-w", `${drag.latest}px`);
          });
        }
      };
      const onUp = () => {
        document.removeEventListener("pointermove", onMove);
        document.removeEventListener("pointerup", onUp);
        document.removeEventListener("pointercancel", onUp);
        if (drag.frame !== null) {
          window.cancelAnimationFrame(drag.frame);
        }
        document.documentElement.classList.remove("resizing");
        // Commit once: reducer clamps, the store persists the preference.
        actions.setSidebarWidth(drag.latest);
      };
      document.addEventListener("pointermove", onMove);
      document.addEventListener("pointerup", onUp);
      document.addEventListener("pointercancel", onUp);
    },
    [actions, state.sidebarWidth],
  );

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
      onKeyDown={onKeyDown}
    />
  );
}
