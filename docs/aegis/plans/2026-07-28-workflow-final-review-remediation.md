# Workflow Final Review Remediation Plan

Date: 2026-07-28

Parent authority: `docs/aegis/work/2026-07-28-workflow-final-review/final-review.md`

## Goal

修复 final review 中已确认的 Workflow 产品故障，并在合入后进行一次跨模块验收。目标是功能正确、能力不衰退、删除旧 Workflow journal 双轨、保留既有 transcript 卡片设计。

## Scope fence

- 允许重写 Workflow 内部实现；不以改写成本限制能力。
- 不修改 Delegate、DelegateSwarm、Workflow transcript 卡片布局和展开语义。
- 不新增第二 runtime、第二 task manager、第二 persistence store 或第二权限 owner。
- 不删除全局 session context compaction；只删除 Workflow 内旧 journal/compact/version 路径。
- 保留用户已有 `crates/neo-agent/tests/workflow_cli.rs` 改动，不覆盖、不回滚。
- 每个切片在独立 worktree 完成并提交；主树只负责集成和最终 review。

## TDD Route

- Mode: off
- Decision: skipped
- Test posture: targeted post-change regression
- Reason: 用户未要求严格 test-first；每个切片补能证明自身合同的最小精确测试。

## Task slices

### Slice 1: Canonical Workflow journal

Owner: `neo-agent-core/workflow`

删除旧代际命名、旧 writer/scanner/recovery、版本分流和仅保护旧路径的 tests。保留一个 canonical envelope/writer/reader/recovery，并让新 run、resume、output、retention、harness、child projection 只走这一条。不得做迁移、双写或旧兼容。

Verification: canonical source retirement scan；journal round-trip、recovery、child lifecycle、read/write/replay 精确测试。

### Slice 2: `/tasks` Workflow surface and elapsed

Owner: `neo-agent` + `neo-tui` + `BackgroundTaskManager`

在现有 Task Browser overlay 内接入 Workflow 子视图。Workflow 选中后 Enter 打开，O 在 Workflow 面内切换详情/输出；普通 task 保持旧行为。elapsed 由 durable timestamps/terminal elapsed 计算，终态冻结，重启保持。

Verification: exact input/render tests；terminal elapsed freeze；rehydrate elapsed preservation；普通 task regression。

### Slice 3: Permission and live event correctness

Owner: `interactive` + `workflow_dispatch` + `turn` + event router

让 Workflow child 使用当前 live permission mode；修复 persistence flush 阻塞 live card、finished turn 丢弃未消费事件、后台 workflow event 改写 foreground streaming mode。

Verification: nested Yolo child no approval；delayed delivery visible before flush；257+ event terminal drain；background event does not set footer working。

### Slice 4: Durable approved workflow presentation

Owner: workflow approval/transcript/session projection

批准后保留可展开 Workflow script/source box，resume/replay 从 durable event 或 canonical projection 恢复，不依赖 pending approval state。

Verification: resolve approval retains source；session replay/resume restores source box。

### Slice 5: Authoring contracts and discovery

Owner: workflow Lua host, DelegateSwarm projection, List/Glob, skill prompt, slash completion

让 swarm item structured output 可被父 workflow 消费；List/Glob 返回结构化 details；统一 delegate 输入形状并改善错误；要求 child 只返回 JSON；builtin workflow 名称出现在 `/` completion，使用单一 `/workflow <name>` 语法。

Verification: exact contract tests for each tool/host/completion path; no summary parsing in workflow examples.

## Integration order

1. Slice 1
2. Slice 2
3. Slice 3 and Slice 4 (parallel branches, then integrate)
4. Slice 5
5. Final reviewer: full diff, source retirement scan, focused tests, dirty-worktree boundary, residual risks.

## Stop conditions

- Any slice introduces a second owner, compatibility fallback, arbitrary child cap, or card redesign: stop and fix before integration.
- If a test cannot prove a user-visible contract, replace it with a narrower behavior test.
- No final completion claim until all slices are reviewed and integrated.
