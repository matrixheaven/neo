# 常见问题

本页汇总使用 Neo 时的常见问题，每条按「症状 → 原因 → 解决」组织。排障所需的详细说明，请以文末「相关页面」中的文档为准。

### 启动后提示没有可用的模型服务商

**症状**：启动后提示没有可用的模型服务商，无法发起对话。

**原因**：尚未配置模型服务商，或没有设置默认模型。

**解决**：

1. 检查 `~/.neo/config.toml`（或 `$NEO_HOME/config.toml`）中是否配置了 `providers` 与 `default_model`；
2. 没有配置时，用 `neo provider add` 添加服务商；
3. 配置方法参考 [快速开始](../quickstart.md) 与 [Provider 配置](../configuration/providers.md)。

### 请求返回 401 / 未授权

**症状**：请求返回 `401`，或提示未授权。

**原因**：API key 缺失或无效。

**解决**：

1. 检查服务商的 API key 是否配置正确；
2. 注意 `api_key`（内联）优先于 `api_key_env`（环境变量）：只有 `api_key` 未设置时，才会读取环境变量。

### 找不到之前的会话

**症状**：之前聊过的会话不见了，会话列表里看不到。

**原因**：会话按工作区隔离，存放在 `wd_<slug>_<hash12>` 桶中；切换目录后看到的是另一个会话池。

**解决**：

1. 在 TUI 中用 `/resume` 或 `Ctrl+R` 打开会话选择器；
2. 命令行用 `neo resume` 或 `neo -c` 恢复会话；
3. 完整说明见 [会话管理](sessions.md)。

### Ctrl+S 引导不生效

**症状**：按 `Ctrl+S` 没有触发引导，输入框毫无反应。

**原因**：终端开启了 XON/XOFF 流控，`Ctrl+S` 被终端截获。

**解决**：

1. 关闭终端的 XON/XOFF 流控：macOS/Linux 下执行 `stty -ixon`；
2. 快捷键完整说明见 [键盘快捷键参考](../reference/keyboard.md)。

### 粘贴图片没有反应

**症状**：按粘贴快捷键后，没有插入图片预览。

**原因**：模型不支持图片，或终端不支持图片协议。

**解决**：

1. 确认所用模型的 `capabilities` 包含 `images`（视觉模型）；
2. 确认终端支持图片协议（kitty / iTerm2）；
3. 图片粘贴的详细说明见 [交互模式](interaction.md)。

### 上下文太长、回答变差

**症状**：会话进行到一半，模型回答开始遗漏信息或明显变差。

**原因**：上下文过长，超出模型的有效处理范围。

**解决**：

1. 用 `/compact` 手动压缩上下文；
2. 或调整 `[runtime.compaction]` 配置（`enabled`、`trigger_ratio` 等）；
3. 配置说明见 [配置文件](../configuration/config-files.md)。

### 更新后版本没变化

**症状**：执行更新后，版本号没有变化。

**原因**：更新后的二进制要重启 Neo 才会生效。

**解决**：

1. 重启 Neo，使更新生效；
2. 需要回滚时，用 `neo update --rollback` 回到上一次安装；
3. 更新说明见 [快速开始](../quickstart.md)。

### MCP 服务器需要登录

**症状**：MCP 服务器提示需要认证，相关工具调用失败。

**原因**：远程服务器要求 OAuth 登录，尚未完成授权。

**解决**：

1. CLI 下用 `neo mcp auth <server_id>` 登录；
2. 或在 TUI 的 `/mcp` 面板中登录；
3. token 过期会自动刷新，无需手动处理；
4. 详细说明见 [MCP 服务器](../customization/mcp.md)。

### 主题不生效

**症状**：放到 `~/.neo/themes/` 的主题文件没有生效。

**原因**：颜色 token 名没有精确匹配（加载器对未知键会报错），或文件位置不对。

**解决**：

1. 确认颜色 token 名与文档表格完全一致；
2. 确认文件放在 `~/.neo/themes/` 下；
3. 详细说明见 [主题（Themes）](../customization/themes.md)。

### 如何备份 / 迁移数据

**症状**：换机器或重装后，希望把会话、配置等数据带过去。

**原因**：数据存放在本机数据目录（`NEO_HOME`），不随工作区迁移。

**解决**：

1. 设置 `NEO_HOME` 环境变量，整体迁移所有数据；
2. 只迁移会话时，用 `sessions_dir` 单独指向会话目录；
3. 数据位置的完整说明见 [数据存储位置](../configuration/data-locations.md)。

## 相关页面

- [快速开始](../quickstart.md)
- [Provider 配置](../configuration/providers.md)
- [会话管理](sessions.md)
- [键盘快捷键参考](../reference/keyboard.md)
- [交互模式](interaction.md)
- [配置文件](../configuration/config-files.md)
- [MCP 服务器](../customization/mcp.md)
- [主题（Themes）](../customization/themes.md)
- [数据存储位置](../configuration/data-locations.md)
