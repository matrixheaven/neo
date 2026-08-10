/**
 * Floating composer. Enter sends, Shift+Enter inserts a newline, IME
 * composition never sends. New sessions create; idle sessions start a turn;
 * running sessions send follow_up. Stop and steer are separate, distinct
 * actions — never borrow the normal send.
 *
 * The pill row below the textarea holds: attachment picker (+ drag & drop),
 * model pill with a searchable overlay, permission pill, development-mode
 * pill, reasoning pill (capable models only) — with the context ring and
 * send/stop/steer on the right. All pill selections are per-next-turn
 * overrides only; nothing is written back to global settings or persisted.
 */

import { ArrowUp, ChevronDown, Paperclip, Search, Square, X, Zap } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { uploadAttachment } from "../api";
import type {
  PermissionMode,
  WebUiComposer,
  WebUiContextWindow,
  WebUiDevelopmentMode,
  WebUiModelInfo,
} from "../protocol";
import { useAppActions, useAppState } from "../state/store";
import { NeoMark } from "./neoMark";
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

const REASONING_EFFORTS = ["low", "medium", "high"] as const;
const REASONING_LABELS: Record<string, string> = {
  low: "低推理",
  medium: "中推理",
  high: "高推理",
};

const MAX_ATTACHMENTS = 4;
const MAX_ATTACHMENT_BYTES = 8 * 1024 * 1024;

/** Compact token count: 83700 → "83.7k", 256000 → "256k". */
export function formatTokens(value: number): string {
  if (value >= 1000) {
    const k = value / 1000;
    return `${Number.isInteger(k) ? k : k.toFixed(1)}k`;
  }
  return String(value);
}

interface QueuedAttachment {
  key: number;
  name: string;
  mime: string;
  /** In-memory data URL preview for images; never persisted. */
  preview: string | null;
  status: "uploading" | "ready" | "error";
  id: string | null;
}

function ContextRing({ window: cw }: { window: WebUiContextWindow }) {
  const max = cw.max_tokens ?? null;
  if (max === null || max <= 0) return null;
  const used = cw.used_tokens;
  const fraction = Math.min(1, Math.max(0, used / max));
  const percent = Math.round(fraction * 100);
  const radius = 6;
  const circumference = 2 * Math.PI * radius;
  const tooltip = `${formatTokens(used)} / ${formatTokens(max)} tokens (${percent}%)`;
  return (
    <span
      className="context-ring"
      role="img"
      aria-label={`上下文占用 ${percent}%`}
      title={tooltip}
    >
      <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden>
        <circle className="context-ring-track" cx="8" cy="8" r={radius} />
        <circle
          className="context-ring-value"
          cx="8"
          cy="8"
          r={radius}
          strokeDasharray={circumference}
          strokeDashoffset={circumference * (1 - fraction)}
          transform="rotate(-90 8 8)"
        />
      </svg>
      <span className="context-ring-pct">{percent}%</span>
    </span>
  );
}

export function Composer({ centered }: { centered: boolean }) {
  const appState = useAppState();
  const actions = useAppActions();
  const sessionId = appState.selectedSessionId;
  const view = sessionId !== null ? appState.sessions[sessionId] : undefined;
  const [localDraft, setLocalDraft] = useState("");
  const draft = sessionId !== null ? (view?.draft ?? "") : localDraft;
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const composingRef = useRef(false);

  const bootstrap = appState.bootstrap;
  const running =
    view !== undefined &&
    (view.phase === "running" || view.phase === "starting" || view.phase === "finishing") &&
    view.currentTurnId !== null;
  const sending = sessionId === null ? appState.creatingSession : (view?.sending ?? false);

  // Per-next-turn overrides; never written back to global settings.
  const [model, setModel] = useState("");
  const [permissionMode, setPermissionMode] = useState("");
  const [developmentMode, setDevelopmentMode] = useState("");
  const [reasoningEffort, setReasoningEffort] = useState("");

  // -- Attachment queue ------------------------------------------------------
  const [attachments, setAttachments] = useState<QueuedAttachment[]>([]);
  const [attachmentError, setAttachmentError] = useState<string | null>(null);
  const [dragOver, setDragOver] = useState(false);
  const attachmentKeyRef = useRef(0);
  // Mirrors attachments.length for synchronous checks inside event handlers.
  const queueCountRef = useRef(0);
  const fileInputRef = useRef<HTMLInputElement | null>(null);

  // -- Model overlay ----------------------------------------------------------
  const [modelOpen, setModelOpen] = useState(false);
  const [modelQuery, setModelQuery] = useState("");
  const modelWrapRef = useRef<HTMLDivElement | null>(null);
  const modelButtonRef = useRef<HTMLButtonElement | null>(null);
  const modelWasOpenRef = useRef(false);
  // Esc closes return focus to the pill; outside clicks leave focus where
  // the user clicked.
  const closeViaEscRef = useRef(false);

  // -- Welcome banner ----------------------------------------------------------
  const hasUserMessage =
    view?.projection.items.some((item) => item.kind === "user_message") ?? false;
  const isFreshSession = sessionId === null || !hasUserMessage;
  const [bannerMounted, setBannerMounted] = useState(isFreshSession);
  const [bannerVisible, setBannerVisible] = useState(false);

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

  // Welcome banner fade: mount first, fade in on the next frame; on the first
  // canonical user message fade out, then unmount.
  useEffect(() => {
    if (isFreshSession) {
      setBannerMounted(true);
      const frame = requestAnimationFrame(() => setBannerVisible(true));
      return () => cancelAnimationFrame(frame);
    }
    setBannerVisible(false);
    const timer = window.setTimeout(() => setBannerMounted(false), 240);
    return () => window.clearTimeout(timer);
  }, [isFreshSession]);

  // Model overlay: Esc / outside click close, focus returns to the pill.
  useEffect(() => {
    if (!modelOpen) {
      if (modelWasOpenRef.current) {
        modelWasOpenRef.current = false;
        if (closeViaEscRef.current) {
          closeViaEscRef.current = false;
          modelButtonRef.current?.focus();
        }
      }
      return;
    }
    modelWasOpenRef.current = true;
    const onPointerDown = (event: MouseEvent) => {
      if (modelWrapRef.current && !modelWrapRef.current.contains(event.target as Node)) {
        setModelOpen(false);
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        closeViaEscRef.current = true;
        setModelOpen(false);
      }
    };
    document.addEventListener("mousedown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("mousedown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [modelOpen]);

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
    if (reasoningEffort !== "") composer.reasoning_effort = reasoningEffort;
    return Object.keys(composer).length > 0 ? composer : undefined;
  };

  const setDraft = (text: string) => {
    if (sessionId !== null) {
      actions.setDraft(sessionId, text);
    } else {
      setLocalDraft(text);
    }
  };

  // -- Attachments -------------------------------------------------------------

  const updateAttachment = (key: number, change: Partial<QueuedAttachment>) => {
    setAttachments((current) =>
      current.map((entry) => (entry.key === key ? { ...entry, ...change } : entry)),
    );
  };

  const addFiles = (files: Iterable<File>) => {
    setAttachmentError(null);
    for (const file of files) {
      // Queue length is read from a ref: the state closure is stale after
      // the first enqueue inside this loop.
      if (queueCountRef.current >= MAX_ATTACHMENTS) {
        setAttachmentError(`最多 ${MAX_ATTACHMENTS} 个附件。`);
        break;
      }
      if (file.size > MAX_ATTACHMENT_BYTES) {
        setAttachmentError(`「${file.name}」超过 8MiB 上限，未加入。`);
        continue;
      }
      queueCountRef.current += 1;
      attachmentKeyRef.current += 1;
      const key = attachmentKeyRef.current;
      const mime = file.type !== "" ? file.type : "application/octet-stream";
      const isImage = mime.startsWith("image/");
      setAttachments((current) => [
        ...current,
        { key, name: file.name, mime, preview: null, status: "uploading", id: null },
      ]);
      const reader = new FileReader();
      reader.onload = () => {
        const result = typeof reader.result === "string" ? reader.result : "";
        const base64 = result.includes(",") ? result.slice(result.indexOf(",") + 1) : result;
        if (isImage) updateAttachment(key, { preview: result });
        uploadAttachment(mime, base64)
          .then((ack) => updateAttachment(key, { status: "ready", id: ack.id }))
          .catch(() => {
            updateAttachment(key, { status: "error" });
            // Non-sensitive: no server payload, no file path.
            setAttachmentError("附件上传失败，请重试或移除。");
          });
      };
      reader.onerror = () => {
        updateAttachment(key, { status: "error" });
        setAttachmentError("附件读取失败，请重试或移除。");
      };
      reader.readAsDataURL(file);
    }
  };

  const removeAttachment = (key: number) => {
    queueCountRef.current = Math.max(0, queueCountRef.current - 1);
    setAttachments((current) => current.filter((entry) => entry.key !== key));
  };

  const uploading = attachments.some((entry) => entry.status === "uploading");

  const submit = () => {
    const text = draft.trim();
    if (text === "" || sending || uploading) return;
    const ids = attachments
      .filter((entry) => entry.status === "ready" && entry.id !== null)
      .map((entry) => entry.id as string);
    actions.sendMessage(text, composerOverrides(), ids.length > 0 ? ids : undefined, () => {
      queueCountRef.current = 0;
      setAttachments([]);
    });
  };

  const onKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Enter" && !event.shiftKey && !composingRef.current && !event.nativeEvent.isComposing) {
      event.preventDefault();
      submit();
    }
  };

  const todos = view?.projection.todos ?? [];
  const canSteer = running; // steer is part of the final input protocol

  const models = bootstrap?.models ?? [];
  const permissionModes = bootstrap?.permission_modes ?? [];
  const developmentModes = bootstrap?.development_modes ?? [];
  const selectedModel: WebUiModelInfo | undefined = models.find(
    (entry) => entry.alias === model,
  );
  const reasoningCapable = (selectedModel?.capabilities ?? []).includes("reasoning");

  const cycle = (current: string, options: readonly string[]): string => {
    const index = current === "" ? -1 : options.indexOf(current);
    const next = index + 1;
    return next >= options.length ? "" : options[next];
  };

  const filteredModels = models.filter((entry) => {
    const needle = modelQuery.trim().toLowerCase();
    if (needle === "") return true;
    return (
      entry.alias.toLowerCase().includes(needle) ||
      entry.provider.toLowerCase().includes(needle)
    );
  });

  const contextWindow = view?.projection.contextWindow ?? null;

  return (
    <div className={`composer-dock ${centered ? "centered" : ""}`}>
      {!centered && sessionId !== null ? <TaskList todos={todos} /> : null}
      <div
        className={`composer ${dragOver ? "drag-over" : ""}`}
        data-centered={centered}
        onDragEnter={(event) => {
          event.preventDefault();
          setDragOver(true);
        }}
        onDragOver={(event) => {
          event.preventDefault();
          setDragOver(true);
        }}
        onDragLeave={(event) => {
          if (!event.currentTarget.contains(event.relatedTarget as Node | null)) {
            setDragOver(false);
          }
        }}
        onDrop={(event) => {
          event.preventDefault();
          setDragOver(false);
          addFiles(event.dataTransfer.files);
        }}
      >
        {bannerMounted ? (
          <div className={`welcome-banner ${bannerVisible ? "visible" : ""}`} role="note">
            <NeoMark size={16} />
            <span>描述你的任务，回车发送 — Neo 在本地完成其余工作。</span>
          </div>
        ) : null}
        {attachments.length > 0 ? (
          <div className="attachment-queue">
            {attachments.map((entry) => (
              <span key={entry.key} className="attachment-chip" data-status={entry.status}>
                {entry.preview !== null ? (
                  <img className="attachment-thumb" src={entry.preview} alt="" />
                ) : (
                  <Paperclip size={12} aria-hidden />
                )}
                <span className="attachment-name">{entry.name}</span>
                {entry.status === "uploading" ? (
                  <span className="attachment-state">上传中</span>
                ) : null}
                {entry.status === "error" ? (
                  <span className="attachment-state error">失败</span>
                ) : null}
                <button
                  type="button"
                  className="attachment-remove"
                  aria-label={`移除附件 ${entry.name}`}
                  onClick={() => removeAttachment(entry.key)}
                >
                  <X size={12} aria-hidden />
                </button>
              </span>
            ))}
          </div>
        ) : null}
        {attachmentError !== null ? (
          <div className="attachment-error" role="alert">
            {attachmentError}
          </div>
        ) : null}
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
            <button
              type="button"
              className="attach-button"
              aria-label="添加附件"
              title="添加附件（最多 4 个，每个不超过 8MiB）"
              onClick={() => fileInputRef.current?.click()}
            >
              <Paperclip size={15} aria-hidden />
            </button>
            <input
              ref={fileInputRef}
              type="file"
              multiple
              hidden
              aria-label="选择附件文件"
              onChange={(event) => {
                if (event.target.files) addFiles(event.target.files);
                event.target.value = "";
              }}
            />
            {models.length > 0 ? (
              <div className="pill-wrap" ref={modelWrapRef}>
                <button
                  type="button"
                  ref={modelButtonRef}
                  className="composer-pill model-pill"
                  aria-label="模型（仅下一回合）"
                  aria-expanded={modelOpen}
                  aria-haspopup="listbox"
                  title={model === "" ? "默认模型" : model}
                  onClick={() => {
                    setModelQuery("");
                    setModelOpen((open) => !open);
                  }}
                >
                  <span className="model-pill-name">
                    {model === "" ? "默认模型" : model}
                  </span>
                  <ChevronDown size={12} aria-hidden />
                </button>
                {modelOpen ? (
                  <div className="pill-popover" role="dialog" aria-label="选择模型">
                    <div className="pill-popover-search">
                      <Search size={13} aria-hidden />
                      <input
                        autoFocus
                        aria-label="搜索模型"
                        placeholder="搜索模型…"
                        value={modelQuery}
                        onChange={(event) => setModelQuery(event.target.value)}
                      />
                    </div>
                    <div className="pill-popover-list" role="listbox" aria-label="模型列表">
                      <button
                        type="button"
                        role="option"
                        aria-selected={model === ""}
                        className={`model-row ${model === "" ? "selected" : ""}`}
                        onClick={() => {
                          setModel("");
                          setModelOpen(false);
                        }}
                      >
                        <span className="model-row-name">默认模型</span>
                        <span className="model-row-meta">跟随会话配置</span>
                      </button>
                      {filteredModels.map((entry) => (
                        <button
                          type="button"
                          key={entry.alias}
                          role="option"
                          aria-selected={model === entry.alias}
                          className={`model-row ${model === entry.alias ? "selected" : ""}`}
                          onClick={() => {
                            setModel(entry.alias);
                            setModelOpen(false);
                          }}
                        >
                          <span className="model-row-name">{entry.alias}</span>
                          <span className="model-row-meta">
                            {entry.provider}
                            {entry.context_window
                              ? ` · ${formatTokens(entry.context_window)}`
                              : ""}
                          </span>
                          {(entry.capabilities ?? []).length > 0 ? (
                            <span className="model-row-caps">
                              {(entry.capabilities ?? []).map((capability) => (
                                <span key={capability} className="cap-chip">
                                  {capability}
                                </span>
                              ))}
                            </span>
                          ) : null}
                        </button>
                      ))}
                      {filteredModels.length === 0 ? (
                        <div className="pill-popover-empty">没有匹配的模型</div>
                      ) : null}
                    </div>
                  </div>
                ) : null}
              </div>
            ) : null}
            {permissionModes.length > 0 ? (
              <button
                type="button"
                className="composer-pill perm-pill"
                data-mode={permissionMode === "" ? "default" : permissionMode}
                aria-label="权限模式（仅下一回合）"
                title="点击切换权限模式（仅下一回合）"
                onClick={() => setPermissionMode(cycle(permissionMode, permissionModes))}
              >
                {permissionMode === ""
                  ? "权限"
                  : PERMISSION_LABELS[permissionMode as PermissionMode]}
              </button>
            ) : null}
            {developmentModes.length > 0 ? (
              <button
                type="button"
                className="composer-pill mode-pill"
                data-active={developmentMode !== ""}
                aria-label="开发模式（仅下一回合）"
                title="点击切换开发模式（仅下一回合）"
                onClick={() => setDevelopmentMode(cycle(developmentMode, developmentModes))}
              >
                {developmentMode === ""
                  ? "模式"
                  : MODE_LABELS[developmentMode as WebUiDevelopmentMode]}
              </button>
            ) : null}
            {reasoningCapable ? (
              <button
                type="button"
                className="composer-pill reasoning-pill"
                data-active={reasoningEffort !== ""}
                aria-label="推理强度（仅下一回合）"
                title="点击切换推理强度（仅下一回合）"
                onClick={() => setReasoningEffort(cycle(reasoningEffort, REASONING_EFFORTS))}
              >
                {reasoningEffort === "" ? "推理" : REASONING_LABELS[reasoningEffort]}
              </button>
            ) : null}
          </div>
          <div className="composer-actions">
            {contextWindow !== null ? <ContextRing window={contextWindow} /> : null}
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
              title={
                uploading
                  ? "等待附件上传完成"
                  : running
                    ? "发送后续消息（排队）"
                    : "发送"
              }
              disabled={draft.trim() === "" || sending || uploading}
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
