/**
 * Floating composer. Enter sends, Shift+Enter inserts a newline, IME
 * composition never sends. New sessions create; idle sessions start a turn;
 * running sessions send follow_up. Stop and steer are separate, distinct
 * actions — never borrow the normal send.
 */

import { ArrowUp, Square, Zap } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type {
  PermissionMode,
  WebUiComposer,
  WebUiDevelopmentMode,
} from "../protocol";
import { useAppActions, useAppState } from "../state/store";
import { TaskList } from "./taskList";

const MODE_LABELS: Record<WebUiDevelopmentMode, string> = {
  normal: "普通",
  plan: "计划",
  goal: "目标",
};

const PERMISSION_LABELS: Record<PermissionMode, string> = {
  ask: "逐条确认",
  auto: "自动",
  yolo: "免确认",
};

export function Composer({ centered }: { centered: boolean }) {
  const state = useAppState();
  const actions = useAppActions();
  const sessionId = state.selectedSessionId;
  const view = sessionId !== null ? state.sessions[sessionId] : undefined;
  const [localDraft, setLocalDraft] = useState("");
  const draft = sessionId !== null ? (view?.draft ?? "") : localDraft;
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const composingRef = useRef(false);

  const bootstrap = state.bootstrap;
  const running =
    view !== undefined &&
    (view.phase === "running" || view.phase === "starting" || view.phase === "finishing") &&
    view.currentTurnId !== null;
  const sending = sessionId === null ? state.creatingSession : (view?.sending ?? false);

  // Per-next-turn overrides; never written back to global settings.
  const [model, setModel] = useState("");
  const [permissionMode, setPermissionMode] = useState("");
  const [developmentMode, setDevelopmentMode] = useState("");

  useEffect(() => {
    // New session starts focus on the composer.
    if (centered) textareaRef.current?.focus();
  }, [centered, sessionId]);

  // A successful create selects the new session; only then is the local
  // draft cleared. Failures (409/413/network) keep it intact.
  useEffect(() => {
    if (sessionId !== null) {
      setLocalDraft("");
    }
  }, [sessionId]);

  const autosize = () => {
    const element = textareaRef.current;
    if (!element) return;
    element.style.height = "auto";
    element.style.height = `${Math.min(180, Math.max(52, element.scrollHeight))}px`;
  };

  useEffect(autosize, [draft]);

  const composerOverrides = (): WebUiComposer | undefined => {
    const composer: WebUiComposer = {};
    if (model !== "") composer.model = model;
    if (permissionMode !== "") composer.permission_mode = permissionMode as PermissionMode;
    if (developmentMode !== "") {
      composer.development_mode = developmentMode as WebUiDevelopmentMode;
    }
    return Object.keys(composer).length > 0 ? composer : undefined;
  };

  const setDraft = (text: string) => {
    if (sessionId !== null) {
      actions.setDraft(sessionId, text);
    } else {
      setLocalDraft(text);
    }
  };

  const submit = () => {
    const text = draft.trim();
    if (text === "" || sending) return;
    actions.sendMessage(text, composerOverrides());
  };

  const onKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Enter" && !event.shiftKey && !composingRef.current && !event.nativeEvent.isComposing) {
      event.preventDefault();
      submit();
    }
  };

  const todos = view?.projection.todos ?? [];
  const canSteer = running; // steer is part of the final input protocol
  const showToolRow =
    (bootstrap?.models?.length ?? 0) > 0 ||
    (bootstrap?.permission_modes?.length ?? 0) > 0 ||
    (bootstrap?.development_modes?.length ?? 0) > 0;

  return (
    <div className={`composer-dock ${centered ? "centered" : ""}`}>
      {!centered && sessionId !== null ? <TaskList todos={todos} /> : null}
      <div className="composer" data-centered={centered}>
        <textarea
          ref={textareaRef}
          className="composer-input"
          aria-label="输入消息"
          placeholder={running ? "输入后续消息…" : "输入消息…"}
          value={draft}
          rows={1}
          onChange={(event) => {
            setDraft(event.target.value);
          }}
          onKeyDown={onKeyDown}
          onCompositionStart={() => {
            composingRef.current = true;
          }}
          onCompositionEnd={() => {
            composingRef.current = false;
          }}
        />
        <div className="composer-footer">
          <div className="composer-tools">
            {showToolRow ? (
              <>
                {(bootstrap?.models?.length ?? 0) > 0 ? (
                  <select
                    className="composer-select"
                    aria-label="模型（仅下一回合）"
                    value={model}
                    onChange={(event) => setModel(event.target.value)}
                  >
                    <option value="">默认模型</option>
                    {bootstrap?.models?.map((entry) => (
                      <option key={entry.alias} value={entry.alias}>
                        {entry.alias}
                      </option>
                    ))}
                  </select>
                ) : null}
                {(bootstrap?.permission_modes?.length ?? 0) > 0 ? (
                  <select
                    className="composer-select"
                    aria-label="逐条确认（仅下一回合）"
                    value={permissionMode}
                    onChange={(event) => setPermissionMode(event.target.value)}
                  >
                    <option value="">默认确认</option>
                    {bootstrap?.permission_modes?.map((entry) => (
                      <option key={entry} value={entry}>
                        {PERMISSION_LABELS[entry]}
                      </option>
                    ))}
                  </select>
                ) : null}
                {(bootstrap?.development_modes?.length ?? 0) > 0 ? (
                  <select
                    className="composer-select"
                    aria-label="开发模式（仅下一回合）"
                    value={developmentMode}
                    onChange={(event) => setDevelopmentMode(event.target.value)}
                  >
                    <option value="">默认模式</option>
                    {bootstrap?.development_modes?.map((entry) => (
                      <option key={entry} value={entry}>
                        {MODE_LABELS[entry]}
                      </option>
                    ))}
                  </select>
                ) : null}
              </>
            ) : null}
          </div>
          <div className="composer-actions">
            {canSteer ? (
              <button
                type="button"
                className="icon-button steer-button"
                aria-label="立即引导当前回合"
                title="立即引导（steer）"
                disabled={draft.trim() === "" || sending}
                onClick={() => actions.steer(draft)}
              >
                <Zap size={15} aria-hidden />
              </button>
            ) : null}
            {running ? (
              <button
                type="button"
                className="send-button stop"
                aria-label="停止当前回合"
                title="停止当前回合"
                onClick={() => actions.stop()}
              >
                <Square size={15} aria-hidden />
              </button>
            ) : null}
            <button
              type="button"
              className="send-button"
              aria-label={running ? "发送后续消息" : "发送"}
              title={running ? "发送后续消息（排队）" : "发送"}
              disabled={draft.trim() === "" || sending}
              onClick={submit}
            >
              <ArrowUp size={16} aria-hidden />
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
