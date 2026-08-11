/**
 * Floating composer. Enter sends, Shift+Enter inserts a newline, IME
 * composition never sends. New sessions create; idle sessions start a turn;
 * running sessions send follow_up. Stop and steer are separate, distinct
 * actions — never borrow the normal send.
 *
 * The pill row below the textarea holds: attachment picker (+ drag & drop),
 * model pill with a two-level menu, permission and development-mode menus,
 * reasoning pill (capable models only) — with the context ring and
 * send/stop/steer on the right. All selections are per-next-turn
 * overrides only; nothing is written back to global settings or persisted.
 */

import {
  ArrowUp,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  Folder,
  GitBranch,
  Paperclip,
  Search,
  Square,
  X,
  Zap,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { fetchCompletions, uploadAttachment } from "../api";
import type {
  PermissionMode,
  ReasoningCapability,
  ReasoningSelection,
  WebUiComposer,
  WebUiCompletionItem,
  WebUiContextWindow,
  WebUiDevelopmentMode,
  WebUiModelInfo,
} from "../protocol";
import { useAppActions, useAppState } from "../state/store";
import { NeoMark } from "./neoMark";
import { TaskList } from "./taskList";
import { activeCompletionRange, replaceCompletion } from "./composerCompletion";

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

const REASONING_LABELS: Record<string, string> = {
  minimal: "极简",
  low: "低",
  medium: "中",
  high: "高",
  xhigh: "极高",
  max: "最大",
};

const MAX_ATTACHMENTS = 4;
const MAX_ATTACHMENT_BYTES = 8 * 1024 * 1024;

type ComposerMenu = "model" | "permission" | "development";
type ModelMenuPane = "root" | "models" | "reasoning";

const NO_REASONING: ReasoningCapability = { type: "none" };

function reasoningLabel(selection: ReasoningSelection): string {
  switch (selection.mode) {
    case "off":
      return "关闭";
    case "on":
      return "开启";
    case "effort":
      return REASONING_LABELS[selection.effort] ?? selection.effort;
    case "budget_tokens":
      return `${selection.budget_tokens.toLocaleString()} 个令牌`;
  }
}

function reasoningKey(selection: ReasoningSelection): string {
  switch (selection.mode) {
    case "effort":
      return `effort:${selection.effort}`;
    case "budget_tokens":
      return `budget:${selection.budget_tokens}`;
    default:
      return selection.mode;
  }
}

function budgetBounds(capability: ReasoningCapability) {
  if (capability.type === "budget_tokens") {
    return { min: capability.min ?? null, max: capability.max ?? null };
  }
  if (capability.type === "combined") return capability.budget ?? null;
  return null;
}

function supportsReasoning(
  capability: ReasoningCapability,
  selection: ReasoningSelection,
): boolean {
  if (selection.mode === "off") {
    return capability.type === "none" || capability.disable_supported;
  }
  if (selection.mode === "on") {
    return (
      capability.type === "toggle" ||
      (capability.type === "combined" && capability.toggle)
    );
  }
  if (selection.mode === "effort") {
    const values =
      capability.type === "effort"
        ? capability.values
        : capability.type === "combined"
          ? capability.effort
          : [];
    return values.includes(selection.effort);
  }
  const bounds = budgetBounds(capability);
  return (
    bounds !== null &&
    (bounds.min == null || selection.budget_tokens >= bounds.min) &&
    (bounds.max == null || selection.budget_tokens <= bounds.max)
  );
}

function reasoningChoices(capability: ReasoningCapability): ReasoningSelection[] {
  if (capability.type === "none") return [];
  const choices: ReasoningSelection[] = [];
  if (capability.disable_supported) choices.push({ mode: "off" });
  const efforts =
    capability.type === "effort"
      ? capability.values
      : capability.type === "combined"
        ? capability.effort
        : [];
  if (efforts.length > 0) {
    choices.push(...efforts.map((effort) => ({ mode: "effort" as const, effort })));
    return choices;
  }
  const bounds = budgetBounds(capability);
  if (bounds !== null) {
    const values = [1024, 8192, bounds.max ?? 24576].filter(
      (value, index, all) =>
        all.indexOf(value) === index &&
        (bounds.min == null || value >= bounds.min) &&
        (bounds.max == null || value <= bounds.max),
    );
    choices.push(...values.map((budget_tokens) => ({ mode: "budget_tokens" as const, budget_tokens })));
    return choices;
  }
  if (
    capability.type === "toggle" ||
    (capability.type === "combined" && capability.toggle)
  ) {
    choices.push({ mode: "on" });
  }
  return choices;
}

function defaultReasoning(capability: ReasoningCapability): ReasoningSelection {
  return reasoningChoices(capability)[0] ?? { mode: "off" };
}

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
  const [caret, setCaret] = useState(0);
  const [completionItems, setCompletionItems] = useState<WebUiCompletionItem[]>([]);
  const [completionIndex, setCompletionIndex] = useState(0);
  const [dismissedCompletion, setDismissedCompletion] = useState<string | null>(null);
  const completionRangeRef = useRef<ReturnType<typeof activeCompletionRange>>(null);

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
  const [reasoning, setReasoning] = useState<ReasoningSelection | null>(null);

  // -- Attachment queue ------------------------------------------------------
  const [attachments, setAttachments] = useState<QueuedAttachment[]>([]);
  const [attachmentError, setAttachmentError] = useState<string | null>(null);
  const [dragOver, setDragOver] = useState(false);
  const attachmentKeyRef = useRef(0);
  // Mirrors attachments.length for synchronous checks inside event handlers.
  const queueCountRef = useRef(0);
  const fileInputRef = useRef<HTMLInputElement | null>(null);

  // -- Per-turn menus ---------------------------------------------------------
  const [openMenu, setOpenMenu] = useState<ComposerMenu | null>(null);
  const [modelMenuPane, setModelMenuPane] = useState<ModelMenuPane>("root");
  const [modelQuery, setModelQuery] = useState("");
  const [budgetInput, setBudgetInput] = useState("");
  const modelWrapRef = useRef<HTMLDivElement | null>(null);
  const permissionWrapRef = useRef<HTMLDivElement | null>(null);
  const developmentWrapRef = useRef<HTMLDivElement | null>(null);
  const menuButtonRef = useRef<HTMLButtonElement | null>(null);
  const menuWasOpenRef = useRef(false);
  // Esc closes return focus to the pill; outside clicks leave focus where
  // the user clicked.
  const closeViaEscRef = useRef(false);

  // -- Welcome banner ----------------------------------------------------------
  const hasUserMessage =
    view?.projection.items.some((item) => item.kind === "user_message") ?? false;
  const hasTranscript = (view?.projection.items.length ?? 0) > 0;
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

  // Per-turn menus: Esc / outside click close, focus returns to the pill.
  useEffect(() => {
    if (openMenu === null) {
      if (menuWasOpenRef.current) {
        menuWasOpenRef.current = false;
        if (closeViaEscRef.current) {
          closeViaEscRef.current = false;
          menuButtonRef.current?.focus();
        }
      }
      return;
    }
    menuWasOpenRef.current = true;
    const activeMenuWrap =
      openMenu === "model"
        ? modelWrapRef.current
        : openMenu === "permission"
          ? permissionWrapRef.current
          : developmentWrapRef.current;
    const onPointerDown = (event: MouseEvent) => {
      if (activeMenuWrap && !activeMenuWrap.contains(event.target as Node)) {
        setOpenMenu(null);
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        closeViaEscRef.current = true;
        setOpenMenu(null);
      }
    };
    document.addEventListener("mousedown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("mousedown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [openMenu]);

  const autosize = () => {
    const element = textareaRef.current;
    if (!element) return;
    element.style.height = "auto";
    element.style.height = `${Math.min(180, Math.max(52, element.scrollHeight))}px`;
  };

  useEffect(autosize, [draft]);

  useEffect(() => {
    const range = activeCompletionRange(draft, caret);
    completionRangeRef.current = range;
    if (range === null || dismissedCompletion === range.query) {
      setCompletionItems([]);
      setCompletionIndex(0);
      return;
    }
    const controller = new AbortController();
    setCompletionItems([]);
    setCompletionIndex(0);
    fetchCompletions(range.query, controller.signal)
      .then((response) => {
        if (!controller.signal.aborted) setCompletionItems(response.items);
      })
      .catch(() => {
        if (!controller.signal.aborted) setCompletionItems([]);
      });
    return () => controller.abort();
  }, [caret, draft, dismissedCompletion]);

  const composerOverrides = (): WebUiComposer | undefined => {
    const composer: WebUiComposer = {};
    if (model !== "") composer.model = model;
    if (permissionMode !== "") composer.permission_mode = permissionMode as PermissionMode;
    if (developmentMode !== "") {
      composer.development_mode = developmentMode as WebUiDevelopmentMode;
    }
    if (reasoning !== null) composer.reasoning = reasoning;
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
    if (completionItems.length > 0 && completionRangeRef.current !== null) {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        setCompletionIndex((current) =>
          event.key === "ArrowDown"
            ? (current + 1) % completionItems.length
            : (current - 1 + completionItems.length) % completionItems.length,
        );
        return;
      }
      if (event.key === "Enter" || event.key === "Tab") {
        event.preventDefault();
        const item = completionItems[completionIndex];
        if (item) selectCompletion(item);
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        setDismissedCompletion(completionRangeRef.current.query);
        return;
      }
    }
    if (event.key === "Enter" && !event.shiftKey && !composingRef.current && !event.nativeEvent.isComposing) {
      event.preventDefault();
      submit();
    }
  };

  const selectCompletion = (item: WebUiCompletionItem) => {
    const range = completionRangeRef.current;
    if (range === null) return;
    const next = replaceCompletion(draft, range, item.value);
    setDraft(next.text);
    setCaret(next.caret);
    setCompletionItems([]);
    setDismissedCompletion(null);
    requestAnimationFrame(() => {
      textareaRef.current?.focus();
      textareaRef.current?.setSelectionRange(next.caret, next.caret);
    });
  };

  const todos = view?.projection.todos ?? [];
  const canSteer = running; // steer is part of the final input protocol

  const models = bootstrap?.models ?? [];
  const permissionModes = bootstrap?.permission_modes ?? [];
  const developmentModes = bootstrap?.development_modes ?? [];
  const defaultModel = bootstrap?.default_model ?? "";
  const activeModelAlias = model === "" ? defaultModel : model;
  const selectedModel: WebUiModelInfo | undefined = models.find(
    (entry) => entry.alias === activeModelAlias,
  );
  const reasoningCapability = selectedModel?.reasoning ?? NO_REASONING;
  const configuredReasoning = bootstrap?.default_reasoning ?? { mode: "off" };
  const effectiveReasoning =
    reasoning !== null && supportsReasoning(reasoningCapability, reasoning)
      ? reasoning
      : supportsReasoning(reasoningCapability, configuredReasoning)
        ? configuredReasoning
        : defaultReasoning(reasoningCapability);
  const reasoningCapable = reasoningCapability.type !== "none";
  const availableReasoning = reasoningChoices(reasoningCapability);
  const bounds = budgetBounds(reasoningCapability);
  const customBudget = Number(budgetInput);
  const customBudgetValid =
    budgetInput !== "" &&
    Number.isSafeInteger(customBudget) &&
    customBudget >= 0 &&
    customBudget <= 4_294_967_295 &&
    supportsReasoning(reasoningCapability, {
      mode: "budget_tokens",
      budget_tokens: customBudget,
    });

  const filteredModels = models.filter((entry) => {
    const needle = modelQuery.trim().toLowerCase();
    if (needle === "") return true;
    return (
      entry.alias.toLowerCase().includes(needle) ||
      (entry.display_name ?? "").toLowerCase().includes(needle) ||
      entry.provider.toLowerCase().includes(needle)
    );
  });
  const groupedModels = filteredModels.reduce<Map<string, WebUiModelInfo[]>>(
    (groups, entry) => {
      const group = groups.get(entry.provider) ?? [];
      group.push(entry);
      groups.set(entry.provider, group);
      return groups;
    },
    new Map(),
  );

  const toggleMenu = (menu: ComposerMenu, button: HTMLButtonElement) => {
    menuButtonRef.current = button;
    if (openMenu === menu) {
      setOpenMenu(null);
      return;
    }
    if (menu === "model") {
      setModelMenuPane("root");
      setModelQuery("");
      setBudgetInput("");
    }
    setOpenMenu(menu);
  };

  const selectModel = (entry: WebUiModelInfo | null) => {
    if (entry === null) {
      setModel("");
      setReasoning(null);
      setModelMenuPane("root");
      return;
    }
    setModel(entry.alias);
    setReasoning(
      supportsReasoning(entry.reasoning, effectiveReasoning)
        ? effectiveReasoning
        : defaultReasoning(entry.reasoning),
    );
    setModelMenuPane("root");
  };

  const modelOption = (entry: WebUiModelInfo) => (
    <button
      type="button"
      key={entry.alias}
      role="option"
      aria-selected={model !== "" && activeModelAlias === entry.alias}
      className={`model-row ${model !== "" && activeModelAlias === entry.alias ? "selected" : ""}`}
      onClick={() => selectModel(entry)}
    >
      <span className="model-row-name">{entry.display_name ?? entry.alias}</span>
      <span className="model-row-meta">
        {entry.display_name ? entry.alias : entry.provider}
        {entry.context_window ? ` · ${formatTokens(entry.context_window)}` : ""}
      </span>
      {model !== "" && activeModelAlias === entry.alias ? (
        <Check className="model-row-check" size={14} />
      ) : null}
    </button>
  );

  const onModelMenuKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    if (event.target instanceof HTMLInputElement && event.target.type === "number") return;
    const panel = (event.target as HTMLElement).closest(".pill-popover");
    if (!(panel instanceof HTMLElement)) return;
    const controls = [...panel.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled)")];
    if (controls.length === 0) return;
    event.preventDefault();
    const current = controls.indexOf(document.activeElement as HTMLElement);
    const next =
      event.key === "ArrowDown"
        ? (current + 1 + controls.length) % controls.length
        : (current - 1 + controls.length) % controls.length;
    controls[next]?.focus();
  };

  const contextWindow = view?.projection.contextWindow ?? null;
  const selectedWorkspace =
    appState.workspaces.find((group) => group.id === appState.selectedWorkspaceId) ??
    appState.workspaces.find((group) => group.current);

  return (
    <div className={`composer-dock ${centered ? "centered" : ""}`}>
      {!centered && sessionId !== null ? <TaskList todos={todos} /> : null}
      {centered && selectedWorkspace ? (
        <div className="workspace-bar" aria-label="新会话项目">
          <Folder size={14} aria-hidden />
          <select
            aria-label="选择项目"
            value={selectedWorkspace.id}
            onChange={(event) => actions.selectWorkspace(event.target.value)}
          >
            {appState.workspaces.map((workspace) => (
              <option key={workspace.id} value={workspace.id}>
                {workspace.label}
              </option>
            ))}
          </select>
          {selectedWorkspace.branch ? (
            <span className="workspace-branch">
              <GitBranch size={13} aria-hidden />
              {selectedWorkspace.branch}
            </span>
          ) : null}
        </div>
      ) : null}
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
            setCaret(event.target.selectionStart ?? event.target.value.length);
            setDismissedCompletion(null);
          }}
          onSelect={(event) => setCaret(event.currentTarget.selectionStart ?? 0)}
          onKeyDown={onKeyDown}
          onCompositionStart={() => {
            composingRef.current = true;
          }}
          onCompositionEnd={() => {
            composingRef.current = false;
          }}
        />
        {completionItems.length > 0 ? (
          <div
            className={`composer-completions ${hasTranscript ? "above" : "below"}`}
            role="listbox"
            aria-label="输入候选"
          >
            {completionItems.map((item, index) => (
              <button
                type="button"
                role="option"
                aria-selected={index === completionIndex}
                className={`composer-completion-row ${index === completionIndex ? "selected" : ""}`}
                key={`${item.value}:${index}`}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => selectCompletion(item)}
              >
                <span className="composer-completion-value">{item.label}</span>
                {item.description ? (
                  <span className="composer-completion-description">{item.description}</span>
                ) : null}
              </button>
            ))}
          </div>
        ) : null}
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
                  className="composer-pill model-pill"
                  aria-label="模型与推理（仅下一回合）"
                  aria-expanded={openMenu === "model"}
                  aria-haspopup="dialog"
                  title="选择模型与推理强度（仅下一回合）"
                  onClick={(event) => {
                    toggleMenu("model", event.currentTarget);
                  }}
                >
                  <span className="model-pill-name">
                    {selectedModel?.display_name ?? (activeModelAlias || "默认模型")}
                  </span>
                  {reasoningCapable ? (
                    <span className="model-pill-reasoning">
                      {reasoningLabel(effectiveReasoning)}
                    </span>
                  ) : null}
                  <ChevronDown size={12} aria-hidden />
                </button>
                {openMenu === "model" ? (
                  <div
                    className="model-menu-shell"
                    data-pane={modelMenuPane}
                    role="dialog"
                    aria-label="选择模型与推理"
                    onKeyDown={onModelMenuKeyDown}
                  >
                    <div className="pill-popover model-settings-popover">
                      <div className="pill-popover-list model-settings-list">
                        <button
                          type="button"
                          autoFocus
                          className={`model-settings-row ${modelMenuPane === "models" ? "selected" : ""}`}
                          onClick={() => setModelMenuPane("models")}
                        >
                          <span>模型</span>
                          <span className="model-settings-value">
                            {selectedModel?.display_name ?? (activeModelAlias || "默认模型")}
                            <ChevronRight size={14} aria-hidden />
                          </span>
                        </button>
                        {reasoningCapable ? (
                          <button
                            type="button"
                            className={`model-settings-row ${modelMenuPane === "reasoning" ? "selected" : ""}`}
                            onClick={() => setModelMenuPane("reasoning")}
                          >
                            <span>推理强度</span>
                            <span className="model-settings-value">
                              {reasoningLabel(effectiveReasoning)}
                              <ChevronRight size={14} aria-hidden />
                            </span>
                          </button>
                        ) : (
                          <div className="model-settings-row disabled">
                            <span>推理强度</span>
                            <span className="model-settings-value">不支持</span>
                          </div>
                        )}
                      </div>
                    </div>
                    {modelMenuPane === "models" ? (
                      <div className="pill-popover model-submenu" aria-label="选择模型">
                        <div className="model-submenu-title">
                          <button
                            type="button"
                            className="model-submenu-back"
                            aria-label="返回"
                            onClick={() => setModelMenuPane("root")}
                          >
                            <ChevronLeft size={14} aria-hidden />
                          </button>
                          <span>模型</span>
                        </div>
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
                        <div className="pill-popover-list model-catalog" role="listbox" aria-label="模型列表">
                          <button
                            type="button"
                            role="option"
                            aria-selected={model === ""}
                            className={`model-row ${model === "" ? "selected" : ""}`}
                            onClick={() => selectModel(null)}
                          >
                            <span className="model-row-name">跟随会话配置</span>
                            <span className="model-row-meta">{defaultModel || "默认模型"}</span>
                            {model === "" ? <Check className="model-row-check" size={14} /> : null}
                          </button>
                          {[...groupedModels].map(([provider, entries]) => (
                            <div className="model-provider-group" key={provider}>
                              <div className="model-provider-heading">{provider}</div>
                              {entries.map(modelOption)}
                            </div>
                          ))}
                          {filteredModels.length === 0 ? (
                            <div className="pill-popover-empty">没有匹配的模型</div>
                          ) : null}
                        </div>
                      </div>
                    ) : null}
                    {modelMenuPane === "reasoning" ? (
                      <div className="pill-popover model-submenu reasoning-submenu" aria-label="选择推理强度">
                        <div className="model-submenu-title">
                          <button
                            type="button"
                            className="model-submenu-back"
                            aria-label="返回"
                            onClick={() => setModelMenuPane("root")}
                          >
                            <ChevronLeft size={14} aria-hidden />
                          </button>
                          <span>推理强度</span>
                        </div>
                        <div className="pill-popover-list" role="listbox" aria-label="推理强度列表">
                          {availableReasoning.map((choice) => {
                            const selected = reasoningKey(choice) === reasoningKey(effectiveReasoning);
                            return (
                              <button
                                type="button"
                                role="option"
                                aria-selected={selected}
                                className={`model-row ${selected ? "selected" : ""}`}
                                key={reasoningKey(choice)}
                                onClick={() => {
                                  setReasoning(choice);
                                  setModelMenuPane("root");
                                }}
                              >
                                <span className="model-row-name">{reasoningLabel(choice)}</span>
                                {selected ? <Check className="model-row-check" size={14} /> : null}
                              </button>
                            );
                          })}
                          {bounds !== null ? (
                            <div className="reasoning-budget-row">
                              <input
                                type="number"
                                aria-label="自定义推理预算"
                                placeholder="自定义令牌数"
                                min={bounds.min ?? undefined}
                                max={bounds.max ?? undefined}
                                value={budgetInput}
                                onChange={(event) => setBudgetInput(event.target.value)}
                              />
                              <button
                                type="button"
                                disabled={!customBudgetValid}
                                onClick={() => {
                                  setReasoning({
                                    mode: "budget_tokens",
                                    budget_tokens: customBudget,
                                  });
                                  setModelMenuPane("root");
                                }}
                              >
                                应用
                              </button>
                            </div>
                          ) : null}
                        </div>
                      </div>
                    ) : null}
                  </div>
                ) : null}
              </div>
            ) : null}
            {permissionModes.length > 0 ? (
              <div className="pill-wrap" ref={permissionWrapRef}>
                <button
                  type="button"
                  className="composer-pill perm-pill"
                  data-mode={permissionMode === "" ? "default" : permissionMode}
                  aria-label="权限模式（仅下一回合）"
                  aria-expanded={openMenu === "permission"}
                  aria-haspopup="listbox"
                  title="选择权限模式（仅下一回合）"
                  onClick={(event) => {
                    toggleMenu("permission", event.currentTarget);
                  }}
                >
                  {permissionMode === ""
                    ? "权限"
                    : PERMISSION_LABELS[permissionMode as PermissionMode]}
                </button>
                {openMenu === "permission" ? (
                  <div className="pill-popover" role="dialog" aria-label="选择权限模式">
                    <div className="pill-popover-list" role="listbox" aria-label="权限模式列表">
                      <button
                        type="button"
                        role="option"
                        aria-selected={permissionMode === ""}
                        className={`model-row ${permissionMode === "" ? "selected" : ""}`}
                        onClick={() => {
                          setPermissionMode("");
                          setOpenMenu(null);
                        }}
                      >
                        <span className="model-row-name">默认</span>
                        <span className="model-row-meta">跟随会话配置</span>
                      </button>
                      {permissionModes.map((entry) => (
                        <button
                          type="button"
                          key={entry}
                          role="option"
                          aria-selected={permissionMode === entry}
                          className={`model-row ${permissionMode === entry ? "selected" : ""}`}
                          onClick={() => {
                            setPermissionMode(entry);
                            setOpenMenu(null);
                          }}
                        >
                          <span className="model-row-name">{PERMISSION_LABELS[entry]}</span>
                        </button>
                      ))}
                    </div>
                  </div>
                ) : null}
              </div>
            ) : null}
            {developmentModes.length > 0 ? (
              <div className="pill-wrap" ref={developmentWrapRef}>
                <button
                  type="button"
                  className="composer-pill mode-pill"
                  data-active={developmentMode !== ""}
                  aria-label="开发模式（仅下一回合）"
                  aria-expanded={openMenu === "development"}
                  aria-haspopup="listbox"
                  title="选择开发模式（仅下一回合）"
                  onClick={(event) => {
                    toggleMenu("development", event.currentTarget);
                  }}
                >
                  {developmentMode === ""
                    ? "模式"
                    : MODE_LABELS[developmentMode as WebUiDevelopmentMode]}
                </button>
                {openMenu === "development" ? (
                  <div className="pill-popover" role="dialog" aria-label="选择开发模式">
                    <div className="pill-popover-list" role="listbox" aria-label="开发模式列表">
                      <button
                        type="button"
                        role="option"
                        aria-selected={developmentMode === ""}
                        className={`model-row ${developmentMode === "" ? "selected" : ""}`}
                        onClick={() => {
                          setDevelopmentMode("");
                          setOpenMenu(null);
                        }}
                      >
                        <span className="model-row-name">默认</span>
                        <span className="model-row-meta">跟随会话配置</span>
                      </button>
                      {developmentModes.map((entry) => (
                        <button
                          type="button"
                          key={entry}
                          role="option"
                          aria-selected={developmentMode === entry}
                          className={`model-row ${developmentMode === entry ? "selected" : ""}`}
                          onClick={() => {
                            setDevelopmentMode(entry);
                            setOpenMenu(null);
                          }}
                        >
                          <span className="model-row-name">{MODE_LABELS[entry]}</span>
                        </button>
                      ))}
                    </div>
                  </div>
                ) : null}
              </div>
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
