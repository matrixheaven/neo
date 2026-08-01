# Read 工具 Token 缓存优化 Design Spec

Date: `2026-08-02`
Status: `approved design`

## 1. Purpose

降低 Neo 会话中 Read 工具造成的 uncached input tokens 总量与波动，提升 DeepSeek/Anthropic 前缀缓存的经济性。设计基于 `tools/cache_probe.py` 实测数据与 `.references/reasonix` 的 `read_file` 工具对比结论。

范围：只改 Neo 本体（`neo-agent-core` / `neo-agent`），不改 reasonix；不改变任何 wire 协议、会话 JSONL 格式、权限模型。

## 2. 实测基线（Evidence）

来源：`target/cache-probe/20260801-162932-d0877b/report.json`（132 请求）。

| 指标 | 值 |
| --- | --- |
| 全 run cache_hit_tokens | 23,081,984 |
| 全 run uncached_input_tokens | 165,621 |
| Read 请求数 / uncached 占比 | 17 次 / 61,706 tokens（**37%**） |
| Read variance / stdev | 13,851,014 / 3,721.7（全场最高） |

规律（逐请求分析）：

- `uncached ≈ 该次 Read 返回的文件内容 tokens`，每请求固定开销仅 ~10-40 tokens。
- 高 uncached 样本全部是**全文件读**：req4（44KB→9,698）、req5（38KB→9,503）、req32（36KB→9,760，cache_probe.html 第二次全读）、req6/7/8（8.4K/8.2K/5.2K）。
- 定点行读（`line_offset`/`n_lines` 窗口，0.9-7.7KB）只需 240-1,900 uncached。
- 同 run 中 `cache_probe.py`（2,694 行）被分 5 次读完（1-1000、1001-1800、1801-2525、2450-2590、2400-2455），合计 ~125KB ≈ 31K uncached。

reasonix 差距根源（对照 `.references/reasonix`）：

1. `read_file` 声明 `SnipHint{Head:120, Tail:12, HeadChars:12000, TailChars:2000}`（`internal/tool/builtin/readfile.go:70-72`）；agent 在 context 收紧带用**确定性、零 LLM 调用**的 pass 把过时的大 tool result 重写为头+尾片段（`internal/agent/prune.go:38-169`），原件归档、提示模型可重跑。Neo 的 `compaction/projection.rs` 只有"全删变 marker"（`project_tool_result`，projection.rs:118-148），且仅在压缩压力下触发（`turn_loop.rs:1010-1068` 由 `CompactionController` 门控）。
2. `read_file` 默认 2000 行/次（readfile.go:45），减少大文件往返次数；Neo 默认 1000 行（`read.rs:8` MAX_LINES）。
3. `read_file` 尾部提示一行（readfile.go:251-253）；Neo 的 `<system>` 脚注约 5 句（read.rs:232-274）。
4. reasonix 对**重复读取**无专门处理；但旧读取会被 snip，历史上不无限堆积。

## 3. 需求与验收（Requirement Ready Check）

| 项 | 内容 |
| --- | --- |
| 需求来源 | 本会话结论（用户要求）：Read 是 per-tool uncached variance 最大来源，需优化 |
| 目标 | ① 旧大 Read 结果在模型输入中被确定性压缩（头+尾）；② 同内容重复读取不再重复注入；③ 模型被引导定点读、默认窗口更小；④ 压缩输入投影保留头尾而非全删 |
| 验收标准 | 见 §7 |
| 开放问题 | 无（设计已定） |
| 结论 | `ready` |

## 4. 设计决策

四个方案合并为三条默认开启 + 一条复用现有开关：

### 4.1 Snip 能力（方案 1 + 方案 4 合并）— 默认开启

**机制**：给 `Tool` trait 增加默认方法 `fn snip_hint(&self) -> Option<SnipHint>`（默认 `None`，零破坏）；`ReadTool` 实现 reasonix 同款几何（head 120 行 / tail 12 行 / head 12,000 字符 / tail 2,000 字符）。`ToolRegistry::register` 把 hint 登记进进程级 name→hint 表（`snip_hint_for(name)`），策略随工具定义走，与 reasonix 设计意图一致且免去把 registry 传进 `chat_request` 的管道改造。

`compaction/projection.rs` 的 `ProjectionPlan` 新增 3 字段：`snip_enabled: bool`、`snip_min_tokens: usize`、`snip_keep_recent: usize`。`project_tool_result` 改为三态：

1. **dedup**（见 4.2）；
2. **snip**：`snip_enabled && !is_error && index < len - snip_keep_recent && content_tokens >= snip_min_tokens && 工具声明了 hint` → 输出
   `[tool result snipped: tool=Read, approx_tokens=N; first H and last T lines shown; full content retained in session history; re-run Read to restore]` + 头 H 行 + `[... K lines omitted ...]` + 尾 T 行（单行超长时按字符窗截断）；
3. **既有全删 omission**（micro compaction 路径，未声明 hint 的工具保持原行为）。

**触发**：`turn_loop::request_projection_plan` 改为 `enabled = micro_enabled || snip_enabled`；`prepare_model_request` 的 `NoAction` 分支返回 `snapshot.projection`（即带 snip 的 request plan）而不是 `ProjectionPlan::disabled()`。压缩 controller 零改动（`UseProjectionOnly` 已经透传 `snapshot.projection`）。`summary.rs` 的 SummaryInput plan 同样带上 snip 字段。

**缓存经济学（2026-08-02 修订）**：初版论证"一次小 miss 换长期小历史"把每个结果的一次性改写当成了总量，**低估了会话增长中结果持续越过 keep-recent 边界造成的反复断前缀**。真实机制：某条 Read 结果在它之后累计 16+ 条消息时才被剪（`snip_keep_recent`），此时它的投影形态从"全文"变为"头尾片段"，与该条消息处的缓存前缀不一致 → 该消息之后全部 uncached 一次；会话越长，越多的合格结果依次越过边界 → **反复断前缀**。频率 ≈ 合格 Read 结果的产生速率（实测 run 中 Read 约占消息的 6%，约每 8 轮断一次，每次重新计费最近 20-60K tokens 的 miss）。这会把缓存命中率从纯追加会话的 99%+ 拉低，与 micro compaction 同一失败类别（micro 更严重：它对所有 ≥1000 tokens 的结果每轮都断）。**结论：snip/dedup 对付费 provider 不是无代价优化，默认必须关闭（`snip_enabled=false`），仅本地/实验模型可开启。**

**触发策略（2026-08-02 修订为 reasonix 占用带模型）**：初版采用"陈旧即剪（每请求、按消息年龄）"——结果一越过 keep-recent 边界就改写，会话增长中反复断前缀，实测会拉低命中率。现改为 reasonix 的占用带触发：仅当**会话累计占用**（`context.estimated_tokens()`）达到 `snip_trigger_ratio`（默认 0.6）× 模型上下文窗口时才启用 snip/dedup 维护 pass；低于该带时 `request_projection_plan` 返回 disabled，前缀保持**纯追加、缓存稳定**。窗口未知（`max_context_tokens=None`）时永不进入该带。这样健康会话（1M 窗口下 200K，20%）零改写；只有会话真正逼近窗口（60%）才做一次性剪枝，且剪后低于触发点即停。与 micro compaction 的"每轮改写"彻底区分。

**否决的替代**：durable 重写会话历史 + 归档（reasonix 做法）——Neo 会话 JSONL 本身就是归档，projection 是 ephemeral 变换（projection.rs:3-4 既有哲学），durable 改写需要新的会话重写机制，收益不变，风险更高。否决。

### 4.2 同内容重读去重（方案 2）— 默认开启，随 snip 开关

**机制**：`project_messages` 单遍处理时维护 `seen_full: HashMap<(Arc<str> 工具名, u64 hash), (usize 首见下标, Arc<str> 原文)>`。对**声明了 snip hint 的工具**（现仅 Read）且 `!is_error` 且纯文本且 ≥ 1,024 字节的 ToolResult：若其内容与更早一个**本次请求中仍完整可见**的结果字节相同（FNV-1a hash 命中后按字符串全等校验），把后者替换为
`[duplicate of an earlier Read result in this request (message index N); byte-identical content omitted; re-run Read to restore full content]`。

**关键正确性**：只有"更早结果在本请求中保持完整"才可去重（snip/omission 过的结果内容已变，天然不匹配；被压缩掉的更早结果不存在于本次输入，天然不去重）。去重发生在 projection 单遍内、dedup 优先于 snip/omission，因此：
- 跨会话 resume：历史驱动，正确；
- 压缩后重读：更早副本已被摘要取代 → 无匹配 → 全文重读，正确；
- 前缀破坏成本：被替换的是"较晚"的重复消息，破坏点之后的仅是新近尾段，尖峰小。

**否决的替代**：工具层按 (path, mtime, size) 缓存去重——无法感知压缩/恢复后更早内容是否仍在上下文中，会产生"提示已读但模型实际看不到"的死循环风险。否决。工具内 `ToolResult.details` 不落 AgentMessage 历史，无法携带 mtime，方案不成立。

### 4.3 引导定点读 + 默认窗口（方案 3）— 默认开启

- `read.rs` 描述文本强化："prefer targeted reads：超过 ~200 行的文件用 `line_offset`/`n_lines` 只读所需范围，不要整文件读；小窗口保持上下文与缓存更小"。对照 reasonix 描述（readfile.go:51）。
- 新增 `DEFAULT_LINES: usize = 400`，`n_lines` 缺省时用 400（`MAX_LINES=1000` 仍是硬上限）。单次读取最大注入从 ~11K tokens 降到 ~4.3K，方差上界直接下降；大文件往返次数增加的成本（~10-40 tokens/次）可忽略。
- 保留行号前缀格式（Edit 工具依赖）与 `<system>` 脚注（信息量 > 成本）。

### 4.4 配置面

- `neo-agent/src/config/mod.rs` `RuntimeCompactionConfig`：+`snip_enabled: bool`（serde default **false**）、`snip_min_tokens: usize`（1000）、`snip_keep_recent: usize`（16）、`snip_trigger_ratio: f64`（0.6，占用带阈值）。旧配置省略新键不破坏（`#[serde(default)]`）。
- `neo-agent-core/src/runtime/config.rs` `CompactionSettings`：同 4 字段；`CompactionSettings::new` 默认 **`snip_enabled=false`**、`snip_trigger_ratio=0.6`（与 micro 同为前缀改写类，付费 provider 默认关）。所有 struct-literal 站点同步。
- 语义：`config.compaction` 为 `None`（无上下文窗口）时 snip/dedup 不生效（与 reasonix `contextWindow<=0` 一致）。`micro_enabled`（全删 omission）行为不变。`snip_enabled` 同时控制 snip 与 dedup（同一维护 pass，同一开关）。

## 5. 兼容性边界 / Non-goals

- **不改**：会话 JSONL 持久格式、wire 协议、tool schema、权限/plan 模式、`AgentMessage` 变体。
- **不改**：reasonix、`tools/cache_probe.py`、`.references/*`。
- **不引入**：durable 历史改写、归档目录、新的 trigger 波段、bash 等非 hint 工具的 snip（v1 保守，未来可加默认分层）。
- projection 仍是 ephemeral：TUI/transcript 显示全量原文，模型输入显示压缩版；两者差异是既有 projection 语义的延续。
- 确定性：snip/dedup 输出是 `(messages, plan)` 的纯函数，重放/resume 一致。

## 6. 风险

| 风险 | 等级 | 缓解 |
| --- | --- | --- |
| **前缀改写导致缓存命中率下降（与 micro 同类，最高风险）**：每条约定的 Read 结果在越过 keep-recent 边界时断一次前缀，会话越长断得越频繁；付费 provider 的 hit 率从纯追加会话的 99%+ 下降，miss 单价为 hit 的 ~4 倍 | **高（付费场景）** | **默认 `snip_enabled=false`**；仅本地/实验模型开启；文档明确警告 |
| 模型在需要被剪内容的场景信息缺失 | 低 | marker 明示"已入会话历史，重跑 Read 恢复"；剪枝仅作用于 >16 条消息前、≥1000 tokens 的旧结果 |
| dedup 误判（hash 碰撞） | 极低 | FNV-1a 命中后字符串全等校验 |
| 测试行为漂移（开启时 projection 在更多测试中启用） | 低 | 新会话无陈旧大结果时 snip/dedup 均为 no-op；断言用显式 plan 的测试不受影响 |
| 多内容块（含 Image）结果 | 低 | 非纯文本一律跳过 snip/dedup，原样保留 |

## 7. 验收标准（Acceptance）

1. `cargo nextest run -p neo-agent-core --lib` 中 projection 模块全部通过，覆盖：snip 头尾几何、`snip_keep_recent` 保护、`snip_min_tokens` 门槛、错误结果跳过、dedup 字节相同/不同/错误、dedup 与 snip 互斥顺序、`ProjectionPlan::disabled()` 全关。
2. `cargo nextest run -p neo-agent-core --lib tools::read` 通过：默认窗口 400 行、描述含定点读引导、`MAX_LINES` 上限回归。
3. `cargo nextest run -p neo-agent-core --lib tools` 通过：`snip_hint_for` 注册/查询。
4. `cargo nextest run -p neo-agent --bin neo` 对应 config 测试通过：新字段默认值、旧配置兼容。
5. `cargo fmt --all --check`、`cargo clippy -p neo-agent-core --lib -- -D clippy::all` 通过。
6. （可选手工）用 `tools/cache_probe.py` 跑一段含多文件读取的会话：Read 的 `uncached` 均值/方差下降、`cache_hit_tokens` 总量下降，且不再出现同文件重复全读注入。

## 8. 参考

- `tools/cache_probe.py`（分析工具）
- `target/cache-probe/20260801-162932-d0877b/report.json`（基线数据）
- `.references/reasonix/internal/tool/builtin/readfile.go`、`internal/agent/prune.go`、`internal/tool/tool.go`（SnipHint）、`internal/agent/compact.go`（触发带）
- `crates/neo-agent-core/src/compaction/projection.rs`、`runtime/turn_loop.rs`、`runtime/context_budget.rs`、`runtime/compaction_controller.rs`、`runtime/config.rs`、`tools/read.rs`、`tools/mod.rs`
- `docs/aegis/specs/2026-08-01-deepseek-cache-probe-design.md`（探测工具设计）
- `docs/en/configuration/config-files.md`、`docs/zh/configuration/config-files.md`（`[runtime.compaction]` 文档）
