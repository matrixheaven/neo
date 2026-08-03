# 服务商与模型配置

Neo 通过 `config.toml` 中的 `[providers.<id>]` 表声明任意数量的 LLM 后端（服务商），再用 `[models.<alias>]` 把模型挂到服务商上。服务商（provider）的协议由 `type` 字段决定，Neo 会据此选择对应的协议客户端。

## 支持的服务商类型

| `type` 值 | 协议 | 适用场景 |
| --- | --- | --- |
| `openai` | OpenAI Chat Completions（`/chat/completions`） | OpenAI 官方、OpenRouter、Ollama、vLLM、DeepSeek 等任何 OpenAI 兼容端点 |
| `openai_response` | OpenAI Responses API（`/responses`） | OpenAI 官方 Responses API（支持原生 reasoning、工具调用等） |
| `anthropic` | Anthropic Messages API | Claude 系列模型 |
| `google` | Google Generative AI | Gemini 系列模型 |

> 旧版的 `openai-chat` / `openai-compatible` / `openai-responses` 类型已移除。Chat Completions 兼容端点用 `openai`；OpenAI 官方 Responses API 用 `openai_response`。

## 各服务商的 TOML 片段

### OpenAI Responses

```toml
[providers.openai]
type = "openai_response"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
```

### OpenAI Chat Completions

```toml
[providers.openai-chat]
type = "openai"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
```

### Anthropic

```toml
[providers.anthropic]
type = "anthropic"
base_url = "https://api.anthropic.com/v1"
api_key_env = "ANTHROPIC_API_KEY"
```

### Google Gemini

```toml
[providers.google]
type = "google"
base_url = "https://generativelanguage.googleapis.com/v1beta"
api_key_env = "GEMINI_API_KEY"
```

## 环境变量优先级

服务商的 API key 按以下顺序解析，命中即返回：

1. **`api_key`** —— 配置文件内联的 key 字符串；
2. **`api_key_env`** —— 读取该环境变量（推荐，避免明文写入配置）；
3. 两者都未设置 → 调用 API 时返回未授权错误。

同时存在时，**内联值优先**：只有当 `api_key` 未设置时，才会回退到 `api_key_env`。

```toml
# 推荐写法：通过环境变量注入
[providers.openai]
type = "openai_response"
api_key_env = "OPENAI_API_KEY"

# 或直接内联（注意保密）
[providers.openrouter]
type = "openai"
base_url = "https://openrouter.ai/api/v1"
api_key = "sk-or-v1-xxxxxxxxxxxx"
```

> 凭证按服务商分别配置：每个服务商通过自身的 `api_key` 或 `api_key_env` 提供凭证。

## 自定义服务商

任何 OpenAI 兼容端点都能用 `type = "openai"` 接入，只需把 `base_url` 指向你的服务。

### Ollama（本地）

```toml
[providers."local-ollama"]
type = "openai"
base_url = "http://localhost:11434/v1"
api_key = "ollama"   # Ollama 不校验 key，任意字符串即可
```

### OpenRouter

```toml
[providers.openrouter]
type = "openai"
base_url = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"

[models."openrouter/deepseek-r1"]
provider = "openrouter"
model = "deepseek/deepseek-r1"
max_context_tokens = 128000
capabilities = ["streaming", "tools", "reasoning"]
```

### DeepSeek / vLLM / 其他兼容端点

```toml
[providers.deepseek]
type = "openai"
base_url = "https://api.deepseek.com/v1"
api_key_env = "DEEPSEEK_API_KEY"

[models."deepseek-chat"]
provider = "deepseek"
model = "deepseek-chat"
max_context_tokens = 64000
capabilities = ["streaming", "tools"]
```

## 模型能力声明

每个 model 条目通过 `capabilities` 字段声明其支持的能力标签：

| 标签 | 含义 |
| --- | --- |
| `streaming` | 支持流式输出 |
| `tools` | 支持 tool / function calling |
| `images` | 支持图片输入（视觉模型） |
| `reasoning` | 支持 reasoning / thinking 内容 |

```toml
[models."anthropic/claude-sonnet-4-5"]
provider = "anthropic"
model = "claude-sonnet-4-5-20250514"
max_context_tokens = 200000
capabilities = ["streaming", "tools", "images", "reasoning"]
display_name = "Claude Sonnet 4.5"
```

能力标签用于 UI 提示与能力路由（如 reasoning effort 仅对声明了 `reasoning` 的模型生效）。缺省时 Neo 按模型默认能力推断。

### 自定义 reasoning effort

服务商可以定义 Neo 常用预设以外的 effort 值：

```toml
[runtime]
reasoning = { mode = "effort", effort = "UltraMax" }
```

Effort 值由服务商定义，区分大小写。具有原生 effort 字段的服务商按原文接收该值；基于 budget 或 toggle 的适配器会拒绝无法映射的值。空字符串或纯空白值无效。支持哪些值请查阅服务商的模型文档。

## 下一步

- [配置文件总览](config-files.md) — `config.toml` 全字段表
- [权限模式](permissions.md) — Ask / Auto / YOLO 模式说明
- `examples/config/providers-models.toml` — 完整可复制的 provider/model 配置示例
