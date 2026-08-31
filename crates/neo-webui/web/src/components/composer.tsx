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
  ChevronDown,
  Folder,
  GitBranch,
  Paperclip,
  Square,
  X,
  Zap,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { fetchCompletions, uploadAttachment } from "../api";
import { formatTokens, ModelPillMenu } from "./modelPill";
import type {
  PermissionMode,
  ReasoningSelection,
  WebUiComposer,
  WebUiCompletionItem,
  WebUiContextWindow,
  WebUiBranchList,
  WebUiDevelopmentMode,
} from "../protocol";
import { useAppActions, useAppState } from "../state/store";
import { AddWorkspaceDialog } from "./addWorkspaceDialog";
import { PickerPopover } from "./pickerPopover";
import { NeoMark } from "./neoMark";
import { QueuePanel } from "./queuePanel";
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

const MAX_ATTACHMENTS = 4;
const MAX_ATTACHMENT_BYTES = 8 * 1024 * 1024;

type ComposerMenu = "model" | "permission" | "development" | "workspace" | "branch";

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
  const modelWrapRef = useRef<HTMLDivElement | null>(null);
  const permissionWrapRef = useRef<HTMLDivElement | null>(null);
  const developmentWrapRef = useRef<HTMLDivElement | null>(null);
  const workspaceWrapRef = useRef<HTMLDivElement | null>(null);
  const branchWrapRef = useRef<HTMLDivElement | null>(null);
  const menuButtonRef = useRef<HTMLButtonElement | null>(null);
  const menuWasOpenRef = useRef(false);
  // Esc closes return focus to the pill; outside clicks leave focus where
  // the user clicked.
  const closeViaEscRef = useRef(false);

  // -- Workspace / branch pickers ----------------------------------------------
  const [workspaceSearch, setWorkspaceSearch] = useState("");
  const [branchSearch, setBranchSearch] = useState("");
  const [branchList, setBranchList] = useState<WebUiBranchList | null>(null);
  const [branchLoading, setBranchLoading] = useState(false);
  const [branchCreating, setBranchCreating] = useState(false);
  const [addingWorkspace, setAddingWorkspace] = useState(false);

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
          : openMenu === "workspace"
            ? workspaceWrapRef.current
            : openMenu === "branch"
              ? branchWrapRef.current
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
  const configuredReasoning = bootstrap?.default_reasoning ?? { mode: "off" };

  const toggleMenu = (menu: ComposerMenu, button: HTMLButtonElement) => {
    menuButtonRef.current = button;
    if (openMenu === menu) {
      setOpenMenu(null);
      return;
    }
    if (menu === "workspace") {
      setWorkspaceSearch("");
    }
    if (menu === "branch") {
      setBranchSearch("");
      setBranchCreating(false);
      setBranchList(null);
      setBranchLoading(true);
    }
    setOpenMenu(menu);
  };

  // Branch list loads when the branch picker opens (once per open).
  useEffect(() => {
    if (openMenu !== "branch") return;
    const workspaceId = selectedWorkspace?.id;
    if (workspaceId === undefined) {
      setBranchLoading(false);
      return;
    }
    let cancelled = false;
    actions
      .listBranches(workspaceId)
      .then((list) => {
        if (!cancelled) setBranchList(list);
      })
      .catch(() => {
        if (!cancelled) setBranchList({ branches: [] });
      })
      .finally(() => {
        if (!cancelled) setBranchLoading(false);
      });
    return () => {
      cancelled = true;
    };
    // selectedWorkspace is derived from appState.workspaces + selection.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [openMenu]);

  const contextWindow = view?.projection.contextWindow ?? null;
  const selectedWorkspace =
    appState.workspaces.find((group) => group.id === appState.selectedWorkspaceId) ??
    appState.workspaces.find((group) => group.current);

  return (
    <div className={`composer-dock ${centered ? "centered" : ""}`}>
      {!centered && sessionId !== null ? (
        <>
          <TaskList todos={todos} />
          <QueuePanel sessionId={sessionId} />
        </>
      ) : null}
      {centered && selectedWorkspace ? (
        <div className="workspace-bar" aria-label="新会话项目">
          <div className="pill-wrap" ref={workspaceWrapRef}>
            <button
              type="button"
              className="workspace-trigger"
              aria-label="选择项目"
              aria-expanded={openMenu === "workspace"}
              aria-haspopup="dialog"
              title="选择项目"
              onClick={(event) => {
                toggleMenu("workspace", event.currentTarget);
              }}
            >
              <Folder size={14} aria-hidden />
              {selectedWorkspace.label}
              <ChevronDown size={12} aria-hidden />
            </button>
            {openMenu === "workspace" ? (
              <PickerPopover
                searchPlaceholder="搜索工作区"
                searchLabel="搜索工作区"
                search={workspaceSearch}
                onSearch={setWorkspaceSearch}
                items={appState.workspaces
                  .filter((workspace) =>
                    workspace.label.toLowerCase().includes(workspaceSearch.trim().toLowerCase()),
                  )
                  .map((workspace) => ({
                    id: workspace.id,
                    label: workspace.label,
                    current: workspace.id === selectedWorkspace.id,
                  }))}
                onSelect={(id) => {
                  actions.selectWorkspace(id);
                  setOpenMenu(null);
                }}
                footerLabel="打开新文件夹作为工作区"
                onFooter={() => {
                  setOpenMenu(null);
                  setAddingWorkspace(true);
                }}
              />
            ) : null}
          </div>
          {selectedWorkspace.branch ? (
            <div className="pill-wrap" ref={branchWrapRef}>
              <button
                type="button"
                className="workspace-branch workspace-trigger"
                aria-label="选择分支"
                aria-expanded={openMenu === "branch"}
                aria-haspopup="dialog"
                title="选择分支"
                onClick={(event) => {
                  toggleMenu("branch", event.currentTarget);
                }}
              >
                <GitBranch size={13} aria-hidden />
                {selectedWorkspace.branch}
                <ChevronDown size={11} aria-hidden />
              </button>
              {openMenu === "branch" ? (
                <PickerPopover
                  searchPlaceholder={branchCreating ? "新分支名称" : "搜索分支"}
                  searchLabel={branchCreating ? "新分支名称" : "搜索分支"}
                  search={branchSearch}
                  onSearch={setBranchSearch}
                  items={
                    branchCreating
                      ? []
                      : branchLoading
                        ? []
                        : (branchList?.branches ?? [])
                            .filter((branch) =>
                              branch.toLowerCase().includes(branchSearch.trim().toLowerCase()),
                            )
                            .map((branch) => ({
                              id: branch,
                              label: branch,
                              current: branch === selectedWorkspace.branch,
                            }))
                  }
                  emptyText={
                    branchCreating
                      ? "输入名称后创建并检出"
                      : branchLoading
                        ? "加载中…"
                        : "无匹配项"
                  }
                  onSelect={(branch) => {
                    if (branch === selectedWorkspace.branch) {
                      setOpenMenu(null);
                      return;
                    }
                    actions
                      .checkoutBranch(selectedWorkspace.id, branch, false)
                      .then(() => setOpenMenu(null))
                      .catch(() => {});
                  }}
                  footerLabel={branchCreating ? "创建并检出" : "创建并检出新分支"}
                  onFooter={() => {
                    if (!branchCreating) {
                      setBranchCreating(true);
                      setBranchSearch("");
                      return;
                    }
                    const name = branchSearch.trim();
                    if (name === "") return;
                    actions
                      .checkoutBranch(selectedWorkspace.id, name, true)
                      .then(() => setOpenMenu(null))
                      .catch(() => {});
                  }}
                />
              ) : null}
            </div>
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
              <ModelPillMenu
                models={models}
                defaultAlias={defaultModel}
                alias={model}
                reasoning={reasoning}
                configuredReasoning={configuredReasoning}
                onChange={(nextAlias, nextReasoning) => {
                  setModel(nextAlias);
                  setReasoning(nextReasoning);
                }}
                open={openMenu === "model"}
                onOpenChange={(next, button) => {
                  if (next) toggleMenu("model", button as HTMLButtonElement);
                  else setOpenMenu(null);
                }}
                wrapRef={modelWrapRef}
                showFollowRow
                pillAriaLabel="模型与推理（仅下一回合）"
                pillTitle="选择模型与推理强度（仅下一回合）"
                menuAriaLabel="选择模型与推理"
              />
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
      {addingWorkspace ? (
        <AddWorkspaceDialog
          onClose={() => setAddingWorkspace(false)}
          onAdded={(workspace) => actions.selectWorkspace(workspace.id)}
        />
      ) : null}
    </div>
  );
}
