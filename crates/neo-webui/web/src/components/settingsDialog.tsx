/**
 * Settings dialog: 基础设置 (general, appearance, models, providers) and
 * Agent 能力 (MCP servers, skills). Reads a read-only snapshot from the
 * backend; model default and MCP changes are persisted to config.toml (they
 * take effect for the running host on the next service start — per-turn
 * selection in the composer stays live). Credentials never leave the server;
 * the snapshot only says whether a provider has a key configured.
 */

import { useEffect, useState } from "react";
import { Check, Pencil, Plus, Trash2, X } from "lucide-react";
import {
  fetchSettings,
  removeMcpServer,
  setDefaultModel,
  setMcpServerEnabled,
  upsertMcpServer,
} from "../api";
import type {
  WebUiMcpServerEdit,
  WebUiMcpServerInfo,
  WebUiSettingsSnapshot,
} from "../protocol";
import { useAppActions, useAppState } from "../state/store";

type Section = "basic" | "agent";

const TRANSPORTS = ["stdio", "http", "sse"] as const;

function capabilityText(capabilities: string[] | undefined): string {
  if (!capabilities || capabilities.length === 0) return "—";
  return capabilities.join(", ");
}

function McpServerRow({
  server,
  onChanged,
  onError,
}: {
  server: WebUiMcpServerInfo;
  onChanged: (snapshot: WebUiSettingsSnapshot) => void;
  onError: (message: string) => void;
}) {
  const [busy, setBusy] = useState(false);
  const target =
    server.transport === "stdio"
      ? (server.command ?? "—")
      : (server.url ?? "—");
  return (
    <li className="settings-table-row">
      <span className="settings-cell-name" title={server.id}>
        {server.id}
      </span>
      <span className="settings-cell">{server.transport}</span>
      <span className="settings-cell settings-cell-mono" title={target}>
        {target}
      </span>
      <span className="settings-cell">{server.tool_count > 0 ? `${server.tool_count} 个工具` : "—"}</span>
      <span className="settings-cell settings-row-actions">
        <button
          type="button"
          role="switch"
          aria-checked={server.enabled}
          aria-label={`${server.enabled ? "禁用" : "启用"} MCP 服务器 ${server.id}`}
          disabled={busy}
          className={`settings-switch ${server.enabled ? "on" : ""}`}
          onClick={() => {
            setBusy(true);
            setMcpServerEnabled(server.id, !server.enabled)
              .then(onChanged)
              .catch(() => onError("无法更新 MCP 服务器状态"))
              .finally(() => setBusy(false));
          }}
        >
          {server.enabled ? "已启用" : "已禁用"}
        </button>
        <button
          type="button"
          className="icon-button"
          aria-label={`删除 MCP 服务器 ${server.id}`}
          title="删除"
          disabled={busy}
          onClick={() => {
            setBusy(true);
            removeMcpServer(server.id)
              .then(onChanged)
              .catch(() => onError("无法删除 MCP 服务器"))
              .finally(() => setBusy(false));
          }}
        >
          <Trash2 size={14} aria-hidden />
        </button>
      </span>
    </li>
  );
}

export function SettingsDialog({ onClose }: { onClose: () => void }) {
  const state = useAppState();
  const actions = useAppActions();
  const [section, setSection] = useState<Section>("basic");
  const [snapshot, setSnapshot] = useState<WebUiSettingsSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [savingModel, setSavingModel] = useState(false);
  const [addingMcp, setAddingMcp] = useState(false);
  const [addId, setAddId] = useState("");
  const [addTransport, setAddTransport] = useState<(typeof TRANSPORTS)[number]>("stdio");
  const [addCommand, setAddCommand] = useState("");
  const [addUrl, setAddUrl] = useState("");
  const [addArgs, setAddArgs] = useState("");
  const [addEnabled, setAddEnabled] = useState(true);
  const [savingMcp, setSavingMcp] = useState(false);

  useEffect(() => {
    fetchSettings()
      .then(setSnapshot)
      .catch(() => setError("无法加载设置。请确认 neo webui 服务仍在运行。"));
  }, []);

  const showNotice = (text: string) => {
    setNotice(text);
    window.setTimeout(() => setNotice(null), 4000);
  };

  const defaultModel = snapshot?.default_model ?? "";
  const providers = snapshot?.providers ?? [];
  const models = snapshot?.models ?? [];
  const mcpServers = snapshot?.mcp_servers ?? [];
  const skills = snapshot?.skills ?? [];

  const submitDefaultModel = (alias: string) => {
    if (alias === defaultModel || savingModel) return;
    setSavingModel(true);
    setDefaultModel(alias)
      .then((next) => {
        setSnapshot(next);
        showNotice(`默认模型已保存为 ${alias}（重启服务后对新会话生效）`);
      })
      .catch(() => setError("无法保存默认模型"))
      .finally(() => setSavingModel(false));
  };

  const submitAddMcp = () => {
    const id = addId.trim();
    if (id === "" || savingMcp) return;
    if (addTransport === "stdio" && addCommand.trim() === "") return;
    if (addTransport !== "stdio" && addUrl.trim() === "") return;
    const edit: WebUiMcpServerEdit = {
      id,
      transport: addTransport,
      enabled: addEnabled,
      command: addTransport === "stdio" ? addCommand.trim() : null,
      url: addTransport !== "stdio" ? addUrl.trim() : null,
      args: addArgs
        .split(/\s+/)
        .map((part) => part.trim())
        .filter((part) => part !== ""),
    };
    setSavingMcp(true);
    upsertMcpServer(edit)
      .then((next) => {
        setSnapshot(next);
        setAddingMcp(false);
        setAddId("");
        setAddCommand("");
        setAddUrl("");
        setAddArgs("");
        showNotice(`MCP 服务器 ${id} 已保存`);
      })
      .catch(() => setError("无法保存 MCP 服务器（请检查命令或地址）"))
      .finally(() => setSavingMcp(false));
  };

  return (
    <div className="dialog-backdrop" role="presentation">
      <div className="settings-dialog" role="dialog" aria-modal="true" aria-label="设置">
        <div className="settings-header">
          <h2>设置</h2>
          <button type="button" className="icon-button" aria-label="关闭设置" onClick={onClose}>
            <X size={16} aria-hidden />
          </button>
        </div>
        <div className="settings-tabs" role="tablist">
          <button
            type="button"
            role="tab"
            aria-selected={section === "basic"}
            className={`settings-tab ${section === "basic" ? "active" : ""}`}
            onClick={() => setSection("basic")}
          >
            基础设置
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={section === "agent"}
            className={`settings-tab ${section === "agent" ? "active" : ""}`}
            onClick={() => setSection("agent")}
          >
            Agent 能力
          </button>
        </div>
        {error !== null ? (
          <div className="settings-error" role="alert">
            {error}
            <button type="button" className="chip-button" onClick={() => setError(null)}>
              知道了
            </button>
          </div>
        ) : null}
        {notice !== null ? <div className="settings-notice" role="status">{notice}</div> : null}

        {section === "basic" ? (
          <div className="settings-body">
            <section className="settings-group">
              <h3>常规</h3>
              <div className="settings-field">
                <label htmlFor="settings-default-model">默认模型</label>
                <select
                  id="settings-default-model"
                  value={defaultModel}
                  disabled={savingModel}
                  onChange={(event) => submitDefaultModel(event.target.value)}
                >
                  {models.length === 0 ? <option value={defaultModel}>{defaultModel}</option> : null}
                  {models.map((model) => (
                    <option key={model.alias} value={model.alias}>
                      {model.alias}
                      {model.alias === defaultModel ? "（当前默认）" : ""}
                    </option>
                  ))}
                </select>
                <p className="settings-hint">
                  写入 ~/.neo/config.toml；重启 neo webui 后生效。会话内仍可通过模型菜单临时切换。
                </p>
              </div>
              <div className="settings-field">
                <span className="settings-static-label">默认提供方</span>
                <span className="settings-static">{snapshot?.default_provider ?? "—"}</span>
              </div>
              <div className="settings-field">
                <span className="settings-static-label">权限模式</span>
                <span className="settings-static">{snapshot?.permission_mode ?? "—"}</span>
              </div>
            </section>

            <section className="settings-group">
              <h3>外观</h3>
              <div className="settings-field">
                <span className="settings-static-label">主题</span>
                <div className="settings-segmented" role="group" aria-label="主题">
                  <button
                    type="button"
                    className={state.theme === "light" ? "active" : ""}
                    onClick={() => actions.setTheme("light")}
                  >
                    浅色
                  </button>
                  <button
                    type="button"
                    className={state.theme === "dark" ? "active" : ""}
                    onClick={() => actions.setTheme("dark")}
                  >
                    深色
                  </button>
                </div>
                <p className="settings-hint">外观偏好保存在当前浏览器（每次启动生成新端口，故浏览器外部不持久）。</p>
              </div>
            </section>

            <section className="settings-group">
              <h3>模型设置</h3>
              {models.length === 0 ? (
                <p className="settings-empty">未配置模型。</p>
              ) : (
                <ul className="settings-table">
                  <li className="settings-table-head">
                    <span>别名</span>
                    <span>提供方</span>
                    <span>上下文</span>
                    <span>能力</span>
                    <span>操作</span>
                  </li>
                  {models.map((model) => (
                    <li key={model.alias} className="settings-table-row">
                      <span className="settings-cell-name" title={model.alias}>
                        {model.alias}
                        {model.alias === defaultModel ? (
                          <Check size={13} aria-label="默认模型" className="settings-default-mark" />
                        ) : null}
                      </span>
                      <span className="settings-cell">{model.provider}</span>
                      <span className="settings-cell">
                        {model.context_window ? `${Math.round(model.context_window / 1000)}k` : "—"}
                      </span>
                      <span className="settings-cell">{capabilityText(model.capabilities)}</span>
                      <span className="settings-cell settings-row-actions">
                        {model.alias === defaultModel ? (
                          <span className="settings-static">默认</span>
                        ) : (
                          <button
                            type="button"
                            className="chip-button"
                            disabled={savingModel}
                            onClick={() => submitDefaultModel(model.alias)}
                          >
                            设为默认
                          </button>
                        )}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
            </section>

            <section className="settings-group">
              <h3>提供方</h3>
              {providers.length === 0 ? (
                <p className="settings-empty">未配置提供方。</p>
              ) : (
                <ul className="settings-table">
                  <li className="settings-table-head">
                    <span>ID</span>
                    <span>类型</span>
                    <span>Base URL</span>
                    <span>密钥</span>
                  </li>
                  {providers.map((provider) => (
                    <li key={provider.id} className="settings-table-row">
                      <span className="settings-cell-name" title={provider.id}>
                        {provider.id}
                      </span>
                      <span className="settings-cell">{provider.provider_type ?? "—"}</span>
                      <span className="settings-cell settings-cell-mono" title={provider.base_url ?? ""}>
                        {provider.base_url ?? "—"}
                      </span>
                      <span className="settings-cell">
                        {provider.has_api_key ? "已配置" : "未配置"}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
            </section>
          </div>
        ) : (
          <div className="settings-body">
            <section className="settings-group">
              <h3>MCP 服务器</h3>
              {mcpServers.length === 0 ? (
                <p className="settings-empty">未配置 MCP 服务器。</p>
              ) : (
                <ul className="settings-table">
                  <li className="settings-table-head">
                    <span>ID</span>
                    <span>传输</span>
                    <span>命令 / 地址</span>
                    <span>工具</span>
                    <span>操作</span>
                  </li>
                  {mcpServers.map((server) => (
                    <McpServerRow
                      key={server.id}
                      server={server}
                      onChanged={(next) => {
                        setSnapshot(next);
                        showNotice(`MCP 服务器 ${server.id} 已更新`);
                      }}
                      onError={setError}
                    />
                  ))}
                </ul>
              )}
              <div className="settings-mcp-add">
                {addingMcp ? (
                  <form
                    className="settings-mcp-form"
                    onSubmit={(event) => {
                      event.preventDefault();
                      submitAddMcp();
                    }}
                  >
                    <h4>添加 MCP 服务器</h4>
                    <label>
                      ID
                      <input
                        value={addId}
                        autoFocus
                        placeholder="my-server"
                        onChange={(event) => setAddId(event.target.value)}
                      />
                    </label>
                    <label>
                      传输
                      <select
                        value={addTransport}
                        onChange={(event) =>
                          setAddTransport(event.target.value as (typeof TRANSPORTS)[number])
                        }
                      >
                        {TRANSPORTS.map((transport) => (
                          <option key={transport} value={transport}>
                            {transport}
                          </option>
                        ))}
                      </select>
                    </label>
                    {addTransport === "stdio" ? (
                      <label>
                        命令
                        <input
                          value={addCommand}
                          placeholder="npx"
                          onChange={(event) => setAddCommand(event.target.value)}
                        />
                      </label>
                    ) : (
                      <label>
                        地址
                        <input
                          value={addUrl}
                          placeholder="http://127.0.0.1:8000/mcp"
                          onChange={(event) => setAddUrl(event.target.value)}
                        />
                      </label>
                    )}
                    <label>
                      参数（空格分隔）
                      <input
                        value={addArgs}
                        placeholder="-y @modelcontextprotocol/server-filesystem"
                        onChange={(event) => setAddArgs(event.target.value)}
                      />
                    </label>
                    <label className="settings-checkbox">
                      <input
                        type="checkbox"
                        checked={addEnabled}
                        onChange={(event) => setAddEnabled(event.target.checked)}
                      />
                      启用
                    </label>
                    <div className="settings-mcp-form-actions">
                      <button type="button" onClick={() => setAddingMcp(false)}>
                        取消
                      </button>
                      <button
                        type="submit"
                        className="primary-button"
                        disabled={
                          savingMcp ||
                          addId.trim() === "" ||
                          (addTransport === "stdio" ? addCommand.trim() === "" : addUrl.trim() === "")
                        }
                      >
                        {savingMcp ? "保存中…" : "保存"}
                      </button>
                    </div>
                  </form>
                ) : (
                  <button type="button" className="chip-button" onClick={() => setAddingMcp(true)}>
                    <Plus size={14} aria-hidden />
                    添加服务器
                  </button>
                )}
              </div>
            </section>

            <section className="settings-group">
              <h3>技能</h3>
              {skills.length === 0 ? (
                <p className="settings-empty">未发现技能。</p>
              ) : (
                <ul className="settings-table">
                  <li className="settings-table-head">
                    <span>名称</span>
                    <span>描述</span>
                  </li>
                  {skills.map((skill) => (
                    <li key={skill.name} className="settings-table-row">
                      <span className="settings-cell-name" title={skill.name}>
                        {skill.display_name ?? skill.name}
                      </span>
                      <span className="settings-cell settings-cell-fill">
                        {skill.description ?? "—"}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
            </section>
          </div>
        )}

        <div className="settings-footer">
          <span aria-hidden>
            <Pencil size={12} />
          </span>
          配置写入 ~/.neo/config.toml（外观除外）
        </div>
      </div>
    </div>
  );
}
