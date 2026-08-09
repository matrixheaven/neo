# Neo 图片视频输入与模型缓存车道实施计划

日期：`2026-08-10`

对应设计：`docs/aegis/specs/2026-08-10-media-input-model-cache-lanes-design.md`

状态：待设计说明审阅后执行

## Aegis 可见性

这是跨 `neo-ai`、`neo-agent-core`、`neo-agent` 和多个提供方序列化器的架构改动；计划先锁定持久化所有者、请求投影所有者、工具暴露边界和缓存键语义，避免把媒体降级、模型切换或提供方兼容逻辑散落到调用方。

## 计划依据

- 设计说明：`docs/aegis/specs/2026-08-10-media-input-model-cache-lanes-design.md`
- 现有请求构造：`crates/neo-agent-core/src/runtime/chat_request.rs`
- 现有消息与图像引用：`crates/neo-agent-core/src/messages.rs`
- 现有 Blob 解析：`crates/neo-agent-core/src/runtime/image_blobs.rs`
- 提供方中立类型：`crates/neo-ai/src/types.rs`
- 缓存基线：`docs/aegis/specs/2026-07-08-prompt-cache-hit-rate-design.md`
- 内存基线：`docs/aegis/specs/2026-08-08-bounded-runtime-memory-design.md`
- Kimi 只读参考：`.references/kimi-code/packages/agent-core-v2/src/agent/media/`

## 需求就绪检查

- 目标：`ready`
- 成功证据：历史不改写、四种媒体能力工具状态正确、模型切换投影正确、缓存车道稳定、媒体大小有界。
- 本计划不覆盖：Kimi 专属上传、自动抽帧、跨模型缓存命中、用户配置化缓存管理。
- 待确认：设计说明审阅；在此之前不执行产品代码任务。

## TDD Route

- Mode：`off`
- Decision：`skipped`
- Strict authority：不适用；用户没有要求严格测试驱动。
- Test posture：先增加最小回归，再运行定点行为测试。
- Reason：项目规则要求定点回归，但不要求为架构设计强行写失败测试循环。
- Verification：每个任务使用单包、单目标、单测试名或提供方协议目标；资源测试单独标记。

## 变更必要性

- 用户可见需求：读取视频并在模型切换后继续工作。
- 无代码选项：仅写工具提示或继续拒绝不支持模型，不能完成用户场景。
- 代码必要原因：当前类型、能力解析、请求投影和提供方序列化均无视频路径。
- 最小边界：现有消息类型、请求投影、工具装配、能力解析和各提供方媒体转换。
- 决定：`code-change`。

## 新表面存在性检查

- 新表面：视频内容部件、媒体投影、缓存车道键、`ReadMediaFile` 工具。
- 可复用所有者：复用现有 `Content`、Blob、`chat_request`、`ToolRegistry`、能力解析和提供方序列化器。
- 不足原因：现有路径只有 `Image`，不支持媒体投影，且模型能力校验会直接拒绝历史图像。
- 不新增：不另建媒体历史仓库、媒体注册中心或 Kimi 兼容适配器。
- 决定：`reuse-existing` 加最小字段和分支；新代码必须挂在现有所有者下。

## 架构完整性检查

- 不变量：canonical 消息和 Blob 追加式；请求投影可丢弃或改写媒体表示但不能改写来源。
- 持久化所有者：会话消息和 Blob 存储。
- 请求投影所有者：`chat_request` 及其媒体解析辅助函数。
- 提供方所有者：协议序列化器负责实际可发送形态，不由调用方猜测。
- 工具所有者：现有工具注册和执行链；工具表按本轮有效能力装配。
- 退休边界：删除图像专用引用的双轨别名和当前媒体拒绝分支；不保留第二套投影路径。
- 结论：`proceed`。

## 文件边界与任务

### 任务 1：统一媒体引用和消息部件

文件：

- `crates/neo-agent-core/src/messages.rs`
- `crates/neo-agent-core/src/runtime/image_blobs.rs`
- `crates/neo-ai/src/types.rs`
- 对应现有消息和类型测试文件

步骤：

1. 将共享引用命名为媒体引用，保留现有序列化形状和 Blob 摘要语义。
2. 增加视频消息部件和提供方内容部件。
3. 将工具执行结果从仅文字提升为文字加结构化媒体部件；`append_tool_result_messages` 不得再无条件压成 `Content::Text`。
4. 让 Blob 解析同时覆盖图像和视频，并在缺失 Blob 时返回确定性不可用状态，不生成空编码。
5. 更新消息到提供方内容的转换，明确未解析 Blob 不能静默成为空媒体。
6. 保持所有 canonical 消息构造、工具调用编号和重放顺序不变。

验证：

- `cargo nextest run -p neo-agent-core --lib resolve_content_blobs`
- `cargo nextest run -p neo-ai --test model_resolution_behavior model_capabilities_shapes_cover_default_and_helpers`
- 只运行受影响的消息序列化定点测试。

### 任务 2：能力解析与有效媒体传输

文件：

- `crates/neo-ai/src/types.rs`
- `crates/neo-ai/src/catalog.rs`
- `crates/neo-agent/src/modes/run/runtime/model.rs`
- 提供方注册和模型能力测试

步骤：

1. 增加视频语义能力字段及 `video_in`、`videos` 目录解析。
2. 为提供方定义最小传输能力查询，按用户消息和工具结果位置区分，不把目录能力当作传输保证。
3. 计算图像和视频在不同消息位置的有效能力交集。
4. 对无法编码或超出限制的媒体返回有类型错误；禁止静默丢弃。
5. 保持自定义模型未知能力默认关闭。

验证：

- 模型能力解析覆盖默认关闭、仅图像、仅视频、双能力。
- 每个提供方至少一个“声明支持但实际传输不支持”的拒绝测试。

### 任务 3：请求媒体投影和缓存车道

文件：

- `crates/neo-agent-core/src/runtime/chat_request.rs`
- 新增请求投影辅助文件仅当现有文件复杂度检查证明必须拆分
- `crates/neo-ai/src/options.rs`
- `crates/neo-ai/src/providers/openai/responses.rs`
- `crates/neo-ai/src/providers/openai/compatible.rs`
- `crates/neo-ai/src/providers/anthropic.rs`
- `crates/neo-ai/src/providers/google.rs`

步骤：

1. 在请求副本上先决定媒体投影，再读取 Blob、编码和做提供方内容转换；不触碰 `AgentContext`。
2. 可发送媒体按当前位置的有效传输模式编码；不可发送媒体使用固定摘要说明，且绝不读取 Blob。
3. 只替换媒体部件，保留工具调用、工具结果文字、调用编号和 N 个后续回合。
4. 若提供方不能重放当前工具表外的历史媒体调用，整体投影“助手调用加全部匹配结果”；不得留下孤立工具结果。
5. 若提供方仅支持用户媒体，工具结果媒体只能投影到完整交换之后的附加用户消息，不能插入交换内部。
6. 增加专用提示缓存键字段，保持现有会话标识字段语义不变。
7. 生成 `会话 + 提供方 + 模型 + 静态请求投影形状` 车道键，不放入历史和本轮输入。
8. 为每个提供方显式实现用户媒体、工具结果媒体、视频拒绝、缺失 Blob、历史工具交换和大小超限路径。
9. 删除当前“历史图像遇到无图像模型直接拒绝”的调用方分支；统一由投影决定可发送或说明。

验证：

- `A(双能力) -> B(无视频) -> A` 的完整请求投影行为测试。
- 断言 canonical 消息和 Blob 字节在请求前后逐字节一致。
- 断言 A、B 车道键不同，且同一车道不随历史追加变化。
- 各提供方请求体定点测试，包含历史 `ReadMediaFile` 工具交换和工具结果媒体位置。

### 任务 4：`ReadMediaFile` 工具

文件：

- 现有内置工具注册和执行模块（以实际 CodeGraph 所有者为准）
- `crates/neo-agent-core/src/runtime/tool_dispatch.rs` 及对应工具测试

步骤：

1. 复用现有文件读取和权限边界，加入媒体类型识别、Blob 写入和大小校验。
2. 根据有效能力装配工具说明和参数；无媒体能力时不暴露工具。
3. 执行期再次校验媒体类型与能力，拒绝过期或伪造调用。
4. 工具结果只追加结构化媒体内容，交给请求投影决定是否发送；禁止工具层直接构造提供方内联字段。
5. 不加入 Kimi 上传、远端缓存或自动抽帧。

验证：

- 四种能力组合的工具表精确断言。
- 图像参数不能作用于视频；仅视频模型不出现图像专用参数。
- 路径越界、未知类型、超限、读取失败均不写入不完整媒体记录。

### 任务 5：跨模块模型切换和回归

文件：

- `crates/neo-agent-core/tests/runtime_behavior/`
- `crates/neo-ai/tests/provider_protocol_behavior/`
- `crates/neo-agent/src/modes/interactive/` 中实际模型切换测试所有者

步骤：

1. 用 `FakeModelClient` 构造模型甲读取视频并追加 N 个回合。
2. 切换模型乙，验证乙收到固定说明和完整历史。
3. 在媒体仍处于活动上下文时切回甲，验证甲收到原始视频和乙期间新增尾部。
4. 在模型乙先使用过和未使用过两种情况下分别验证缓存车道。
5. 压缩后切回甲，验证旧视频不重新注入，稳定来源说明仍在；重新 `ReadMediaFile` 会追加新交换。
6. 验证运行中的回合不会因模型切换改变；切换只作用于下一次请求。

验证：

- 每个跨模块行为测试只覆盖新增风险，不复制底层编码参数矩阵。
- 资源测试使用最小能触发上限的数据，名称以 `_resource` 结尾。

## 执行顺序与提交边界

1. 任务 1，先稳定数据模型和 Blob 语义。
2. 任务 2，接入能力和传输查询。
3. 任务 3，完成请求投影和缓存键。
4. 任务 4，接入工具。
5. 任务 5，补跨模块回归并完成提供方审查。

每个任务单独验证并单独提交；只暂存该任务拥有的文件。不得暂存当前工作树已有 Git 状态改动。

## 兼容、退休与风险

- 旧 JSONL 中没有视频的记录继续按原形状重放。
- 旧图像引用迁移到共享媒体引用后只保留一个源码所有者，不保留旧别名。
- 旧的无图像模型请求前拒绝逻辑在任务 3 中退休，由统一媒体投影取代。
- 若某提供方无法合法重放历史媒体工具交换，优先在其适配器请求副本中整体稳定转换；无法证明转换正确时，明确拒绝该请求，不发送不完整请求。
- 内联视频的资源上限必须由实现和资源测试共同证明，不能以 Kimi 的 100MB 说明推导 Neo 的安全上限。
- 压缩后不重新注入已越过边界的原媒体；稳定来源说明和重新读取语义是唯一路径。

## 计划压力测试

- 所有者：已有消息、Blob、请求构造、工具注册和提供方序列化路径。
- 复杂度：增加一个视频分支、一个统一投影边界和一个专用缓存键字段；不创建第二套历史或媒体服务。
- 验证：单元、提供方协议和运行时模型切换各有最小定点测试。
- 退出条件：设计未获审阅、提供方历史工具交换语义无法证明、或媒体内存上限无法稳定验证时暂停，不绕过错误。
