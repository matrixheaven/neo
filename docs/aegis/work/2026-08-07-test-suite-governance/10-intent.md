# 测试套件治理意图

## TaskIntentDraft

- 结果：形成四个 crate 共用的测试治理设计、计划和交接，并以本机性能为主目标。
- 成功证据：静态基线、明确删除门槛、统一结构、可执行任务和强约束交接。
- 停止条件：需要修改生产行为、无证据删除高价值测试或扩大范围。
- 非目标：本轮不执行测试清理，不运行一小时本机完整测试。

## BaselineReadSetHint

- `AGENTS.md`
- `.config/nextest.toml`
- `.github/workflows/ci.yml`
- 四个 crate 的测试分布和巨型文件

## BaselineUsageDraft

- 已确认：根规则、当前 Nextest 分组、当前持续集成命令、四个 crate 的只读审计。
- 缺失：当前本机分段耗时，由实施 Task 1 获取。
- 决定：继续编写交接，但禁止用远端耗时代替本机基线。

## ImpactStatementDraft

- 影响：测试文件结构、测试价值、Nextest 调度和持续集成步骤。
- 不影响：生产行为、公开接口、持久化、上下文和用户界面。
