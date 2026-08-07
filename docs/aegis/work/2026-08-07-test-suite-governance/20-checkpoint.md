# 测试套件治理检查点

- 当前任务：执行测试治理实施计划（16 个提交批次），完成结构整理、语义精简、性能治理和三平台验证。
- 已完成：
  - Task 1：本机性能基线完整记录 —— 冷构建+发现 real 203.68s（3,496 个测试、86 个二进制目标）；热执行 real 179.50s；串行资源组 real 152.68s（占热执行 85%，为首要热点）。发现 5 个基线失败，全部为已提交生产变更（`58160c44` 工具绿色、`706d9021` blocking focus、`40541cd9` 折叠编辑统计）未同步的陈旧测试断言，非生产缺陷，已在证据文件中分类记录。
  - Task 2：`AGENTS.md` 测试规范提交 `0665ac4e`（spec 复核 APPROVE、质量复核 APPROVE）。
  - Task 3 结构轨：neo-ai 纯移动提交 `b193c3dc`（6 旧目标收敛为 3 固定领域入口；error.rs 拆 test_cases/；发现数 118+89 一致；3 条关键路径精确运行各 1 个通过；spec/质量复核均 APPROVE）。
- 进行中：Task 3 语义精简轨（提交 3）—— 表驱动合并、删除弱断言、5 秒固定等待改就绪信号；需处理 openai_responses.rs 1479 行超 1200 硬上限（拆分复核）。
- 证据：`target/test-governance/` 下 environment.txt、cold-target-path.txt、baseline-{cold,hot,serial}.time/.log、baseline-list.txt、neo-ai/pre-move-inventory.txt。
- 已知限制：`docs/aegis/INDEX.md` 为他人未提交改动，不得纳入任何提交；docs/aegis/work/ 文件虽被 ignore 但历史已跟踪，改动保持未提交状态。
- 下一步：Task 3 语义精简轨完成后按固定序列进入 Task 4（核心运行时与会话）。
- 漂移判断：范围内；未修改生产行为，未触碰他人文件。

## 进度更新（Task 3-7 完成）

- Task 3 neo-ai：`b193c3dc`（纯移动）+ `0a763b36`（语义精简：6 组合并、1 重写、openai_responses 行为前缀拆分）。207→194 测试。
- Task 4 核心运行时/会话：`ef328940`（拆分）+ `18fc582f`（夹具归位）+ `9f5e6abb`（fmt 修复）+ `f53a9d41`（主题权限矩阵合并）。246+89 组合并后 1373→1371。
- Task 5 核心工具：`b58d24dc`（8 目标收敛 + 17 处源码 test_cases 提取）+ `ddf016bd`（技能路径拒绝表驱动）。1371→1364。
- Task 6 核心工作流：`a293c2ee`（21 目标收敛 + runtime 三拆）+ `96c69b5e`（旧目标删除）+ `3acc7509`（dispatch 权限结果合并）。1364→1363。过程偏差：语义合并误入删除提交，已记录证据文件。
- Task 7 核心多代理：`58470f22`（4 目标收敛 12 子模块）+ `dd98ec6e`（零项记录）。1363 不变。
- 全部纯移动批次：发现数/函数名集合一致，关键守护精确运行各 1 通过；所有文件 ≤1200 行 / ≤30 测试。
- 已知慢测待 Task 10：`event_routing::subagent_cannot_force_call_hidden_parent_tools` ≈34.6s（字节级未动）。
