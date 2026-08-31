/**
 * Custom default-model selector (model alias + reasoning intensity together),
 * replacing the native `<select>` in settings. A pill opens a two-level
 * popover: a searchable model list grouped by provider, then a reasoning
 * submenu constrained to the selected model's capability. Mirrors the
 * composer's model pill UX.
 */

import { Check, ChevronRight, ChevronsUpDown } from "lucide-react";
import { useEffect, useRef, useState } from "react";
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

function selectedModelOf(
  models: WebUiModelInfo[],
  alias: string,
): WebUiModelInfo | undefined {
  return models.find((entry) => entry.alias === alias);
}

export function ModelReasoningSelector({
  models,
  alias,
  reasoning,
  onChange,
  disabled,
}: {
  models: WebUiModelInfo[];
  alias: string;
  reasoning: ReasoningSelection;
  onChange: (alias: string, reasoning: ReasoningSelection) => void;
  disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [pane, setPane] = useState<"root" | "models" | "reasoning">("root");
  const [query, setQuery] = useState("");
  const wrapRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!open) return;
    const onDocClick = (event: MouseEvent) => {
      if (
        wrapRef.current &&
        !wrapRef.current.contains(event.target as Node)
      ) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", onDocClick);
    return () => document.removeEventListener("mousedown", onDocClick);
  }, [open]);

  const selected = selectedModelOf(models, alias);
  const capability = selected?.reasoning ?? NO_REASONING;
  const effectiveReasoning = supportsReasoning(capability, reasoning)
    ? reasoning
    : supportsReasoning(capability, defaultReasoning(capability))
      ? defaultReasoning(capability)
      : { mode: "off" as const };
  const reasoningCapable = capability.type !== "none";
  const availableReasoning = reasoningChoices(capability);

  const needle = query.trim().toLowerCase();
  const filtered = needle
    ? models.filter((entry) =>
        (entry.display_name ?? entry.alias).toLowerCase().includes(needle),
      )
    : models;
  const grouped = filtered.reduce<Map<string, WebUiModelInfo[]>>((map, entry) => {
    const list = map.get(entry.provider) ?? [];
    list.push(entry);
    map.set(entry.provider, list);
    return map;
  }, new Map());

  const selectModel = (entry: WebUiModelInfo) => {
    const nextReasoning = supportsReasoning(entry.reasoning, effectiveReasoning)
      ? effectiveReasoning
      : defaultReasoning(entry.reasoning);
    onChange(entry.alias, nextReasoning);
    setPane("root");
    setQuery("");
  };

  const option = (entry: WebUiModelInfo) => (
    <button
      type="button"
      key={entry.alias}
      className={`model-row ${alias === entry.alias ? "selected" : ""}`}
      onClick={() => selectModel(entry)}
    >
      <span className="model-row-name">{entry.display_name ?? entry.alias}</span>
      <span className="model-row-meta">{entry.provider}</span>
      {alias === entry.alias ? <Check className="model-row-check" size={14} /> : null}
    </button>
  );

  return (
    <div className="model-selector-wrap" ref={wrapRef}>
      <button
        type="button"
        className="composer-pill model-pill settings-model-pill"
        disabled={disabled}
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={() => setOpen((prev) => !prev)}
      >
        <span className="model-pill-name">{selected?.display_name ?? (alias || "默认模型")}</span>
        {reasoningCapable ? (
          <span className="model-pill-reasoning">{reasoningLabel(effectiveReasoning)}</span>
        ) : null}
        <ChevronsUpDown size={13} className="model-pill-caret" aria-hidden />
      </button>

      {open ? (
        <div className="pill-popover model-settings-popover model-selector-popover">
          {pane === "root" ? (
            <div className="pill-popover-list model-settings-list">
              <button
                type="button"
                className="model-settings-row"
                onClick={() => setPane("models")}
              >
                <span className="model-settings-label">模型</span>
                <span className="model-settings-value">
                  {selected?.display_name ?? (alias || "默认模型")}
                </span>
                <ChevronRight size={14} className="model-settings-caret" aria-hidden />
              </button>
              <button
                type="button"
                className="model-settings-row"
                onClick={() => setPane("reasoning")}
              >
                <span className="model-settings-label">推理强度</span>
                <span className="model-settings-value">
                  {reasoningCapable ? reasoningLabel(effectiveReasoning) : "不支持"}
                </span>
                <ChevronRight size={14} className="model-settings-caret" aria-hidden />
              </button>
            </div>
          ) : null}

          {pane === "models" ? (
            <div className="pill-popover model-submenu" aria-label="选择模型">
              <div className="model-submenu-title">
                <button
                  type="button"
                  className="model-submenu-back"
                  aria-label="返回"
                  onClick={() => setPane("root")}
                >
                  <ChevronRight size={14} className="flip" aria-hidden />
                </button>
                <span>选择模型</span>
              </div>
              <input
                className="model-submenu-search"
                value={query}
                placeholder="搜索模型…"
                autoFocus
                onChange={(event) => setQuery(event.target.value)}
              />
              <div className="pill-popover-list model-catalog" role="listbox" aria-label="模型列表">
                {[...grouped].map(([provider, entries]) => (
                  <div className="model-provider-group" key={provider}>
                    <div className="model-provider-heading">{provider}</div>
                    {entries.map(option)}
                  </div>
                ))}
                {filtered.length === 0 ? (
                  <div className="model-catalog-empty">没有匹配的模型</div>
                ) : null}
              </div>
            </div>
          ) : null}

          {pane === "reasoning" ? (
            <div
              className="pill-popover model-submenu reasoning-submenu"
              aria-label="选择推理强度"
            >
              <div className="model-submenu-title">
                <button
                  type="button"
                  className="model-submenu-back"
                  aria-label="返回"
                  onClick={() => setPane("root")}
                >
                  <ChevronRight size={14} className="flip" aria-hidden />
                </button>
                <span>选择推理强度</span>
              </div>
              {availableReasoning.length === 0 ? (
                <div className="model-catalog-empty">该模型不支持推理</div>
              ) : (
                <div className="pill-popover-list model-catalog" role="listbox" aria-label="推理强度">
                  {availableReasoning.map((choice) => {
                    const selectedKey = reasoningKey(choice);
                    const active = selectedKey === reasoningKey(effectiveReasoning);
                    return (
                      <button
                        type="button"
                        className={`model-row ${active ? "selected" : ""}`}
                        key={selectedKey}
                        onClick={() => {
                          onChange(alias, choice);
                          setPane("root");
                        }}
                      >
                        <span className="model-row-name">{reasoningLabel(choice)}</span>
                        {active ? <Check className="model-row-check" size={14} /> : null}
                      </button>
                    );
                  })}
                </div>
              )}
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
