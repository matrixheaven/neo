# Thinking and Message Presentation - Checkpoint

- Task ID: 2026-08-03-thinking-and-message-presentation
- Current todo: 主文档和工作记录已写入，等待自审、索引和提交
- Active slice: documentation self-review and scoped commit
- Blocked on: none
- Next step: 检查占位符、索引覆盖、结构校验和精确暂存；不修改 Rust 源码

## Completed Todos

- 完成设计规格：摘要正文保留、完整思考显示、过程回复和最终回答分离。
- 完成实施计划：文件边界、任务顺序、测试边界、兼容和退休规则明确。
- 完成交接说明：意图锁、范围锁、基线锁、禁止事项和恢复路径明确。
- 创建 Aegis 工作记录目录。

## DriftCheckDraft

- Scope status: documentation-only; no Rust source or unrelated user file changed
- Compatibility status: existing `ThinkingKind`, title replacement, unknown legacy behavior, session append-only rules, and Delegate-family cards are preserved
- Retirement status: the title-only summary projection is explicitly retired by the future implementation; no source retirement occurred in this slice
- New risk signals: provider message-phase availability is endpoint-dependent and must remain `Unknown` when absent
- Advisory decision: continue to documentation validation

## ResumeStateHint

Resume by reading the spec, plan, and handoff in `docs/aegis/`, then checking
the current worktree. Do not reopen the design or touch unrelated dirty files.
The next source slice starts at plan Task 1 and must verify the exact provider
phase field before editing the normalized event path.

## DriftCheckDraft

- Scope status: documentation-only; source implementation pending
- Compatibility status: existing ThinkingKind, legacy Unknown behavior, append-only session records, and Delegate-family cards preserved
- Retirement status: title-only summary projection explicitly retired in future implementation; no source retirement in this slice
- New risk signals:
- provider message-phase availability is endpoint-dependent; emit Unknown when absent
- Advisory decision: continue
