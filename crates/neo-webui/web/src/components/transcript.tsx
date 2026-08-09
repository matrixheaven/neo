/**
 * Transcript scroll pane. Leaving the bottom disables follow; new events
 * never steal the scroll position. A floating jump-to-latest action appears
 * above the composer while away from the bottom.
 */

import { ArrowDown } from "lucide-react";
import { useEffect, useLayoutEffect, useRef } from "react";
import { useAppActions, useAppState } from "../state/store";
import { TranscriptItemView } from "./transcriptItems";

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

  return (
    <>
      <div
        className="transcript-scroll"
        ref={scrollRef}
        onScroll={onScroll}
        aria-label="会话转录"
      >
        <div className="transcript-column">
          {items.map((item) => (
            <TranscriptItemView key={item.id} sessionId={sessionId} item={item} />
          ))}
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
