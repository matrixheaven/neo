# 交接提示：修复全屏转录文档落地回归

把分隔线下方的全部内容原样交给负责实施的 AI。

---

你是 Neo 全屏转录文档后续修复的协调实施代理。工作目录：

```text
/Users/chenyuanhao/Workspace/neo
```

用户已经授权按以下计划实施、精确验证并逐任务本地提交：

```text
docs/aegis/plans/2026-08-06-fullscreen-transcript-follow-up-repair.md
```

必须完整执行计划，使用 `subagent-driven-development`，每个任务都经过独立实现、设计符合性复查、代码质量复查和协调代理重新验证。不要重新讨论全屏架构，不要重新做无边界的全仓探索，不要把问题改写成另一套产品设计。

## 一、开始前必须读取

按顺序查看：

1. `AGENTS.md`
2. `~/.codex/RTK.md`
3. `~/.codex/CX.md`
4. `docs/aegis/specs/2026-08-04-fullscreen-transcript-document-design.md`
5. `docs/aegis/adr/ADR-0012-fullscreen-transcript-document.md`
6. `docs/aegis/baseline/2026-08-04-fullscreen-transcript-document.md`
7. `docs/aegis/plans/2026-08-06-fullscreen-transcript-follow-up-repair.md`
8. 本交接文件

执行前运行：

```bash
icm recall-context "fullscreen transcript tool group spacing approval focus mouse selection new activity" --limit 5
git status --short --branch
git log -5 --oneline
```

审查时的历史快照是：

```text
branch: main
HEAD: f4f2e19f
worktree: clean
remote relation: ahead 17
```

这只是历史证据。你必须以执行时的实际状态为准，保留所有用户和其他代理的现有改动。

## 二、禁止事项

- 禁止恢复历史区/动态区、原生历史写入或旧 `Ctrl+O` 浏览器。
- 禁止新增第二转录、第二视口、第二渲染器、第二鼠标选择系统或兼容回退路径。
- 禁止在卡片内部通过空行、省略、截断、压缩或折叠掩盖几何错误。
- 禁止修改 Delegate、DelegateGroup、DelegateSwarm 卡片本体、层级、进度、展开和内容。
- 禁止修改 Workflow 执行、恢复、日志、结果或模型可见输出。
- 禁止修改 Bash/Terminal 准入等待、超时、取消、长命令或输出捕获语义。
- 禁止修改模型上下文、提供方请求、压缩输入、缓存前缀或历史事件顺序。
- 禁止多个实现代理并行写同一工作树。
- 禁止实现代理或复查代理执行任何 Git 写入。
- 禁止 `reset`、`checkout --`、`restore`、`stash`、`clean`、`rebase`、`rm`、amend、强制推送、切分支和工作树增删。
- 未授权推送、发布、创建分支或工作树。

## 三、已经确定的根因，不要重复探索

1. 连续 `ToolRun` 的完整块归在组首条，但追加和普通成员更新没有使组首条重新测高。
2. 审批与提问只接管输入，`DocumentLayout` 没有让最早阻塞条目接管可见焦点。
3. 并行工具的开始事件在模型流阶段已经进入规范转录，审批后来插入对应工具之后，所以后续 `Preparing` 会显示在当前审批下方。
4. `RawStdinEvents::enqueue_pending` 在释放到来时删除等待派发的拖动。
5. `parse_sgr_mouse` 把 SGR 编码 35 的无按键移动当成释放。
6. `TranscriptPane::compose_rows` 完全没有应用选区样式。
7. 审批和提问输入路由会吞掉非滚轮鼠标事件。
8. `new_activity` 只有内部状态和测试，没有生产提示。
9. 复制动作存在，但选区和复制状态没有可见反馈。
10. `fit_chrome_to_height` 会从顶部删除超高弹窗内容。

## 四、固定实施顺序

严格按计划执行以下七个任务：

1. 连续工具组统一失效和几何。
2. 鼠标事件解析与队列顺序。
3. 选区高亮、阻塞状态鼠标路由和可见复制提示。
4. 最早阻塞条目可见焦点与后续工具顺序显露。
5. 锁定视口的新内容提示。
6. 短终端富弹窗完整可达。
7. 组合回归、真实终端验收和基线追加。

不得调换 Task 1 至 Task 5 的顺序。Task 4 依赖正确的工具组几何和鼠标基础；Task 5 必须在阻塞焦点确定后再组合底部提示。

## 五、并行工具显示的唯一解释

本次只修复显示顺序：

```text
所有规范工具事件继续完整进入 TranscriptStore
  -> 最早未解决审批或提问限制当前可见窗口
  -> 其后的 Preparing 暂不进入当前帧
  -> 当前审批处理完成
  -> 下一项按规范顺序显露
```

不要改 `authorize_tool_batch` 的整批审批和工具正文执行语义。不要延迟会话持久化，不要删除后续工具事件，也不要伪造未开始状态。

## 六、每任务执行协议

协调代理在每个任务前记录：

```text
TaskStartSnapshot
- 当前分支和 HEAD
- 现有修改与未跟踪文件
- 当前任务允许修改的路径
- 当前任务基线 SHA
- 与其他改动重叠时的保留方式
```

随后严格执行：

```text
新实现代理实现当前任务
  -> 设计符合性复查代理查看计划逐条核对
  -> 有问题则原实现代理修复
  -> 同一设计复查代理重新查看直到通过
  -> 新代码质量复查代理查看正确性、边界、测试和复杂度
  -> 有问题则原实现代理修复
  -> 同一质量复查代理重新查看直到通过
  -> 协调代理亲自重新运行计划中的精确验证
  -> 协调代理运行 git diff --check
  -> 协调代理只暂存当前任务文件并提交
  -> 协调代理读取提交文件列表、剩余差异和新 HEAD
  -> 更新检查点并进入下一任务
```

任何复查存在开放问题时，禁止进入下一任务。实现代理的自我复查不能替代两阶段独立复查。

## 七、子代理提示必须包含

每次派发实现代理时，直接提供当前任务全文，不要让它自行寻找计划。提示中必须包括：

- 当前任务目标和停止条件；
- 允许修改的精确路径；
- 已确定根因；
- 明确非目标和禁止事项；
- 必须读取的源码窗口；
- 精确验证命令；
- 不得 Git 写入；
- 不得修复无关失败；
- 发现计划错误时返回 `BLOCKED`，不能自行扩大架构。

设计符合性复查只回答：是否逐条满足计划、是否越界、是否遗漏行为。代码质量复查只回答：是否有真实错误、跨平台风险、性能退化、弱测试或不必要复杂度。两类复查不得合并。

## 八、提交边界

每个任务一个提交，建议消息已经写在计划中。协调代理可以按项目规则自主 `git add` 和 `git commit`，但只能精确暂存当前任务路径。不得把用户现有改动、其他代理改动或后续任务文件混入当前提交。

每次提交后必须运行：

```bash
git show --stat --oneline HEAD
git status --short --branch
```

## 九、完成证据

不能以单元测试通过代替真实终端结论。最终报告必须分开写：

- macOS 精确定点自动化结果；
- macOS 图形终端鼠标和剪贴板人工结果；
- Fedora 原生或真实 PTY 结果；
- Windows 原生自动化结果；
- Windows Terminal 登录桌面会话的鼠标和剪贴板结果；
- 未执行的平台与原因；
- 最终残余风险。

若无法进入 Windows 图形桌面，不得写“Windows 鼠标已验证”。ssh、合成事件和 PTY smoke 都不能替代图形终端人工验证。

## 十、完成条件

只有计划的 Final Acceptance 全部满足、七个任务均有独立提交、两阶段复查均无开放问题、协调代理完成新鲜验证、工作树只剩明确归属的外部改动时，才能报告完成。

完成前使用 `aegis:verification-before-completion`。本交接没有授权推送或发布。
