/**
 * Three-zone picker popover (workspace / branch selection): a search field
 * on top, the selectable list in the middle and one footer action at the
 * bottom, separated by horizontal rules. Fully controlled — the parent owns
 * open state, search text, filtering and the footer action semantics (for
 * the branch picker the footer switches into create mode).
 */

import { Search } from "lucide-react";
import { useEffect, useRef } from "react";

export interface PickerItem {
  id: string;
  label: string;
  /** Marks the active entry; rendered with a check and no-op selection. */
  current?: boolean;
}

export function PickerPopover({
  searchPlaceholder,
  search,
  onSearch,
  searchLabel,
  items,
  onSelect,
  footerLabel,
  onFooter,
  emptyText = "无匹配项",
}: {
  searchPlaceholder: string;
  search: string;
  onSearch: (value: string) => void;
  searchLabel: string;
  items: PickerItem[];
  onSelect: (id: string) => void;
  footerLabel: string;
  onFooter: () => void;
  emptyText?: string;
}) {
  const searchRef = useRef<HTMLInputElement | null>(null);
  useEffect(() => {
    searchRef.current?.focus();
  }, []);

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    const panel = (event.target as HTMLElement).closest(".picker-popover");
    if (!(panel instanceof HTMLElement)) return;
    const controls = [
      ...panel.querySelectorAll<HTMLElement>(".picker-item:not([disabled])"),
    ];
    if (controls.length === 0) return;
    event.preventDefault();
    const current = controls.indexOf(document.activeElement as HTMLElement);
    const next =
      event.key === "ArrowDown"
        ? (current + 1 + controls.length) % controls.length
        : (current - 1 + controls.length) % controls.length;
    controls[next]?.focus();
  };

  return (
    <div
      className="pill-popover picker-popover"
      role="dialog"
      aria-label={searchLabel}
      onKeyDown={onKeyDown}
    >
      <div className="picker-zone picker-zone-search">
        <Search size={13} aria-hidden />
        <input
          ref={searchRef}
          type="text"
          value={search}
          placeholder={searchPlaceholder}
          aria-label={searchLabel}
          onChange={(event) => onSearch(event.target.value)}
        />
      </div>
      <div className="picker-zone picker-zone-list" role="listbox" aria-label={searchLabel}>
        {items.length === 0 ? (
          <p className="picker-empty">{emptyText}</p>
        ) : (
          items.map((item) => (
            <button
              key={item.id}
              type="button"
              role="option"
              aria-selected={item.current === true}
              className={`picker-item ${item.current === true ? "selected" : ""}`}
              onClick={() => onSelect(item.id)}
            >
              <span className="picker-item-label">{item.label}</span>
              {item.current === true ? <span className="picker-current">当前</span> : null}
            </button>
          ))
        )}
      </div>
      <div className="picker-zone picker-zone-footer">
        <button type="button" className="picker-footer-action" onClick={onFooter}>
          {footerLabel}
        </button>
      </div>
    </div>
  );
}
