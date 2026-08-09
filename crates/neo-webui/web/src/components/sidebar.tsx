/**
 * Sidebar: new-session entry, server-side title search, grouped session list
 * (pinned / normal / archived), hover icon actions, and one shared context
 * menu (right-click, menu key, Shift+F10) with rename/pin/archive. Hover
 * buttons and the menu never switch the current session.
 */

import {
  Archive,
  ArchiveRestore,
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
import type { WebUiSessionSummary, WebUiSummaryState } from "../protocol";
import { useAppActions, useAppState } from "../state/store";

export function summaryStateText(state: WebUiSummaryState): string {
  switch (state) {
    case "idle":
      return "空闲";
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

function formatUpdatedAt(updatedAt: string | null | undefined): string {
  if (!updatedAt) return "";
  const date = new Date(updatedAt);
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleString();
}

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

  const openMenuAt = (x: number, y: number, trigger: HTMLElement | null) => {
    onOpenMenu({ sessionId: summary.session_id, x, y }, trigger);
  };

  return (
    <li
      className={`session-row ${selected ? "selected" : ""}`}
      data-session-id={summary.session_id}
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
          <button type="submit" className="action-button">
            保存
          </button>
        </form>
      ) : (
        <>
          <button
            type="button"
            className="session-main"
            aria-current={selected ? "true" : undefined}
            onClick={() => actions.selectSession(summary.session_id)}
          >
            <span className="session-title">{summary.title ?? "未命名会话"}</span>
            <span className="session-meta">
              <span className={`session-state state-${summary.state}`}>
                {summaryStateText(summary.state)}
              </span>
              <span className="session-time">{formatUpdatedAt(summary.updated_at)}</span>
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

  const source = searchResults ?? state.summaries;
  const pinned = source.filter((entry) => entry.pinned && !entry.archived);
  const normal = source.filter((entry) => !entry.pinned && !entry.archived);
  const archived = source.filter((entry) => entry.archived);
  const byUpdated = (a: WebUiSessionSummary, b: WebUiSessionSummary) =>
    (b.updated_at ?? "").localeCompare(a.updated_at ?? "");
  pinned.sort(byUpdated);
  normal.sort(byUpdated);
  archived.sort(byUpdated);

  const menuSummary =
    state.activeContextMenu !== null
      ? state.summaries.find((entry) => entry.session_id === state.activeContextMenu) ??
        searchResults?.find((entry) => entry.session_id === state.activeContextMenu) ??
        null
      : null;

  const renderGroup = (label: string, items: WebUiSessionSummary[]) => {
    if (items.length === 0) return null;
    return (
      <div className="session-group" role="group" aria-label={label}>
        <div className="session-group-label">{label}</div>
        <ul className="session-list">
          {items.map((summary) => (
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
          ))}
        </ul>
      </div>
    );
  };

  return (
    <aside
      className={`sidebar ${state.sidebarDrawerOpen ? "drawer-open" : ""}`}
      style={{ width: state.sidebarWidth }}
      aria-label="会话列表"
    >
      <div className="sidebar-top">
        <button
          type="button"
          className="new-session-button"
          onClick={() => actions.selectSession(null)}
        >
          <Plus size={15} aria-hidden /> 新会话
        </button>
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
        {searchResults !== null
          ? renderGroup("搜索结果", [...pinned, ...normal])
          : (
            <>
              {renderGroup("已置顶", pinned)}
              {renderGroup("会话", normal)}
              {renderGroup("已归档", archived)}
            </>
          )}
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
