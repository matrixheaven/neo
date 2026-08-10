/**
 * Sidebar (redesign §6): server-side title search, a cross-workspace Pinned
 * section, then collapsible workspace groups (current workspace expanded with
 * a "+" new-session button, others collapsed, most-recent first). Group rows
 * exclude pinned sessions (they live in the Pinned section) and tuck archived
 * sessions behind a collapsed "已归档 n" entry. Hover icon actions and one
 * shared context menu (right-click, menu key, Shift+F10) never switch the
 * current session; closing the menu returns focus to the trigger row.
 */

import {
  Archive,
  ArchiveRestore,
  ChevronRight,
  FolderClosed,
  FolderOpen,
  Loader2,
  MoreHorizontal,
  Pin,
  PinOff,
  Plus,
  Search,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { listSessions } from "../api";
import type {
  WebUiSessionSummary,
  WebUiSummaryState,
  WebUiWorkspaceGroup,
} from "../protocol";
import { useAppActions, useAppState } from "../state/store";

export function summaryStateText(state: WebUiSummaryState): string | null {
  switch (state) {
    case "idle":
      return null;
    case "running":
      return "运行中";
    case "waiting_approval":
      return "等待确认";
    case "waiting_question":
      return "等待回答";
    case "failed":
      return "失败";
  }
}

function isWaitingState(state: WebUiSummaryState): boolean {
  return state === "waiting_approval" || state === "waiting_question";
}

function formatRelativeTime(updatedAt: string | null | undefined): string {
  if (!updatedAt) return "";
  const date = new Date(updatedAt);
  if (Number.isNaN(date.getTime())) return "";
  const diffMs = Date.now() - date.getTime();
  if (diffMs < 0) return date.toLocaleDateString();
  const minutes = Math.floor(diffMs / 60_000);
  if (minutes < 1) return "刚刚";
  if (minutes < 60) return `${minutes} 分钟前`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} 小时前`;
  const days = Math.floor(hours / 24);
  if (days < 7) return `${days} 天前`;
  return date.toLocaleDateString();
}

const byUpdated = (a: WebUiSessionSummary, b: WebUiSessionSummary) =>
  (b.updated_at ?? "").localeCompare(a.updated_at ?? "");

const SESSION_PAGE_SIZE = 5;

interface MenuState {
  sessionId: string;
  x: number;
  y: number;
}

function SessionMenu({
  menu,
  summary,
  onClose,
  onRename,
}: {
  menu: MenuState;
  summary: WebUiSessionSummary;
  onClose: () => void;
  onRename: (sessionId: string) => void;
}) {
  const actions = useAppActions();
  const menuRef = useRef<HTMLDivElement | null>(null);
  const itemRefs = useRef<Array<HTMLButtonElement | null>>([]);

  useEffect(() => {
    itemRefs.current[0]?.focus();
  }, []);

  useEffect(() => {
    const onPointerDown = (event: PointerEvent) => {
      if (menuRef.current && !menuRef.current.contains(event.target as Node)) {
        onClose();
      }
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, [onClose]);

  const onKeyDown = (event: React.KeyboardEvent) => {
    const items = itemRefs.current.filter((entry): entry is HTMLButtonElement => entry !== null);
    const index = items.indexOf(document.activeElement as HTMLButtonElement);
    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      items[(index + 1) % items.length]?.focus();
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      items[(index - 1 + items.length) % items.length]?.focus();
    } else if (event.key === "Tab") {
      event.preventDefault();
    }
  };

  const entries: Array<{ label: string; run: () => void }> = [
    { label: "重命名", run: () => onRename(summary.session_id) },
    {
      label: summary.pinned ? "取消置顶" : "置顶",
      run: () => actions.patchMetadata(summary.session_id, { pinned: !summary.pinned }),
    },
    {
      label: summary.archived ? "取消归档" : "归档",
      run: () => actions.patchMetadata(summary.session_id, { archived: !summary.archived }),
    },
  ];

  return (
    <div
      ref={menuRef}
      className="context-menu"
      role="menu"
      aria-label="会话操作"
      style={{ left: menu.x, top: menu.y }}
      onKeyDown={onKeyDown}
    >
      {entries.map((entry, index) => (
        <button
          key={entry.label}
          type="button"
          role="menuitem"
          className="context-menu-item"
          ref={(element) => {
            itemRefs.current[index] = element;
          }}
          onClick={() => {
            entry.run();
            onClose();
          }}
        >
          {entry.label}
        </button>
      ))}
    </div>
  );
}

function SessionRow({
  summary,
  selected,
  renaming,
  onOpenMenu,
  onRenameSubmit,
  onRenameCancel,
}: {
  summary: WebUiSessionSummary;
  selected: boolean;
  renaming: boolean;
  onOpenMenu: (menu: MenuState, trigger: HTMLElement | null) => void;
  onRenameSubmit: (title: string) => void;
  onRenameCancel: () => void;
}) {
  const actions = useAppActions();
  const [renameValue, setRenameValue] = useState(summary.title ?? "");
  const stateText = summaryStateText(summary.state);
  const title = summary.title ?? "未命名会话";
  const updatedTime = formatRelativeTime(summary.updated_at);
  const sessionTooltip = [
    title,
    summary.workspace_label ? `工作区：${summary.workspace_label}` : null,
    `状态：${stateText ?? "空闲"}`,
    updatedTime ? `更新时间：${updatedTime}` : null,
  ]
    .filter((line): line is string => line !== null)
    .join("\n");

  const openMenuAt = (x: number, y: number, trigger: HTMLElement | null) => {
    onOpenMenu({ sessionId: summary.session_id, x, y }, trigger);
  };

  return (
    <li
      className={`session-row ${selected ? "selected" : ""}`}
      data-session-id={summary.session_id}
      // Programmatically focusable (not in the Tab order) so closing the
      // context menu can return focus to a right-clicked row.
      tabIndex={-1}
      onContextMenu={(event) => {
        event.preventDefault();
        openMenuAt(event.clientX, event.clientY, event.currentTarget);
      }}
      onKeyDown={(event) => {
        if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
          event.preventDefault();
          const rect = event.currentTarget.getBoundingClientRect();
          openMenuAt(rect.left + 24, rect.bottom, event.target as HTMLElement);
        }
      }}
    >
      {renaming ? (
        <form
          className="rename-form"
          onSubmit={(event) => {
            event.preventDefault();
            onRenameSubmit(renameValue.trim());
          }}
        >
          <input
            aria-label="会话标题"
            value={renameValue}
            autoFocus
            onChange={(event) => setRenameValue(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                event.preventDefault();
                onRenameCancel();
              }
            }}
            onClick={(event) => event.stopPropagation()}
          />
          <button type="submit" className="chip-button">
            保存
          </button>
        </form>
      ) : (
        <>
          <button
            type="button"
            className="session-main"
            aria-current={selected ? "true" : undefined}
            title={sessionTooltip}
            onClick={() => actions.selectSession(summary.session_id)}
          >
            <span className="session-title-row">
              <span className="session-activity">
                {summary.state === "running" ? (
                  <span role="status" aria-label="运行中">
                    <Loader2 size={12} className="spin" aria-hidden />
                  </span>
                ) : null}
              </span>
              <span className="session-title">{title}</span>
              {isWaitingState(summary.state) ? (
                // The summary carries no pending-count field; the state text
                // is the badge label (no fabricated counts).
                <span className={`session-badge state-${summary.state}`}>
                  {stateText}
                </span>
              ) : summary.state !== "running" && stateText !== null ? (
                <span className={`session-state state-${summary.state}`}>
                  {stateText}
                </span>
              ) : null}
            </span>
          </button>
          <span className="session-hover-actions">
            <button
              type="button"
              className="icon-button"
              aria-label={summary.pinned ? "取消置顶" : "置顶"}
              title={summary.pinned ? "取消置顶" : "置顶"}
              onClick={(event) => {
                event.stopPropagation();
                actions.patchMetadata(summary.session_id, { pinned: !summary.pinned });
              }}
            >
              {summary.pinned ? <PinOff size={14} aria-hidden /> : <Pin size={14} aria-hidden />}
            </button>
            <button
              type="button"
              className="icon-button"
              aria-label={summary.archived ? "取消归档" : "归档"}
              title={summary.archived ? "取消归档" : "归档"}
              onClick={(event) => {
                event.stopPropagation();
                actions.patchMetadata(summary.session_id, { archived: !summary.archived });
              }}
            >
              {summary.archived ? (
                <ArchiveRestore size={14} aria-hidden />
              ) : (
                <Archive size={14} aria-hidden />
              )}
            </button>
            <button
              type="button"
              className="icon-button"
              aria-label="更多操作"
              title="更多操作"
              aria-haspopup="menu"
              onClick={(event) => {
                event.stopPropagation();
                const rect = event.currentTarget.getBoundingClientRect();
                openMenuAt(rect.left, rect.bottom + 4, event.currentTarget);
              }}
            >
              <MoreHorizontal size={14} aria-hidden />
            </button>
          </span>
        </>
      )}
    </li>
  );
}

export function Sidebar() {
  const state = useAppState();
  const actions = useAppActions();
  const [query, setQuery] = useState("");
  const [searchResults, setSearchResults] = useState<WebUiSessionSummary[] | null>(null);
  const [renamingId, setRenamingId] = useState<string | null>(null);
  /** Per-group expanded overrides; absent = current expanded, others collapsed. */
  const [expandedOverrides, setExpandedOverrides] = useState<Record<string, boolean>>({});
  /** Per-group archived-section open flags; absent = collapsed. */
  const [archivedOpen, setArchivedOpen] = useState<Record<string, boolean>>({});
  /** Per-group session counts; absent starts at the compact default. */
  const [visibleSessionCounts, setVisibleSessionCounts] = useState<Record<string, number>>({});
  const triggerRef = useRef<HTMLElement | null>(null);
  const menuPositionRef = useRef({ x: 0, y: 0 });

  const openMenu = useCallback(
    (next: MenuState, trigger: HTMLElement | null) => {
      triggerRef.current = trigger;
      menuPositionRef.current = { x: next.x, y: next.y };
      actions.setContextMenu(next.sessionId);
    },
    [actions],
  );

  const closeMenu = useCallback(() => {
    actions.setContextMenu(null);
    // Return focus to the session row that opened the menu.
    const trigger = triggerRef.current;
    if (trigger && document.contains(trigger)) {
      trigger.focus();
    }
    triggerRef.current = null;
  }, [actions]);

  // Server-side title search only; no transcript scanning, no local index.
  useEffect(() => {
    if (query.trim() === "") {
      setSearchResults(null);
      return;
    }
    const handle = window.setTimeout(() => {
      listSessions({ scope: "active", query: query.trim() })
        .then((page) => setSearchResults(page.items))
        .catch(() => setSearchResults([]));
    }, 250);
    return () => window.clearTimeout(handle);
  }, [query]);

  const renderRow = (summary: WebUiSessionSummary) => (
    <SessionRow
      key={summary.session_id}
      summary={summary}
      selected={state.selectedSessionId === summary.session_id}
      renaming={renamingId === summary.session_id}
      onOpenMenu={openMenu}
      onRenameSubmit={(title) => {
        if (title !== "") {
          actions.patchMetadata(summary.session_id, { title });
        }
        setRenamingId(null);
      }}
      onRenameCancel={() => setRenamingId(null)}
    />
  );

  const menuSummary =
    state.activeContextMenu !== null
      ? state.summaries.find((entry) => entry.session_id === state.activeContextMenu) ??
        searchResults?.find((entry) => entry.session_id === state.activeContextMenu) ??
        null
      : null;

  const renderSearchResults = () => {
    const source = searchResults ?? [];
    const items = [...source.filter((entry) => !entry.archived)].sort(byUpdated);
    if (items.length === 0) return null;
    return (
      <div className="session-group" role="group" aria-label="搜索结果">
        <div className="session-group-static">搜索结果</div>
        <ul className="session-list">{items.map(renderRow)}</ul>
      </div>
    );
  };

  const renderGrouped = () => {
    // Grouped aggregation is the source of truth; before the first workspace
    // snapshot, fall back to one current-workspace group from the bootstrap.
    const groups: WebUiWorkspaceGroup[] =
      state.workspaces.length > 0
        ? state.workspaces
        : [
            {
              label: state.bootstrap?.workspace_label ?? "当前工作区",
              current: true,
              sessions: state.summaries,
            },
          ];

    // Pinned sessions surface once, cross-workspace, in the Pinned section.
    const pinned = groups
      .flatMap((group) => group.sessions)
      .filter((entry) => entry.pinned && !entry.archived)
      .sort(byUpdated);
    const pinnedIds = new Set(pinned.map((entry) => entry.session_id));

    // Current workspace first; the rest by most recent activity.
    const ordered = groups
      .map((group, index) => ({ group, index }))
      .sort((a, b) => {
        if (a.group.current !== b.group.current) return a.group.current ? -1 : 1;
        const latest = (entry: (typeof a)["group"]) =>
          entry.sessions.reduce<string>(
            (max, session) => ((session.updated_at ?? "") > max ? (session.updated_at ?? "") : max),
            "",
          );
        return latest(b.group).localeCompare(latest(a.group)) || a.index - b.index;
      });

    return (
      <>
        {pinned.length > 0 ? (
          <div className="session-group" role="group" aria-label="已置顶">
            <div className="session-group-static">已置顶</div>
            <ul className="session-list">{pinned.map(renderRow)}</ul>
          </div>
        ) : null}
        {ordered.map(({ group }) => {
          const live = group.sessions.filter(
            (entry) => !entry.archived && !pinnedIds.has(entry.session_id),
          );
          const archived = group.sessions.filter((entry) => entry.archived);
          // Empty groups never render; a lone current workspace still renders
          // as a group (no flat special case).
          if (live.length === 0 && archived.length === 0) return null;
          const expanded = expandedOverrides[group.label] ?? group.current;
          const archivedExpanded = archivedOpen[group.label] ?? false;
          const sortedLive = [...live].sort(byUpdated);
          const visibleCount = visibleSessionCounts[group.label] ?? SESSION_PAGE_SIZE;
          const visibleLive = sortedLive.slice(0, visibleCount);
          return (
            <div className="session-group" role="group" aria-label={group.label} key={group.label}>
              <div className="session-group-header">
                <button
                  type="button"
                  className="session-group-toggle"
                  aria-expanded={expanded}
                  onClick={() =>
                    setExpandedOverrides((previous) => ({
                      ...previous,
                      [group.label]: !expanded,
                    }))
                  }
                >
                  {expanded ? (
                    <FolderOpen size={14} aria-hidden className="session-group-folder" />
                  ) : (
                    <FolderClosed size={14} aria-hidden className="session-group-folder" />
                  )}
                  <span className="session-group-label">{group.label}</span>
                  {/* Count matches the visible rows: archived are behind the
                      collapsed entry, pinned live in the Pinned section. */}
                  <span className="session-group-count">{live.length}</span>
                </button>
                {group.current ? (
                  <button
                    type="button"
                    className="icon-button session-group-new"
                    aria-label="新会话"
                    title="新会话"
                    onClick={() => actions.selectSession(null)}
                  >
                    <Plus size={14} aria-hidden />
                  </button>
                ) : null}
              </div>
              {expanded ? (
                <>
                  {live.length > 0 ? (
                    <ul className="session-list">{visibleLive.map(renderRow)}</ul>
                  ) : null}
                  {visibleLive.length < sortedLive.length || visibleCount > SESSION_PAGE_SIZE ? (
                    <div className="session-group-page-actions">
                      {visibleLive.length < sortedLive.length ? (
                        <button
                          type="button"
                          className="session-group-more"
                          onClick={() =>
                            setVisibleSessionCounts((previous) => ({
                              ...previous,
                              [group.label]:
                                (previous[group.label] ?? SESSION_PAGE_SIZE) + SESSION_PAGE_SIZE,
                            }))
                          }
                        >
                          展开更多
                        </button>
                      ) : null}
                      {visibleCount > SESSION_PAGE_SIZE ? (
                        <button
                          type="button"
                          className="session-group-more"
                          onClick={() =>
                            setVisibleSessionCounts((previous) => ({
                              ...previous,
                              [group.label]: SESSION_PAGE_SIZE,
                            }))
                          }
                        >
                          收起
                        </button>
                      ) : null}
                    </div>
                  ) : null}
                  {archived.length > 0 ? (
                    <div className="session-archived">
                      <button
                        type="button"
                        className="session-archived-toggle"
                        aria-expanded={archivedExpanded}
                        onClick={() =>
                          setArchivedOpen((previous) => ({
                            ...previous,
                            [group.label]: !archivedExpanded,
                          }))
                        }
                      >
                        <ChevronRight
                          size={12}
                          aria-hidden
                          className={`session-group-caret ${archivedExpanded ? "expanded" : ""}`}
                        />
                        已归档 {archived.length}
                      </button>
                      {archivedExpanded ? (
                        <ul className="session-list">
                          {[...archived].sort(byUpdated).map(renderRow)}
                        </ul>
                      ) : null}
                    </div>
                  ) : null}
                </>
              ) : null}
            </div>
          );
        })}
      </>
    );
  };

  return (
    <aside
      id="session-sidebar"
      className={`sidebar ${state.sidebarDrawerOpen ? "drawer-open" : ""} ${
        state.sidebarCollapsed ? "sidebar-collapsed" : ""
      }`}
      aria-label="会话列表"
    >
      <div className="sidebar-top">
        <div className="search-box">
          <Search size={14} aria-hidden />
          <input
            type="search"
            aria-label="搜索会话标题"
            placeholder="搜索会话标题"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
        </div>
      </div>
      <div className="sidebar-scroll">
        {searchResults !== null ? renderSearchResults() : renderGrouped()}
      </div>
      {state.activeContextMenu !== null && menuSummary !== null ? (
        <SessionMenu
          menu={{
            sessionId: state.activeContextMenu,
            x: menuPositionRef.current.x,
            y: menuPositionRef.current.y,
          }}
          summary={menuSummary}
          onClose={closeMenu}
          onRename={(sessionId) => setRenamingId(sessionId)}
        />
      ) : null}
    </aside>
  );
}
