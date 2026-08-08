# Delegate 系列结果交付设计

Status: `approved for planning`
Date: `2026-08-08`

## 1. 目标

让主代理可靠获得 `Delegate`、`DelegateGroup` 和 `DelegateSwarm` 的最终交付物。
结果可以直接进入主代理的下一次决策；只有确实超过现有工具输出预算时才分页读取。

本设计解决两类已观察问题：

1. 子代理完成后主代理只收到 512 字符左右的摘要，无法知道实际交付内容。
2. 集群完成后主代理只收到聚合统计，子代理结果藏在模型不可见的 `details` 中。

## 2. 当前缺陷与决定性证据

下一次模型请求消费 `ToolResult.content`，不会自动看到 `ToolResult.details`。目前：

- `multi_agent/runtime.rs` 的 `bounded_latest_text` 把终态文本限制在 512 字符；
- `multi_agent_format.rs::delegate_result_content` 只生成状态和摘要；
- `delegate.rs::swarm_run_result` 只把聚合统计放进可见文本，子项结果放进 `details.items`；
- `delegate_controls.rs` 的 `WaitDelegate` 终态只返回等待状态；
- `background_tasks.rs::TaskOutputTool` 对代理和集群再次复用摘要路径；
- 中英文用户指南仍声称主代理只能得到结果摘要。

提示词不清是次要问题。即使提示词写得很好，完整内容仍然不可见，模型也无法凭空恢复它。

## 3. 已确认的设计选择

### 3.1 自动内嵌，超限分页

- 结果能装入现有完整工具结果预算时，直接返回完整最终回答。
- 结果超限时，返回第一页、总大小、稳定游标和已经填好参数的下一步 `TaskOutput` 调用。
- 不采用“所有结果始终分页”，避免正常任务增加无意义的额外工具调用。

### 3.2 一个结果来源

子代理会话中的规范消息和终态事件是完整结果的唯一来源。`AgentSnapshot.outcome.summary`、进度快照和界面卡片内容都是派生预览，不能充当完整结果。

### 3.3 一个整理入口

在现有 `tools/multi_agent_format.rs` 中保留一个 Delegate 系列模型结果整理入口，供前台完成、后台完成通知、`WaitDelegate`、`DelegateSwarm` 和 `TaskOutput` 复用。不得在调用方分别拼接摘要、状态或下一步提示。

`DelegateGroup` 没有独立的结果生产者或结果格式；它的子代理结果和外层展示沿用同一整理入口，不增加第三种结果约定。

## 4. 模型可见结果格式

模型可见内容使用紧凑 JSON。字段命名保持稳定，空字段省略；界面摘要仍从类型化 `details` 渲染。

### 4.1 单个代理

```json
{
  "ok": true,
  "kind": "delegate_result",
  "target": {"kind": "agent", "id": "agent_xxx"},
  "status": "completed",
  "result": {
    "mode": "inline",
    "text": "完整最终回答",
    "total_chars": 1234,
    "has_more": false
  },
  "next_actions": []
}
```

当结果超出预算时，`result.mode` 为 `page`，`text` 是当前页，另外包含 `total_chars`、`has_more` 和不透明 `cursor`。`next_actions` 必须包含唯一、可直接执行的 `TaskOutput` 调用：

```json
{
  "tool": "TaskOutput",
  "arguments": {
    "task_id": "agent_xxx",
    "view": "result",
    "cursor": "opaque-cursor"
  }
}
```

### 4.2 集群

```json
{
  "ok": true,
  "kind": "delegate_swarm_result",
  "target": {"kind": "swarm", "id": "swarm_xxx"},
  "status": "completed",
  "aggregate": {"total": 2, "completed": 2, "failed": 0},
  "items": [
    {
      "index": 0,
      "agent_id": "agent_a",
      "title": "检查实现",
      "status": "completed",
      "result": {"mode": "inline", "text": "...", "total_chars": 80, "has_more": false}
    }
  ],
  "next_actions": []
}
```

`items` 必须按请求顺序返回，保留每个子项的 `agent_id`、状态和结果。某项过大时，只分页该项，并在该项结果中放入精确的 `TaskOutput` 调用；不得退化为只有 `aggregate` 的返回。

### 4.3 失败、取消与超时

终态仍必须返回 `ok`、目标标识、真实状态、可用的错误信息和恢复动作。失败子项不能伪装成成功，也不能用空摘要覆盖真实错误。没有最终回答时 `result` 省略或为 `null`，但状态和错误仍完整可见。

## 5. 工具行为

### 5.1 `Delegate`

前台模式在子代理终止后使用统一整理入口返回终态结果。后台模式立即返回任务标识和 `WaitDelegate` 的唯一等待动作；自动完成通知使用同一终态整理入口。

### 5.2 `WaitDelegate`

等待职责不变。目标全部终止后，返回和 `Delegate` 相同的单代理或集群结果；超时只返回当前状态、未完成目标和继续等待的精确调用，不建议改用轮询组合。

### 5.3 `TaskOutput`

保留现有 `summary`、`journal`、`artifacts` 等工作流视图。对代理和集群增加并启用已有 `view="result"` 语义：从规范消息记录按游标返回最终回答页面。错误游标、未知目标、非终态目标和已无结果目标都返回明确的可分支错误。

游标绑定目标、视图和查询，不允许模型自行构造；页面按 UTF-8 字符边界收缩，完整 `ToolResult` 仍受现有字节上限约束。

### 5.4 `ListDelegates`

继续负责发现、过滤和状态列表，不承担完整结果恢复；文案明确说明它不是结果读取入口。

## 6. 持久化、内存与上下文

- 完整回答继续追加写入子代理规范会话日志，历史消息顺序不变。
- 进度快照继续使用 512 字符预览，终态快照不得复制完整回答。
- 分页读取只保留当前页面、固定大小解析缓冲和游标状态，不建立第二份长文本缓存。
- 不修改主代理缓存前缀、系统提示、历史消息或规范事件；结果投影只是追加到当前工具结果的派生视图。
- 不启用微压缩、去重裁剪或结果重写来解决本问题。

## 7. 用户引导

更新 `Delegate`、`DelegateSwarm`、`WaitDelegate`、`TaskOutput` 的工具描述，以及中英文代理指南：

- 结果在返回中时直接使用；
- 返回 `next_actions` 时严格执行其中的 `TaskOutput` 调用；
- `ListDelegates` 只用于发现和状态；
- 不需要猜测隐藏字段或重复运行子代理来恢复结果。

## 8. 兼容边界与非目标

必须保持：

- `Delegate`、`DelegateGroup`、`DelegateSwarm` 卡片的布局、层级、顺序、进度、展开语义和 transcript 位置；
- 规范会话 JSONL 的追加式写入、重放、缓存前缀和消息顺序；
- `WaitDelegate` 的等待和取消生命周期；
- 现有 `TaskOutput` 工作流视图、游标错误和字节上限。

本设计不做：

- 新增结果工具、第二个结果存储、后台轮询器或兼容别名；
- 调大统一工具输出上限；
- 把全部历史对话自动灌回主代理；
- 修改界面卡片内容以掩盖模型可见结果缺失；
- 改造主代理上下文压缩、重试或提供商协议。

## 9. 验收标准

1. 512 字符以上但仍在工具预算内的单代理回答，在 `ToolResult.content` 中逐字可见。
2. 超预算单代理回答可通过返回的首个游标和连续 `TaskOutput(view="result")` 调用完整重建，顺序和 UTF-8 内容不变。
3. `WaitDelegate` 的终态结果与 `Delegate` 使用同一格式，不能只返回状态。
4. 集群返回有序子项、每项标识、状态和摘要；每项完整结果可内嵌或分页，不能只有聚合统计。
5. 失败、取消、超时和未知目标均提供真实状态、错误和可执行后续动作。
6. `ToolResult.details` 缺失时，主代理仍能完成读取结果所需的下一步决策。
7. 运行中进度仍受 512 字符限制，现有卡片和持久化记录不变。
8. 用户指南不再声称主代理只能得到结果摘要。

## 10. 架构审查信号

- 规范来源：`docs/aegis/specs/2026-07-30-workflow-model-visible-results-design.md`、`docs/aegis/adr/ADR-0008-workflow-product-surface-contract.md`、`docs/aegis/specs/2026-08-08-bounded-runtime-memory-design.md`。
- 设计结论：需求和现有工作流结果原则一致，当前实现属于模型可见投影漂移，范围为 `architecture`。
- 存在性检查：复用现有 `TaskOutput`、会话 JSONL 读取器和 `multi_agent_format.rs`；不新增工具、存储或结果所有者。
- 后续基线同步：实现并验证后补充 Delegate 系列结果投影的落地基线；本规格不替代运行时权威文档。
