# 数据存储位置

Neo 把所有持久化数据集中放在 `~/.neo/`（或 `$NEO_HOME`）下。会话、配置、技能、prompt、主题、审批规则都按约定布局存放，便于备份、迁移和清理。

## Neo Home 目录

| 变量 | 路径 | 说明 |
| --- | --- | --- |
| `NEO_HOME`（环境变量） | 用户自定义 | 设置后优先使用该目录作为 neo home |
| 默认 | `~/.neo/` | 未设置 `NEO_HOME` 时使用 |

所有相对 `~/.neo/` 的路径在文档中均可用 `$NEO_HOME` 替换。

## `~/.neo/` 目录结构

```
~/.neo/
├── config.toml              # 主配置文件（单一来源）
├── SYSTEM.md                # 可选：替换 Neo 内置系统 prompt
├── APPEND_SYSTEM.md         # 可选：追加在基础系统 prompt 后的指令
├── AGENTS.md                # 用户全局指令（始终受信）
├── approval_rules.json      # 持久化前缀审批规则（Layer 2）
├── trust.json               # 项目信任决策记录
├── sessions/                # 会话根目录
│   └── wd_<slug>_<hash12>/  # 每个 workspace 一个桶
│       └── session_<uuid>/  # 每次会话一个目录
│           ├── state.json   # 会话状态（模型、时间戳等）
│           ├── workflows/   # 持久化 workflow run
│           │   └── <run_id>/
│           │       ├── run.json
│           │       └── journal.jsonl
│           └── agents/
│               └── main/    # 主 agent 记录
│                   ├── wire.jsonl
│                   ├── plans/
│                   ├── goals/
│                   └── tasks/
├── prompts/                 # 全局 prompt 模板
├── skills/                  # 内置 + 用户技能
├── themes/                  # 主题 JSON 文件（如 magenta-dark.json）
└── ...
```

## 会话存储路径

会话按 workspace 分桶、按会话 id 分目录，结构为：

```
<sessions_dir>/wd_<slug>_<hash12>/session_<uuid>/agents/<agent_id>/wire.jsonl
```

| 段 | 生成规则 | 示例 |
| --- | --- | --- |
| `sessions_dir` | `config.toml` 的 `sessions_dir`，默认 `~/.neo/sessions` | `~/.neo/sessions` |
| `wd_<slug>_<hash12>` | `wd_` + workspace 目录 basename 的 slug + `_` + 绝对路径 SHA-256 的前 12 个十六进制字符 | `wd_neo_a1b2c3d4e5f6` |
| `session_<uuid>` | 每次会话生成的新 UUID | `session_4f7c...` |
| `agents/<agent_id>` | agent id，主 agent 固定为 `main`；Delegate 子 agent 用各自 id | `agents/main` |
| `wire.jsonl` | 该 agent 的事件流（JSON Lines） | — |

> slug 规则：basename 转小写、非 `[a-z0-9._-]` 替换为 `-`、去掉首尾 `-`、截断到 40 字符；空时回落为 `workspace`。哈希保证同名不同路径的 workspace 拥有独立的桶。

每个会话目录内的固定文件：

| 文件 / 目录 | 说明 |
| --- | --- |
| `state.json` | 会话元数据（schema 版本、创建时间等） |
| `agents/main/wire.jsonl` | 主 agent 的完整事件流（`neo.session.jsonl` 格式，schema v1），包含持久化指令 epoch |
| `agents/main/plans/` | 主 agent 的计划文件 |
| `agents/main/goals/` | 主 agent 的目标文件 |
| `agents/main/tasks/` | 主 agent 的后台任务产物 |
| `workflows/<run_id>/run.json` | 不可变 launch metadata：workflow identity、已审查 source/args、phases、launch source 与 journal format version |
| `workflows/<run_id>/journal.jsonl` | Append-only workflow 状态、invocation intent/result、控制与实际用量记录 |
| `agents/<agent_id>/...` | 子 agent（如 Delegate 产生的）对应记录 |

Workflow 文件位于 session 目录下，不属于 transcript 或 background-task 投影。`run.json` 是不可变 launch metadata。当前状态、控制与 provider 实际用量只来自 append-only `journal.jsonl`，后者支持 `TaskOutput`、pause/resume/stop 和 host-exit recovery。历史 session 仍可读取；没有 `workflows/<run_id>/` 文件的旧 workflow 卡片只是历史投影，不能恢复执行。

## 其他配置文件位置

| 路径 | 来源 | 说明 |
| --- | --- | --- |
| `~/.neo/config.toml` | 主配置 | 见 [配置文件](config-files.md) |
| `~/.neo/SYSTEM.md` | 系统 prompt | 可选；替换 Neo 内置基础系统 prompt |
| `~/.neo/APPEND_SYSTEM.md` | 系统 prompt 追加 | 可选；追加在基础系统 prompt 后 |
| `~/.neo/AGENTS.md` | 用户全局指令 | 始终受信；作为会话指令 epoch 加载（不改写系统 prompt），见 [AGENTS.md](../customization/agents.md#agentsmd) |
| `~/.neo/approval_rules.json` | 前缀审批规则 | 见 [权限模式](permissions.md#前缀级layer-2) |
| `~/.neo/trust.json` | 项目信任 | 记录每个 workspace 是否被用户信任（当存在 `AGENTS.md` 等输入时触发）；门控项目 `AGENTS.md` 指令加载 |
| `~/.neo/prompts/` | 全局 prompt 模板 | `global_prompts_dir()` 返回的目录 |
| `~/.neo/skills/` | 技能目录 | 加上 `config.toml` 中 `skill_path` / `extra_skill_dirs` 声明的额外目录 |
| `~/.neo/themes/*.json` | 主题 | 如 `magenta-dark.json`，TUI 启动时加载 |

`sessions_dir` 支持自定义位置（接受 `~` 展开），便于把会话放到外置磁盘或 tmpfs：

```toml
# config.toml
sessions_dir = "~/neo-sessions"
```

## 清理指南

### 删除某个 workspace 的全部会话

```
rm -rf ~/.neo/sessions/wd_<slug>_<hash12>/
```

可用 `ls ~/.neo/sessions/` 查看所有 workspace 桶，根据 slug 找到对应项目。

### 清空所有会话

```shell
rm -rf ~/.neo/sessions/
```

Neo 会在下次启动时按需重建。

### 重置审批规则

```shell
rm ~/.neo/approval_rules.json
```

删除后所有前缀规则失效，Ask 模式会重新逐项询问。

### 完整重置

```shell
mv ~/.neo ~/.neo.bak    # 备份
# 或
rm -rf ~/.neo           # 彻底清除
```

> `trust.json` 也保存在 neo home 下；删除后所有「已信任项目」决策会丢失，下次启动需要重新确认。

## 下一步

- [配置文件总览](config-files.md) — `sessions_dir` 等字段定义
- [权限模式](permissions.md) — `approval_rules.json` 的语义
- [Provider 配置](providers.md) — API key 与端点配置
