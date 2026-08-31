/**
 * Full-page Settings (sidebar-form, not a modal). Left nav groups the
 * sections; the right pane renders the active section. Config-backed fields
 * (default model, permission mode, appearance, providers/models, MCP) persist
 * to `~/.neo/config.toml`; appearance is applied to the page on save.
 */

import { ArrowLeft, Pencil, Plus, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import {
  addModel,
  addProvider,
  fetchSettings,
  removeMcpServer,
  removeModel,
  removeProvider,
  setAppearance,
  setDefaultModelSelection,
  setMcpServerEnabled,
  setPermissionMode,
  upsertMcpServer,
} from "../api";
import type {
  ReasoningCapability,
  ReasoningSelection,
  WebUiAppearance,
  WebUiMcpServerEdit,
  WebUiMcpServerInfo,
  WebUiModelEdit,
  WebUiProviderEdit,
  WebUiSettingsSnapshot,
} from "../protocol";
import { useAppActions } from "../state/store";
import { ModelReasoningSelector } from "./modelReasoningSelector";

type NavItem =
  | "general"
  | "appearance"
  | "models"
  | "mcp"
  | "skills";

const NAV_GROUPS: { label: string; items: { id: NavItem; label: string }[] }[] = [
  {
    label: "基础设置",
    items: [
      { id: "general", label: "常规" },
      { id: "appearance", label: "外观" },
      { id: "models", label: "模型设置" },
    ],
  },
  {
    label: "Agent 能力",
    items: [
      { id: "mcp", label: "MCP 服务器" },
      { id: "skills", label: "技能" },
    ],
  },
];

const PERMISSION_LABELS: Record<string, string> = {
  ask: "逐条确认",
  auto: "自动",
  yolo: "免确认",
};

const API_TYPE_OPTIONS = [
  { value: "openai", label: "OpenAI 兼容 (/v1/chat/completions)" },
  { value: "openai_response", label: "OpenAI Responses (/v1/responses)" },
  { value: "anthropic", label: "Anthropic Messages (/v1/messages)" },
  { value: "google", label: "Google Generative AI" },
];

const TRANSPORTS = ["stdio", "http", "sse"] as const;

const THEME_OPTIONS = [
  { value: "system", label: "自动" },
  { value: "light", label: "浅色" },
  { value: "dark", label: "深色" },
];

const CODE_THEME_OPTIONS = [
  { value: "auto", label: "跟随界面" },
  { value: "light", label: "浅色" },
  { value: "dark", label: "深色" },
];

const SAMPLE_CODE = `const themePreview = ThemeConfig {
  surface: "sidebar",
  accent: "#7aa2f7",
  contrast: 45,
};`;

function numberInput(
  label: string,
  value: number,
  onChange: (value: number) => void,
  min = 1,
  max = 64,
) {
  return (
    <label className="settings-field">
      <span className="settings-label">{label}</span>
      <input
        type="number"
        className="settings-input settings-input-number"
        value={value}
        min={min}
        max={max}
        onChange={(event) => {
          const parsed = Number.parseInt(event.target.value, 10);
          if (Number.isFinite(parsed)) onChange(parsed);
        }}
      />
    </label>
  );
}

function Toggle({
  checked,
  onChange,
  label,
}: {
  checked: boolean;
  onChange: (value: boolean) => void;
  label: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      className={`settings-switch ${checked ? "on" : ""}`}
      onClick={() => onChange(!checked)}
    >
      {label}
    </button>
  );
}

function Segmented<T extends string>({
  options,
  value,
  onChange,
  ariaLabel,
}: {
  options: { value: T; label: string }[];
  value: T;
  onChange: (value: T) => void;
  ariaLabel: string;
}) {
  return (
    <div className="settings-segmented" role="group" aria-label={ariaLabel}>
      {options.map((option) => (
        <button
          type="button"
          key={option.value}
          className={value === option.value ? "active" : ""}
          onClick={() => onChange(option.value)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------------------
// 常规
// ---------------------------------------------------------------------------

function GeneralSection({
  snapshot,
  onChanged,
  onError,
  onNotice,
}: {
  snapshot: WebUiSettingsSnapshot;
  onChanged: (next: WebUiSettingsSnapshot) => void;
  onError: (message: string) => void;
  onNotice: (message: string) => void;
}) {
  const [saving, setSaving] = useState(false);

  const saveModel = (alias: string, reasoning: ReasoningSelection) => {
    if (saving) return;
    setSaving(true);
    setDefaultModelSelection(alias, reasoning)
      .then((next) => {
        onChanged(next);
        onNotice(`默认模型已设为 ${alias}（重启服务后对新会话生效）`);
      })
      .catch(() => onError("无法保存默认模型（请确认模型可用）"))
      .finally(() => setSaving(false));
  };

  const savePermission = (mode: string) => {
    if (mode === snapshot.permission_mode || saving) return;
    setSaving(true);
    setPermissionMode(mode)
      .then(onChanged)
      .catch(() => onError("无法保存权限模式"))
      .finally(() => setSaving(false));
  };

  return (
    <section className="settings-group">
      <h3>常规</h3>
      <div className="settings-field">
        <span className="settings-label">默认模型</span>
        <ModelReasoningSelector
          models={snapshot.models}
          alias={snapshot.default_model}
          reasoning={snapshot.default_reasoning}
          disabled={saving}
          onChange={saveModel}
        />
        <p className="settings-hint">选择模型并配置推理强度；写入 ~/.neo/config.toml，重启后对新会话生效。</p>
      </div>
      <div className="settings-field">
        <span className="settings-label">默认权限模式</span>
        <Segmented
          ariaLabel="默认权限模式"
          options={Object.entries(PERMISSION_LABELS).map(([value, label]) => ({ value, label }))}
          value={snapshot.permission_mode}
          onChange={savePermission}
        />
        <p className="settings-hint">新会话默认使用该工具授权方式；会话内仍可临时切换。</p>
      </div>
    </section>
  );
}

// ---------------------------------------------------------------------------
// 外观
// ---------------------------------------------------------------------------

function AppearanceSection({
  appearance,
  onSaved,
  onError,
  onNotice,
}: {
  appearance: WebUiAppearance;
  onSaved: (next: WebUiAppearance) => void;
  onError: (message: string) => void;
  onNotice: (message: string) => void;
}) {
  const [draft, setDraft] = useState<WebUiAppearance>(appearance);
  const [saving, setSaving] = useState(false);
  const [dirty, setDirty] = useState(false);

  const update = (patch: Partial<WebUiAppearance>) => {
    setDraft((prev) => ({ ...prev, ...patch }));
    setDirty(true);
  };

  const save = () => {
    if (saving) return;
    setSaving(true);
    setAppearance(draft)
      .then((next) => {
        onSaved(next.appearance);
        setDirty(false);
        onNotice("外观已保存");
      })
      .catch(() => onError("无法保存外观（请检查数值）"))
      .finally(() => setSaving(false));
  };

  return (
    <section className="settings-group">
      <h3>外观</h3>
      <div className="settings-card">
        <div className="settings-card-title">界面设置</div>
        <p className="settings-hint">设置应用主题和界面文字大小。</p>
        <div className="settings-field-row">
          <div className="settings-field">
            <span className="settings-label">界面主题</span>
            <Segmented
              ariaLabel="界面主题"
              options={THEME_OPTIONS}
              value={draft.theme as "system" | "light" | "dark"}
              onChange={(theme) => update({ theme })}
            />
          </div>
          {numberInput("界面字号", draft.ui_font_size, (ui_font_size) => update({ ui_font_size }))}
        </div>
      </div>

      <div className="settings-card">
        <div className="settings-card-title">代码设置</div>
        <p className="settings-hint">设置代码内容的主题、字号和显示方式。</p>
        <div className="settings-field">
          <span className="settings-label">代码主题</span>
          <Segmented
            ariaLabel="代码主题"
            options={CODE_THEME_OPTIONS}
            value={draft.code_theme as "auto" | "light" | "dark"}
            onChange={(code_theme) => update({ code_theme })}
          />
        </div>
        <div className="settings-field settings-field-inline">
          <span className="settings-label">显示行号</span>
          <Toggle
            checked={draft.show_line_numbers}
            onChange={(show_line_numbers) => update({ show_line_numbers })}
            label={draft.show_line_numbers ? "开" : "关"}
          />
        </div>
        <div className="settings-field settings-field-inline">
          <span className="settings-label">长行自动换行</span>
          <Toggle
            checked={draft.word_wrap}
            onChange={(word_wrap) => update({ word_wrap })}
            label={draft.word_wrap ? "开" : "关"}
          />
        </div>
        {numberInput("代码字号", draft.code_font_size, (code_font_size) => update({ code_font_size }))}
      </div>

      <div className="settings-card">
        <div className="settings-card-title">代码预览</div>
        <p className="settings-hint">同时预览浅色与深色代码主题，当前界面使用的主题将标记为「当前生效」。</p>
        <div className="code-preview-grid">
          {(["light", "dark"] as const).map((theme) => {
            const resolvedInterface =
              draft.theme === "system"
                ? typeof window.matchMedia === "function" &&
                  window.matchMedia("(prefers-color-scheme: light)").matches
                  ? "light"
                  : "dark"
                : draft.theme;
            const active =
              draft.code_theme === "auto"
                ? resolvedInterface === theme
                : draft.code_theme === theme;
            return (
              <div
                key={theme}
                className={`code-preview-card ${active ? "active" : ""}`}
                data-preview-theme={theme}
                data-code-theme={draft.code_theme === "auto" ? theme : draft.code_theme}
                data-line-numbers={draft.show_line_numbers ? "on" : "off"}
                data-word-wrap={draft.word_wrap ? "on" : "off"}
              >
                <div className="code-preview-card-head">
                  <span className="code-preview-title">{theme === "light" ? "浅色预览" : "深色预览"}</span>
                  <span className="code-preview-status">{active ? "当前生效" : "取消"}</span>
                </div>
                <pre className="code-preview-pre" style={{ fontSize: `${draft.code_font_size}px` }}>
                  <code>{SAMPLE_CODE}</code>
                </pre>
              </div>
            );
          })}
        </div>
      </div>

      <div className="settings-mcp-form-actions">
        <button
          type="button"
          className="primary-button"
          disabled={saving || !dirty}
          onClick={save}
        >
          {saving ? "保存中…" : "保存外观"}
        </button>
      </div>
    </section>
  );
}

// ---------------------------------------------------------------------------
// 模型设置 / 提供方
// ---------------------------------------------------------------------------

function ModelEditorModal({
  providerId,
  onCancel,
  onSave,
}: {
  providerId: string;
  onCancel: () => void;
  onSave: (model: WebUiModelEdit) => void;
}) {
  const [modelId, setModelId] = useState("");
  const [alias, setAlias] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [context, setContext] = useState("1000000");
  const [maxOutput, setMaxOutput] = useState("128000");
  const [inputTypes, setInputTypes] = useState<{ text: boolean; image: boolean; video: boolean }>({
    text: true,
    image: false,
    video: false,
  });
  const [outputText, setOutputText] = useState(true);

  const save = () => {
    const capabilities: string[] = ["streaming", "tools"];
    if (!inputTypes.text) capabilities.push("images");
    const reasoning: ReasoningCapability = { type: "none" };
    onSave({
      alias: alias.trim() || `${providerId}/${modelId.trim()}`,
      provider: providerId,
      model: modelId.trim(),
      display_name: displayName.trim() ? displayName.trim() : null,
      max_context_tokens: Number.parseInt(context, 10) || null,
      max_output_tokens: Number.parseInt(maxOutput, 10) || null,
      capabilities,
      reasoning,
    });
  };

  return (
    <div className="dialog-backdrop" role="presentation">
      <div className="settings-modal" role="dialog" aria-modal="true" aria-label="添加模型">
        <div className="settings-modal-head">
          <h3>添加模型</h3>
        </div>
        <label className="settings-field">
          <span className="settings-label">模型 ID</span>
          <input
            className="settings-input"
            value={modelId}
            placeholder="模型 ID"
            onChange={(event) => setModelId(event.target.value)}
          />
        </label>
        <label className="settings-field">
          <span className="settings-label">别名</span>
          <input
            className="settings-input"
            value={alias}
            placeholder={`${providerId}/<模型 ID>`}
            onChange={(event) => setAlias(event.target.value)}
          />
        </label>
        <label className="settings-field">
          <span className="settings-label">显示名称</span>
          <input
            className="settings-input"
            value={displayName}
            onChange={(event) => setDisplayName(event.target.value)}
          />
        </label>
        <label className="settings-field">
          <span className="settings-label">上下文窗口</span>
          <input
            className="settings-input"
            value={context}
            inputMode="numeric"
            onChange={(event) => setContext(event.target.value)}
          />
        </label>
        <label className="settings-field">
          <span className="settings-label">最大输出 Token</span>
          <input
            className="settings-input"
            value={maxOutput}
            inputMode="numeric"
            onChange={(event) => setMaxOutput(event.target.value)}
          />
        </label>
        <div className="settings-field">
          <span className="settings-label">输入类型</span>
          <div className="settings-check-group">
            <label className="settings-check"><input type="checkbox" checked={inputTypes.text} onChange={(event) => setInputTypes((prev) => ({ ...prev, text: event.target.checked }))} />文本</label>
            <label className="settings-check"><input type="checkbox" checked={inputTypes.image} onChange={(event) => setInputTypes((prev) => ({ ...prev, image: event.target.checked }))} />图片</label>
            <label className="settings-check"><input type="checkbox" checked={inputTypes.video} onChange={(event) => setInputTypes((prev) => ({ ...prev, video: event.target.checked }))} />视频</label>
          </div>
        </div>
        <div className="settings-field">
          <span className="settings-label">输出类型</span>
          <div className="settings-check-group">
            <label className="settings-check"><input type="checkbox" checked={outputText} onChange={(event) => setOutputText(event.target.checked)} />文本</label>
          </div>
        </div>
        <div className="settings-mcp-form-actions">
          <button type="button" onClick={onCancel}>取消</button>
          <button type="button" className="primary-button" disabled={modelId.trim() === ""} onClick={save}>保存</button>
        </div>
      </div>
    </div>
  );
}

function ModelsSection({
  snapshot,
  onChanged,
  onError,
  onNotice,
}: {
  snapshot: WebUiSettingsSnapshot;
  onChanged: (next: WebUiSettingsSnapshot) => void;
  onError: (message: string) => void;
  onNotice: (message: string) => void;
}) {
  const [selectedProvider, setSelectedProvider] = useState<string | null>(null);
  const [addingProvider, setAddingProvider] = useState(false);
  const [providerDraft, setProviderDraft] = useState({
    id: "",
    display_name: "",
    base_url: "",
    api_key: "",
    api_key_env: "",
    provider_type: "openai",
  });
  const [addingModel, setAddingModel] = useState(false);
  const [busy, setBusy] = useState(false);

  const provider = snapshot.providers.find((entry) => entry.id === selectedProvider);
  const providerModels = snapshot.models.filter((model) => model.provider === selectedProvider);

  const saveProvider = () => {
    if (providerDraft.id.trim() === "" || busy) return;
    setBusy(true);
    const edit: WebUiProviderEdit = {
      id: providerDraft.id.trim(),
      display_name: providerDraft.display_name.trim() || null,
      base_url: providerDraft.base_url.trim() || null,
      provider_type: providerDraft.provider_type,
      api_key: providerDraft.api_key.trim() ? providerDraft.api_key.trim() : null,
      api_key_env: providerDraft.api_key_env.trim() ? providerDraft.api_key_env.trim() : null,
    };
    addProvider(edit)
      .then((next) => {
        onChanged(next);
        setSelectedProvider(edit.id);
        setAddingProvider(false);
        setProviderDraft({ id: "", display_name: "", base_url: "", api_key: "", api_key_env: "", provider_type: "openai" });
        onNotice(`提供方 ${edit.id} 已保存`);
      })
      .catch(() => onError("无法保存提供方（请检查 Base URL / 格式）"))
      .finally(() => setBusy(false));
  };

  const saveModel = (model: WebUiModelEdit) => {
    if (busy) return;
    setBusy(true);
    addModel(model)
      .then((next) => {
        onChanged(next);
        setAddingModel(false);
        onNotice(`模型 ${model.alias} 已保存`);
      })
      .catch(() => onError("无法保存模型（请检查字段）"))
      .finally(() => setBusy(false));
  };

  const deleteProvider = (id: string) => {
    if (busy) return;
    setBusy(true);
    removeProvider(id)
      .then((next) => {
        onChanged(next);
        if (selectedProvider === id) setSelectedProvider(null);
        onNotice(`提供方 ${id} 已删除`);
      })
      .catch(() => onError("无法删除提供方"))
      .finally(() => setBusy(false));
  };

  const deleteModel = (alias: string) => {
    if (busy) return;
    setBusy(true);
    removeModel(alias)
      .then((next) => {
        onChanged(next);
        onNotice(`模型 ${alias} 已删除`);
      })
      .catch(() => onError("无法删除模型"))
      .finally(() => setBusy(false));
  };

  return (
    <section className="settings-group">
      <h3>模型设置</h3>
      <p className="settings-hint">管理自定义模型供应商，配置后可在聊天时选择使用。</p>
      <div className="provider-layout">
        <div className="provider-list">
          <button
            type="button"
            className={`provider-item ${selectedProvider === null && !addingProvider ? "active" : ""}`}
            onClick={() => {
              setSelectedProvider(null);
              setAddingProvider(false);
            }}
          >
            自定义供应商
          </button>
          {snapshot.providers.map((entry) => (
            <button
              type="button"
              key={entry.id}
              className={`provider-item ${selectedProvider === entry.id ? "active" : ""}`}
              onClick={() => {
                setSelectedProvider(entry.id);
                setAddingProvider(false);
              }}
            >
              <span className="provider-item-name">{entry.display_name ?? entry.id}</span>
              <span className="provider-item-key">{entry.has_api_key ? "已配置" : "未配置"}</span>
            </button>
          ))}
          <button
            type="button"
            className="provider-item provider-add"
            onClick={() => {
              setAddingProvider(true);
              setSelectedProvider(null);
            }}
          >
            <Plus size={14} aria-hidden /> 添加供应商
          </button>
        </div>

        <div className="provider-detail">
          {addingProvider ? (
            <div className="settings-card">
              <div className="settings-card-title">添加模型供应商</div>
              <p className="settings-hint">配置一个完全自定义的 API 端点和初始模型。</p>
              <label className="settings-field">
                <span className="settings-label">名称</span>
                <input className="settings-input" value={providerDraft.display_name} placeholder="如：智谱 GLM" onChange={(event) => setProviderDraft((prev) => ({ ...prev, display_name: event.target.value }))} />
              </label>
              <label className="settings-field">
                <span className="settings-label">ID</span>
                <input className="settings-input" value={providerDraft.id} placeholder="zhipu" onChange={(event) => setProviderDraft((prev) => ({ ...prev, id: event.target.value }))} />
              </label>
              <label className="settings-field">
                <span className="settings-label">Base URL</span>
                <input className="settings-input" value={providerDraft.base_url} placeholder="https://api.example.com/v1" onChange={(event) => setProviderDraft((prev) => ({ ...prev, base_url: event.target.value }))} />
              </label>
              <label className="settings-field">
                <span className="settings-label">API Key</span>
                <input className="settings-input" value={providerDraft.api_key} placeholder="输入 API Key" onChange={(event) => setProviderDraft((prev) => ({ ...prev, api_key: event.target.value }))} />
              </label>
              <label className="settings-field">
                <span className="settings-label">API Key 环境变量</span>
                <input className="settings-input" value={providerDraft.api_key_env} placeholder="或输入环境变量名（二选一）" onChange={(event) => setProviderDraft((prev) => ({ ...prev, api_key_env: event.target.value }))} />
              </label>
              <div className="settings-field">
                <span className="settings-label">API 格式</span>
                <select className="settings-input" value={providerDraft.provider_type} onChange={(event) => setProviderDraft((prev) => ({ ...prev, provider_type: event.target.value }))}>
                  {API_TYPE_OPTIONS.map((opt) => (<option key={opt.value} value={opt.value}>{opt.label}</option>))}
                </select>
              </div>
              <div className="settings-mcp-form-actions">
                <button type="button" onClick={() => setAddingProvider(false)}>取消</button>
                <button type="button" className="primary-button" disabled={busy || providerDraft.id.trim() === ""} onClick={saveProvider}>
                  {busy ? "保存中…" : "添加供应商"}
                </button>
              </div>
            </div>
          ) : provider ? (
            <div className="settings-card">
              <div className="settings-card-title">
                <span>{provider.display_name ?? provider.id}</span>
                <button type="button" className="icon-button" aria-label="删除供应商" onClick={() => deleteProvider(provider.id)}><Trash2 size={14} aria-hidden /></button>
              </div>
              <dl className="settings-dl">
                <dt>ID</dt><dd>{provider.id}</dd>
                <dt>类型</dt><dd>{provider.provider_type ?? "—"}</dd>
                <dt>Base URL</dt><dd className="mono">{provider.base_url ?? "—"}</dd>
                <dt>密钥</dt><dd>{provider.has_api_key ? "已配置" : "未配置"}</dd>
              </dl>
              <div className="settings-card-title">
                <span>模型列表</span>
                <button type="button" className="chip-button" onClick={() => setAddingModel(true)}><Plus size={14} aria-hidden />添加模型</button>
              </div>
              {providerModels.length === 0 ? (
                <p className="settings-empty">还没有配置模型，添加模型后可在聊天中使用。</p>
              ) : (
                <ul className="settings-table">
                  <li className="settings-table-head"><span>别名</span><span>上下文</span><span>输出</span><span>操作</span></li>
                  {providerModels.map((model) => (
                    <li key={model.alias} className="settings-table-row">
                      <span className="settings-cell-name" title={model.alias}>{model.display_name ?? model.alias}</span>
                      <span className="settings-cell">{model.context_window ? `${Math.round(model.context_window / 1000)}k` : "—"}</span>
                      <span className="settings-cell">—</span>
                      <span className="settings-cell settings-row-actions">
                        <button type="button" className="icon-button" aria-label="删除模型" onClick={() => deleteModel(model.alias)}><Trash2 size={14} aria-hidden /></button>
                      </span>
                    </li>
                  ))}
                </ul>
              )}
              {addingModel ? <ModelEditorModal providerId={provider.id} onCancel={() => setAddingModel(false)} onSave={saveModel} /> : null}
            </div>
          ) : (
            <div className="settings-card">
              <div className="settings-card-title">模型设置</div>
              <p className="settings-empty">选择左侧供应商，或添加一个新的自定义供应商。</p>
            </div>
          )}
        </div>
      </div>
    </section>
  );
}

// ---------------------------------------------------------------------------
// MCP 服务器
// ---------------------------------------------------------------------------

function McpSection({
  snapshot,
  onChanged,
  onError,
  onNotice,
}: {
  snapshot: WebUiSettingsSnapshot;
  onChanged: (next: WebUiSettingsSnapshot) => void;
  onError: (message: string) => void;
  onNotice: (message: string) => void;
}) {
  const [adding, setAdding] = useState(false);
  const [id, setId] = useState("");
  const [transport, setTransport] = useState<(typeof TRANSPORTS)[number]>("stdio");
  const [command, setCommand] = useState("");
  const [args, setArgs] = useState("");
  const [env, setEnv] = useState("");
  const [url, setUrl] = useState("");
  const [bearer, setBearer] = useState("");
  const [headers, setHeaders] = useState("");
  const [enabled, setEnabled] = useState(true);
  const [busy, setBusy] = useState(false);

  const save = () => {
    if (id.trim() === "" || busy) return;
    if (transport === "stdio" && command.trim() === "") return;
    if (transport !== "stdio" && url.trim() === "") return;
    const parseMap = (text: string): Record<string, string> => {
      const out: Record<string, string> = {};
      for (const line of text.split("\n")) {
        const trimmed = line.trim();
        if (!trimmed) continue;
        const idx = trimmed.indexOf("=");
        if (idx <= 0) continue;
        out[trimmed.slice(0, idx).trim()] = trimmed.slice(idx + 1).trim();
      }
      return out;
    };
    const edit: WebUiMcpServerEdit = {
      id: id.trim(),
      transport,
      enabled,
      command: transport === "stdio" ? command.trim() : null,
      url: transport !== "stdio" ? url.trim() : null,
      args: transport === "stdio"
        ? args.split("\n").map((line) => line.trim()).filter((line) => line !== "")
        : [],
      env: transport === "stdio" ? parseMap(env) : {},
      headers: transport !== "stdio"
        ? { ...(bearer.trim() ? { Authorization: `Bearer ${bearer.trim()}` } : {}), ...parseMap(headers) }
        : {},
    };
    setBusy(true);
    upsertMcpServer(edit)
      .then((next) => {
        onChanged(next);
        setAdding(false);
        setId(""); setCommand(""); setArgs(""); setEnv(""); setUrl(""); setBearer(""); setHeaders("");
        onNotice(`MCP 服务器 ${id.trim()} 已保存`);
      })
      .catch(() => onError("无法保存 MCP 服务器（请检查命令或地址）"))
      .finally(() => setBusy(false));
  };

  const toggle = (server: WebUiMcpServerInfo, value: boolean) => {
    setMcpServerEnabled(server.id, value)
      .then(onChanged)
      .catch(() => onError("无法更新 MCP 服务器状态"));
  };

  const remove = (server: WebUiMcpServerInfo) => {
    removeMcpServer(server.id)
      .then((next) => {
        onChanged(next);
        onNotice(`MCP 服务器 ${server.id} 已删除`);
      })
      .catch(() => onError("无法删除 MCP 服务器"));
  };

  return (
    <section className="settings-group">
      <h3>MCP 服务器</h3>
      {snapshot.mcp_servers.length === 0 ? (
        <p className="settings-empty">未配置 MCP 服务器。</p>
      ) : (
        <ul className="settings-table">
          <li className="settings-table-head"><span>ID</span><span>传输</span><span>命令 / 地址</span><span>工具</span><span>操作</span></li>
          {snapshot.mcp_servers.map((server) => {
            const target = server.transport === "stdio" ? (server.command ?? "—") : (server.url ?? "—");
            const keys = server.transport === "stdio" ? server.env_keys : server.header_keys;
            return (
              <li key={server.id} className="settings-table-row">
                <span className="settings-cell-name" title={server.id}>{server.id}</span>
                <span className="settings-cell">{server.transport}</span>
                <span className="settings-cell settings-cell-mono" title={target}>{target}</span>
                <span className="settings-cell">{server.tool_count > 0 ? `${server.tool_count} 个工具` : "—"}</span>
                <span className="settings-cell settings-row-actions">
                  <Toggle checked={server.enabled} label={server.enabled ? "已启用" : "已禁用"} onChange={(value) => toggle(server, value)} />
                  {keys && keys.length > 0 ? <span className="settings-cell-mono settings-keys" title={keys.join(", ")}>{keys.length} 键</span> : null}
                  <button type="button" className="icon-button" aria-label="删除" onClick={() => remove(server)}><Trash2 size={14} aria-hidden /></button>
                </span>
              </li>
            );
          })}
        </ul>
      )}

      {adding ? (
        <div className="settings-card">
          <div className="settings-card-title">添加 MCP 服务器</div>
          <div className="settings-field">
            <span className="settings-label">ID</span>
            <input className="settings-input" value={id} placeholder="my-server" onChange={(event) => setId(event.target.value)} />
          </div>
          <div className="settings-field">
            <span className="settings-label">传输</span>
            <select className="settings-input" value={transport} onChange={(event) => setTransport(event.target.value as (typeof TRANSPORTS)[number])}>
              {TRANSPORTS.map((t) => (<option key={t} value={t}>{t}</option>))}
            </select>
          </div>
          {transport === "stdio" ? (
            <>
              <label className="settings-field"><span className="settings-label">命令</span><input className="settings-input" value={command} placeholder="npx" onChange={(event) => setCommand(event.target.value)} /></label>
              <label className="settings-field"><span className="settings-label">参数（每行一个）</span><textarea className="settings-input" value={args} placeholder={"-y\n@modelcontextprotocol/server-filesystem"} onChange={(event) => setArgs(event.target.value)} /></label>
              <label className="settings-field"><span className="settings-label">环境变量（KEY=value 每行）</span><textarea className="settings-input" value={env} placeholder={"API_KEY=xxx"} onChange={(event) => setEnv(event.target.value)} /></label>
            </>
          ) : (
            <>
              <label className="settings-field"><span className="settings-label">地址</span><input className="settings-input" value={url} placeholder="http://127.0.0.1:8000/mcp" onChange={(event) => setUrl(event.target.value)} /></label>
              <label className="settings-field"><span className="settings-label">Bearer Token</span><input type="password" className="settings-input" value={bearer} placeholder="可选" onChange={(event) => setBearer(event.target.value)} /></label>
              <label className="settings-field"><span className="settings-label">Headers（KEY=value 每行）</span><textarea className="settings-input" value={headers} placeholder={"X-Custom=abc"} onChange={(event) => setHeaders(event.target.value)} /></label>
            </>
          )}
          <div className="settings-field settings-field-inline">
            <span className="settings-label">启用</span>
            <Toggle checked={enabled} label={enabled ? "已启用" : "已禁用"} onChange={setEnabled} />
          </div>
          <div className="settings-mcp-form-actions">
            <button type="button" onClick={() => setAdding(false)}>取消</button>
            <button type="button" className="primary-button" disabled={busy || id.trim() === "" || (transport === "stdio" ? command.trim() === "" : url.trim() === "")} onClick={save}>
              {busy ? "保存中…" : "保存"}
            </button>
          </div>
        </div>
      ) : (
        <button type="button" className="chip-button" onClick={() => setAdding(true)}><Plus size={14} aria-hidden />添加服务器</button>
      )}
    </section>
  );
}

// ---------------------------------------------------------------------------
// 技能
// ---------------------------------------------------------------------------

function SkillsSection({
  snapshot,
}: {
  snapshot: WebUiSettingsSnapshot;
}) {
  return (
    <section className="settings-group">
      <h3>技能</h3>
      {snapshot.skills.length === 0 ? (
        <p className="settings-empty">未发现技能。</p>
      ) : (
        <ul className="settings-table skills-table">
          {snapshot.skills.map((skill) => (
            <li key={skill.name} className="skills-row">
              <span className="skills-name" title={skill.name}>{skill.display_name ?? skill.name}</span>
              <span className="skills-desc">{skill.description ?? "—"}</span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

// ---------------------------------------------------------------------------
// 页面骨架
// ---------------------------------------------------------------------------

export function SettingsPage() {
  const actions = useAppActions();
  const [snapshot, setSnapshot] = useState<WebUiSettingsSnapshot | null>(null);
  const [section, setSection] = useState<NavItem>("general");
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    fetchSettings()
      .then(setSnapshot)
      .catch(() => setError("无法加载设置。请确认 neo webui 服务仍在运行。"));
  }, []);

  const showNotice = (text: string) => {
    setNotice(text);
    window.setTimeout(() => setNotice(null), 4000);
  };

  const onAppearanceSaved = (appearance: WebUiAppearance) => {
    actions.applyAppearance(appearance);
    if (snapshot) setSnapshot({ ...snapshot, appearance });
  };

  return (
    <div className="settings-page">
      <div className="settings-page-header">
        <button type="button" className="settings-back" onClick={() => actions.setView("chat")}>
          <ArrowLeft size={15} aria-hidden /> 返回工作区
        </button>
        <h2>设置</h2>
      </div>
      <div className="settings-page-body">
        <nav className="settings-nav" aria-label="设置">
          {NAV_GROUPS.map((group) => (
            <div className="settings-nav-group" key={group.label}>
              <div className="settings-nav-label">{group.label}</div>
              {group.items.map((item) => (
                <button
                  type="button"
                  key={item.id}
                  className={`settings-nav-item ${section === item.id ? "active" : ""}`}
                  onClick={() => setSection(item.id)}
                >
                  {item.label}
                </button>
              ))}
            </div>
          ))}
        </nav>
        <div className="settings-content">
          {error !== null ? (
            <div className="settings-error" role="alert">
              {error}
              <button type="button" className="chip-button" onClick={() => setError(null)}>知道了</button>
            </div>
          ) : null}
          {notice !== null ? <div className="settings-notice" role="status">{notice}</div> : null}
          {snapshot === null ? (
            <div className="settings-loading">加载中…</div>
          ) : section === "general" ? (
            <GeneralSection snapshot={snapshot} onChanged={setSnapshot} onError={setError} onNotice={showNotice} />
          ) : section === "appearance" ? (
            <AppearanceSection appearance={snapshot.appearance} onSaved={onAppearanceSaved} onError={setError} onNotice={showNotice} />
          ) : section === "models" ? (
            <ModelsSection snapshot={snapshot} onChanged={setSnapshot} onError={setError} onNotice={showNotice} />
          ) : section === "mcp" ? (
            <McpSection snapshot={snapshot} onChanged={setSnapshot} onError={setError} onNotice={showNotice} />
          ) : (
            <SkillsSection snapshot={snapshot} />
          )}
        </div>
      </div>
      <div className="settings-footer">
        <Pencil size={12} aria-hidden />
        配置写入 ~/.neo/config.toml（外观已持久化到 [webui]）
      </div>
    </div>
  );
}
