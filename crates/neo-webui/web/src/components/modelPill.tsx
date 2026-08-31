/**
 * Shared model + reasoning pill menu — THE model picker UI (extracted from the
 * composer so the settings default-model selector renders exactly the same
 * thing instead of a drifting copy).
 *
 * Fully controlled: the owner owns `open` and the selection (`alias` with ""
 * meaning "follow the session default", plus `reasoning` with null meaning the
 * same); this component owns only the submenu pane, the search query and the
 * custom budget input, and closes itself on outside click / Escape.
 */

import { Check, ChevronDown, ChevronLeft, ChevronRight, Search } from "lucide-react";
import { useEffect, useRef, useState, type KeyboardEvent, type RefObject } from "react";
import type {
  ReasoningCapability,
  ReasoningSelection,
  WebUiModelInfo,
} from "../protocol";

const REASONING_LABELS: Record<string, string> = {
  minimal: "极简",
  low: "低",
  medium: "中",
  high: "高",
  xhigh: "极高",
  max: "最大",
};

const NO_REASONING: ReasoningCapability = { type: "none" };

/** Compact token count: 83700 → "83.7k", 256000 → "256k". */
export function formatTokens(value: number): string {
  if (value >= 1000) {
    const k = value / 1000;
    return `${Number.isInteger(k) ? k : k.toFixed(1)}k`;
  }
  return String(value);
}

export function reasoningLabel(selection: ReasoningSelection): string {
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

export function supportsReasoning(
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
    choices.push(
      ...values.map((budget_tokens) => ({ mode: "budget_tokens" as const, budget_tokens })),
    );
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

type Pane = "root" | "models" | "reasoning";

export function ModelPillMenu({
  models,
  defaultAlias,
  alias,
  reasoning,
  configuredReasoning,
  onChange,
  open,
  onOpenChange,
  wrapRef,
  direction = "up",
  showFollowRow = false,
  pillClassName,
  pillAriaLabel,
  pillTitle,
  menuAriaLabel,
}: {
  models: WebUiModelInfo[];
  /** Alias displayed when `alias` is "" (follow the session default). */
  defaultAlias: string;
  /** Selected alias; "" means follow `defaultAlias`. */
  alias: string;
  /** Selected reasoning; null means follow `configuredReasoning`. */
  reasoning: ReasoningSelection | null;
  configuredReasoning: ReasoningSelection;
  onChange: (alias: string, reasoning: ReasoningSelection | null) => void;
  open: boolean;
  onOpenChange: (open: boolean, button?: HTMLButtonElement) => void;
  /** External wrapper ref (the composer tracks it for focus return). */
  wrapRef?: RefObject<HTMLDivElement>;
  /** Popover direction: the composer opens up, the settings page opens down. */
  direction?: "up" | "down";
  /** Offer the "跟随会话配置" row (composer only). */
  showFollowRow?: boolean;
  pillClassName?: string;
  pillAriaLabel: string;
  pillTitle: string;
  menuAriaLabel: string;
}) {
  const [pane, setPane] = useState<Pane>("root");
  const [query, setQuery] = useState("");
  const [budgetInput, setBudgetInput] = useState("");
  const internalWrapRef = useRef<HTMLDivElement | null>(null);
  const wrap = wrapRef ?? internalWrapRef;

  // Reopening starts from the root pane with a clear search.
  useEffect(() => {
    if (open) {
      setPane("root");
      setQuery("");
      setBudgetInput("");
    }
  }, [open]);

  // Outside click / Escape close the menu.
  useEffect(() => {
    if (!open) return;
    const element = wrap.current;
    const onPointerDown = (event: MouseEvent) => {
      if (element && !element.contains(event.target as Node)) {
        onOpenChange(false);
      }
    };
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") onOpenChange(false);
    };
    document.addEventListener("mousedown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("mousedown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open, onOpenChange, wrap]);

  const activeAlias = alias === "" ? defaultAlias : alias;
  const selectedModel: WebUiModelInfo | undefined = models.find(
    (entry) => entry.alias === activeAlias,
  );
  const capability = selectedModel?.reasoning ?? NO_REASONING;
  const effectiveReasoning =
    reasoning !== null && supportsReasoning(capability, reasoning)
      ? reasoning
      : supportsReasoning(capability, configuredReasoning)
        ? configuredReasoning
        : defaultReasoning(capability);
  const reasoningCapable = capability.type !== "none";
  const availableReasoning = reasoningChoices(capability);
  const bounds = budgetBounds(capability);
  const customBudget = Number(budgetInput);
  const customBudgetValid =
    budgetInput !== "" &&
    Number.isSafeInteger(customBudget) &&
    customBudget >= 0 &&
    customBudget <= 4_294_967_295 &&
    supportsReasoning(capability, { mode: "budget_tokens", budget_tokens: customBudget });

  const needle = query.trim().toLowerCase();
  const filteredModels = models.filter((entry) => {
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

  const selectModel = (entry: WebUiModelInfo | null) => {
    if (entry === null) {
      onChange("", null);
      setPane("root");
      return;
    }
    onChange(
      entry.alias,
      supportsReasoning(entry.reasoning, effectiveReasoning)
        ? effectiveReasoning
        : defaultReasoning(entry.reasoning),
    );
    setPane("root");
  };

  const modelOption = (entry: WebUiModelInfo) => {
    const selected = alias !== "" && activeAlias === entry.alias;
    return (
      <button
        type="button"
        key={entry.alias}
        role="option"
        aria-selected={selected}
        className={`model-row ${selected ? "selected" : ""}`}
        onClick={() => selectModel(entry)}
      >
        <span className="model-row-name">{entry.display_name ?? entry.alias}</span>
        <span className="model-row-meta">
          {entry.display_name ? entry.alias : entry.provider}
          {entry.context_window ? ` · ${formatTokens(entry.context_window)}` : ""}
        </span>
        {selected ? <Check className="model-row-check" size={14} /> : null}
      </button>
    );
  };

  const onMenuKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    if (event.target instanceof HTMLInputElement && event.target.type === "number") return;
    const panel = (event.target as HTMLElement).closest(".pill-popover");
    if (!(panel instanceof HTMLElement)) return;
    const controls = [
      ...panel.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled)"),
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
    <div className="pill-wrap" ref={wrap}>
      <button
        type="button"
        className={`composer-pill model-pill${pillClassName ? ` ${pillClassName}` : ""}`}
        aria-label={pillAriaLabel}
        aria-expanded={open}
        aria-haspopup="dialog"
        title={pillTitle}
        onClick={(event) => {
          onOpenChange(!open, event.currentTarget);
        }}
      >
        <span className="model-pill-name">
          {selectedModel?.display_name ?? (activeAlias || "默认模型")}
        </span>
        {reasoningCapable ? (
          <span className="model-pill-reasoning">{reasoningLabel(effectiveReasoning)}</span>
        ) : null}
        <ChevronDown size={12} aria-hidden />
      </button>
      {open ? (
        <div
          className="model-menu-shell"
          data-pane={pane}
          data-direction={direction}
          role="dialog"
          aria-label={menuAriaLabel}
          onKeyDown={onMenuKeyDown}
        >
          <div className="pill-popover model-settings-popover">
            <div className="pill-popover-list model-settings-list">
              <button
                type="button"
                autoFocus
                className={`model-settings-row ${pane === "models" ? "selected" : ""}`}
                onClick={() => setPane("models")}
              >
                <span>模型</span>
                <span className="model-settings-value">
                  {selectedModel?.display_name ?? (activeAlias || "默认模型")}
                  <ChevronRight size={14} aria-hidden />
                </span>
              </button>
              {reasoningCapable ? (
                <button
                  type="button"
                  className={`model-settings-row ${pane === "reasoning" ? "selected" : ""}`}
                  onClick={() => setPane("reasoning")}
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
          {pane === "models" ? (
            <div className="pill-popover model-submenu" aria-label="选择模型">
              <div className="model-submenu-title">
                <button
                  type="button"
                  className="model-submenu-back"
                  aria-label="返回"
                  onClick={() => setPane("root")}
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
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                />
              </div>
              <div className="pill-popover-list model-catalog" role="listbox" aria-label="模型列表">
                {showFollowRow ? (
                  <button
                    type="button"
                    role="option"
                    aria-selected={alias === ""}
                    className={`model-row ${alias === "" ? "selected" : ""}`}
                    onClick={() => selectModel(null)}
                  >
                    <span className="model-row-name">跟随会话配置</span>
                    <span className="model-row-meta">{defaultAlias || "默认模型"}</span>
                    {alias === "" ? <Check className="model-row-check" size={14} /> : null}
                  </button>
                ) : null}
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
          {pane === "reasoning" ? (
            <div className="pill-popover model-submenu reasoning-submenu" aria-label="选择推理强度">
              <div className="model-submenu-title">
                <button
                  type="button"
                  className="model-submenu-back"
                  aria-label="返回"
                  onClick={() => setPane("root")}
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
                        onChange(alias, choice);
                        setPane("root");
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
                        onChange(alias, { mode: "budget_tokens", budget_tokens: customBudget });
                        setPane("root");
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
  );
}
