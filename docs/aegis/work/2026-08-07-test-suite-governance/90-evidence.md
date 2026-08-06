# 测试套件治理证据

- `neo-ai`：208 个测试定义，最大测试文件 3,381 行。
- `neo-agent-core`：约 1,382 个测试定义，最大测试文件 13,091 行。
- `neo-tui`：1,064 个测试定义，最大测试文件 4,068 行。
- `neo-agent`：865 个测试定义，最大测试文件 20,552 行。
- `.config/nextest.toml` 将六个完整测试目标放入单线程组，并包含 10,000 次同步写入和 12 MiB 输出压力测试的慢测覆盖。
- 旧远端样本执行 3,311 个测试约 154.5 秒；该数据只作为环境差异对照。
- 当前工作树已有无关生产文件修改，本轮文档不触碰这些文件。
- 设计说明固定四个 crate 的最终顶层测试目标、当前全部测试专用超限文件、源码侧超过 12 个测试的提取清单，以及逐组退役记录格式。
- 实施计划固定 16 个提交批次，纯移动与语义精简分离，工作流与多代理分离。
- 本机冷阶段使用独立 `CARGO_TARGET_DIR`；最终使用另一个全新目录。两个阶段均使用默认 Nextest 配置。
- 精确验证要求先用完整测试路径发现，再以 `cargo test --exact` 运行并记录实际运行数。
- 六份 ADR 均有现行引用，已补格式而非删除；两份编辑回读旧文档由 2026-07-28 新简报明确取代后删除。
- `python /Users/chenyuanhao/.codex/aegis/scripts/aegis-workspace.py check --root /Users/chenyuanhao/Workspace/neo` 已通过。

证据性质：只读静态分析和历史远端数据，不是当前本机完整测试通过证明。


## EvidenceBundleDraft

- Artifact key: static-test-inventory
- Type: static-analysis
- Source: CodeGraph, rg, wc, Cargo metadata, four crate audits
- Summary: Current worktree has about 3519 test definitions and 83 top-level integration targets; no local full-suite run was performed.
- Verifier: root-agent

## Aegis 文档清理证据

- 删除：`docs/aegis/specs/2026-07-25-edit-mismatch-readback-brief.md`、`docs/aegis/plans/2026-07-25-edit-mismatch-readback.md`。
- 删除：未跟踪且无引用、无完成证据的 `docs/aegis/work/2026-07-30-remote-ci-clippy-cleanup/`。
- 保留并修复：ADR-0005、0007、0008、0009、0011、0012。
- 保留并登记：有完成证据、恢复状态或现行引用的旧工作记录。
- 结构结果：全仓 Aegis 索引、ADR 和结构化草稿校验通过。
