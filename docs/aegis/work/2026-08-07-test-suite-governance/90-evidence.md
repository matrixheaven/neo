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

## Task 1 本机性能基线（HEAD a68171ca，macOS arm64 10 核 24 GiB）

- 冷构建+发现（全新 CARGO_TARGET_DIR）：real 203.68s（user 262.57s / sys 46.21s）。
- 热执行（同编译目录，默认 nextest 配置）：real 179.50s，共 3,496 个测试、86 个二进制目标。
- 发现 5 个基线失败（在隔离运行中均确定性复现，非抖动）：
  1. `neo-agent::bin/neo modes::interactive::tests::approval_transcript_holds_every_request_and_focuses_earliest`
  2. `neo-tui transcript::entry::tests::edit_approval_prompt_follows_global_expansion`
  3. `neo-tui::multi_agent_transcript delegate_card_marks_unfinished_tool_as_using_with_neutral_marker`
  4. `neo-tui::transcript_pane transcript_pane_edit_approval_follows_global_expansion`
  5. `neo-tui::transcript_pane transcript_pane_renders_only_earliest_pending_approval`
- 分类：全部为陈旧测试断言，由已提交的有意生产行为变更引起，非生产缺陷：
  - `58160c44 fix: color active delegate tools green` 将 Ongoing 工具色从 text_primary 改为 status_ok，更新了 `delegate_family_tool_activity_uses_theme_and_collapsed_file_hint`，漏改 `delegate_card_marks_unfinished_tool_as_using_with_neutral_marker`。
  - `706d9021 fix(tui): keep blocking transcript entries visible` 引入 blocking-focus 可视窗口约束（窗口限于最早阻塞条目卡片），未同步 `transcript_pane_renders_only_earliest_pending_approval` 与 `approval_transcript_holds_every_request_and_focuses_earliest`。
  - `40541cd9 fix: show collapsed edit file stats` 折叠编辑卡片改为逐文件统计行，未同步 `transcript_pane_edit_approval_follows_global_expansion` 与 `edit_approval_prompt_follows_global_expansion`。
- 远端：origin/main 最后 CI 运行（d466e0f9，2026-08-06）失败于 lint 门槛，测试套件状态未经远端验证。
- 串行资源组基线：待 baseline-serial.time。
- 串行资源组基线（6 个二进制，max-threads=1）：real 152.68s（user 9.01s / sys 9.54s），422 个测试，退出码 0。
- 串行组内最慢成员：`complete_agent_output_survives_preview_queue_pressure` 20.6s（12 MiB 资源守护，保留）、`runtime_marks_model_background_bash_as_backgrounded_shell_event` 18.2s、`runtime_clamps_out_of_range_bash_timeout_and_returns_notice` 17.7s、`agent_event_stream_cancels_only_when_abandoned` 16.4s、`terminal_capture_survives_ring_overflow` 6.0s。
- 关键结论：热执行 179.50s 中串行组占 152.68s（85%），整二进制串行是首要性能热点；热执行本身已远低于用户报告的“约一小时”（用户口径含冷编译或旧状态），本机分段基线以本次测量为准。

## Task 3 语义精简轨 §5.10 退役记录（neo-ai）

| 删除或合并的测试 | 保留的主要守护 | 两者共同捕获的生产故障 | 证明方式 | 精确命令 | 实际运行数 |
|---|---|---|---|---|---:|
| `parse_retry_after_seconds`、`parse_retry_after_past_http_date_returns_zero`、`parse_retry_after_invalid_returns_none` | `parse_retry_after_maps_delta_seconds_and_http_dates`（表驱动合并，5 个具名案例） | `parse_retry_after` 的整数秒、HTTP 日期、非法输入分支 | 合并；案例逐个命名断言，同一生产函数同一断言形状 | `cargo test --package neo-ai --lib -- providers::common::error::retry_after::parse_retry_after_maps_delta_seconds_and_http_dates --exact` | 1（5 案例） |
| `http_status_401_maps_to_auth`、`http_status_408_maps_to_retryable_transport`、`http_status_413_with_context_overflow_maps_to_context_overflow`、`http_status_413_without_context_pattern_maps_to_protocol`、`http_status_429_maps_to_rate_limit`、`http_status_503_maps_to_server` | `http_status_codes_classify_into_typed_ai_errors`（表驱动合并，6 个具名案例） | `into_ai_error` 的 HttpStatus 状态 match 分支（401 auth / 408 transport / 413 context-overflow 与 protocol / 429 rate-limit / 503 server） | 合并；案例逐个命名断言 | `cargo test --package neo-ai --lib -- providers::common::error::classification::http_status_codes_classify_into_typed_ai_errors --exact` | 1（6 案例） |
| `test_infer_api_type_anthropic`、`test_infer_api_type_openai`、`test_infer_api_type_explicit` | `infer_api_type_maps_npm_package_and_explicit_type`（表驱动合并，3 个具名案例） | `infer_api_type` 的 explicit_type 与 npm 包名匹配分支 | 合并；案例逐个命名断言 | `cargo test --package neo-ai --lib -- catalog::tests::infer_api_type_maps_npm_package_and_explicit_type --exact` | 1（3 案例） |
| `reasoning_effort_serializes_as_stable_snake_case_values`、`reasoning_effort_serializes_max_and_stable_names`、`reasoning_effort_preserves_custom_provider_value` | `reasoning_effort_names_round_trip_through_serialization`（表驱动合并，5 个具名案例） | `ReasoningEffort` 透明序列化/反序列化名称映射（已知名称、大小写保留、自定义值透传） | 合并；案例逐个命名断言 | `cargo test --package neo-ai --test environment_behavior -- request_options::reasoning_effort_names_round_trip_through_serialization --exact --nocapture` | 1（5 案例） |
| `model_capabilities_default_to_text_chat_streaming`、`model_capabilities_helpers_describe_common_shapes` | `model_capabilities_shapes_cover_default_and_helpers`（表驱动合并，3 个具名案例） | `ModelCapabilities` 构造器（default/chat、tool_chat、embedding）字段形状 | 合并；案例逐个命名断言 | `cargo test --package neo-ai --test model_resolution_behavior -- model_registry::model_capabilities_shapes_cover_default_and_helpers --exact --nocapture` | 1（3 案例） |
| `openai_tool_calls_finish_reason_with_structured_calls_remains_tool_use` | `openai_compatible_client_finishes_tool_call_on_tool_calls_finish_reason_without_done` | finish_reason=`tool_calls` 时 ToolCallEnd 终止事件与 `StopReason::ToolUse`（`ParseState::finish_events` 分支）；两测试同走 `ParseState::ingest`→`apply_finish_reason`/`ingest_delta`→`finish_events` 调用链，仅流帧分片不同，生产代码无帧数分支 | 调用路径 + 临时故障注入：注释 `finish_events` 中 `last_stop_reason = StopReason::ToolUse` 后保留测试失败，随后撤销注入 | `cargo test --package neo-ai --test provider_protocol_behavior -- openai_compatible::openai_compatible_client_finishes_tool_call_on_tool_calls_finish_reason_without_done --exact --nocapture` | 1 |
| 重写：`start_unfinished_chunked_error` 固定 5 秒 `thread::sleep` | `provider_error_body_stops_reading_at_limit`（google.rs，断言不变） | 客户端在错误体达到读取上限时停止读取；未完成 chunk 连接在测试释放前保持打开（守旧行为：休眠保活 5 秒） | 重写为 release 通道就绪信号：服务器写完未完成 chunk 后阻塞在 `recv()`，测试断言完成后 `server.release()` 关闭连接，无固定等待 | `cargo test --package neo-ai --test provider_protocol_behavior -- google::provider_error_body_stops_reading_at_limit --exact --nocapture` | 1 |

结构拆分（非退役）：`openai_responses.rs` 1479 行超 1200 硬上限，按行为前缀拆为 `openai_responses.rs`（核心请求/图像）、`openai_responses_reasoning.rs`（推理选择与摘要流）、`openai_responses_errors.rs`（流错误分类），测试发现集合不变（29 个，`cargo nextest list` 核对）。

## Task 4 语义精简轨 §5.10 退役记录（neo-agent-core，runtime/session 批次）

| 删除或合并的测试 | 保留的主要守护 | 两者共同捕获的生产故障 | 证明方式 | 精确命令 | 实际运行数 |
|---|---|---|---|---|---:|
| `permissions_mode::theme_draft_preview_runs_without_any_approval_in_ask_mode`、`permissions_mode::theme_draft_save_executes_directly_in_auto_mode`、`permissions_mode::theme_draft_save_is_denied_in_plan_mode_while_preview_runs` | `permissions_mode::theme_draft_permission_matrix_covers_ask_auto_and_plan_paths`（表驱动合并，4 个具名案例）+ `permissions_mode::theme_draft_save_requires_typed_theme_save_approval_with_no_session_grant`（完整运行链路） | `theme_draft_permission_preparation`（src/runtime/permission.rs）的 action 分支：preview 全模式直跑、save 在 plan 模式 Deny（"blocked by plan mode"）、save 在 Auto 模式直跑；三测试同走 `run_theme_draft_turn` 探针机制与同一断言形状，仅 (mode, action) 输入变体 | 合并（案例逐个命名断言同一分支）+ 临时故障注入：把 plan-active save 的 `Deny` 改为 `Run` 后矩阵 plan 案例失败，随后撤销注入 | `cargo test --package neo-agent-core --test runtime_behavior -- permissions_mode::theme_draft_permission_matrix_covers_ask_auto_and_plan_paths --exact --nocapture` | 1（4 案例） |

本批次其余检查结论（无删除）：主题保存权限的注册表边界测试 `tool_permissions::theme_draft_never_accepts_file_write_in_place_of_tool_access`（file_write/shell/file_read 授予不得替代 `tool` 授予，ToolRegistry 边界分支）与运行时审批链路（Ask 审批请求、session 级授权禁止）分支不同，保留；session_behavior 全部持久化往返测试（state.rs 三个 store 读写、jsonl_append.rs 全部 append/read/replay/legacy 用例、tree.rs 全部 metadata/fork/rename 用例）均经真实文件读写，无纯派生类型往返，保留；未发现仅断言文本非空的弱断言测试（全部文本断言为精确内容 `contains`/`assert_eq!`）；`JsonlSessionWriter` 锁为 sidecar 文件锁，`cancelled_session_lock_wait_leaves_no_waiter` 通过显式 `drop(writer)` 释放 seed 锁后重开，无持锁自等待。

## Task 5 语义精简轨 §5.10 退役记录（neo-agent-core，工具批次）

| 删除或合并的测试 | 保留的主要守护 | 两者共同捕获的生产故障 | 证明方式 | 精确命令 | 实际运行数 |
|---|---|---|---|---|---:|
| `create_skill_rejects_resource_path_escape`、`create_skill_rejects_resource_outside_canonical_dirs`、`create_skill_rejects_absolute_resource_path`、`create_skill_rejects_skill_md_as_resource`、`create_skill_rejects_windows_hostile_resource_path_components` | `create_skill_rejects_invalid_resource_paths_without_side_effects`（表驱动合并，8 个具名案例） | `validate_resource_path`（src/tools/skills_manager.rs）的资源路径拒绝分支：父目录穿越（unsafe component）、绝对路径（先命中空组件检查，早于 is_absolute）、非 references/scripts/assets 前缀、SKILL.md 目标、Windows 非法字符/尾点/尾空格；全部经 `invalid_resource_path` 包装，在任何文件写入前失败 | 合并；案例逐个命名断言同一生产函数同一断言形状（error 含 "invalid resource path" + 无副作用），注入移除 `..` 组件拒绝后 parent_traversal 案例失败（工具继续执行并写出文件），随后撤销 | `cargo test --package neo-agent-core --lib -- tools::skills_manager::create_skill_reject_paths::create_skill_rejects_invalid_resource_paths_without_side_effects --exact` | 1（8 案例） |
| `create_skill_rejects_symlinked_skill_directory`、`create_skill_rejects_symlinked_skills_root`、`create_skill_rejects_symlinked_backup_parent` | `create_skill_rejects_symlinked_directory_in_skill_or_backup_paths`（表驱动合并，3 个具名案例） | `create_missing_directories_recording`（src/session/atomic_file.rs）的 reparse 分支 "refusing symlinked directory"：技能根、技能目录、backup 根任一层为符号链接时，预检在任何写入前失败（backup 案例另断言原 SKILL.md 保留、未跟随写入 outside） | 合并（案例逐个命名，backup 案例保留副作用断言）+ 调用路径：execute → `ensure_safe_directory_tree` ×3（skills 根 / 技能目录 / backup 根）→ 同一 reparse 分支；注入把 reparse 条件改为恒 false 后测试失败（skill_directory_symlink 案例错误变为 "refusing to create files under non-directory ancestor"，不再标识符号链接），随后撤销 | `cargo test --package neo-agent-core --lib -- tools::skills_manager::create_skill_reject_symlinks::create_skill_rejects_symlinked_directory_in_skill_or_backup_paths --exact` | 1（3 案例） |
| `create_skill_rejects_symlinked_skill_file_without_following_it`、`create_skill_rejects_dangling_symlinked_skill_file` | `create_skill_rejects_symlinked_or_dangling_skill_file`（表驱动合并，2 个具名案例） | `reject_reparse_or_symlink_if_present`（src/session/atomic_file.rs）的 "refusing symlinked file" 分支：SKILL.md 为（悬空或指向外部文件的）符号链接时拒绝，覆盖写入前失败、不跟随外部目标 | 合并（案例逐个命名；非悬空案例保留外部文件内容不变断言）+ 注入把 reparse 条件改为恒 false 后测试失败（symlinked_skill_file 案例错误变为写入路径上的 "refusing to copy symlinked skill artifact"，预检拒绝契约被破坏），随后撤销 | `cargo test --package neo-agent-core --lib -- tools::skills_manager::create_skill_reject_symlinks::create_skill_rejects_symlinked_or_dangling_skill_file --exact` | 1（2 案例） |

本批次其余检查结论（无删除）：MCP 状态字符串仅有 `needs_auth_status_has_stable_string` 一个 `McpServerStatus::as_str` 测试，无同分支变体可合并；`auth_diagnostics.rs` 两个 `diagnostic_hint` 测试分别走 needs_auth+Http 提示分支与 protocol 消息不选提示分支，断言不同分支，保留；未发现提示分类（prompt classification）同分支重复测试；`skill_descriptions.rs` 与 `task_descriptions.rs` 的 `tool_descriptions_are_non_empty` 虽为弱断言但不在本批三个允许组（技能路径拒绝/MCP 状态字符串/提示分类）内，保留待协调者裁决；`create_skill_backup.rs` 的符号链接资源目标测试走 `preflight_resource_file` 调用点（备份域文件内），与 SKILL.md 检查调用点不同，保留。

## Task 6 语义精简轨 §5.10 退役记录（neo-agent-core，workflow 批次）

| 删除或合并的测试 | 保留的主要守护 | 两者共同捕获的生产故障 | 证明方式 | 精确命令 | 实际运行数 |
|---|---|---|---|---|---:|
| `dispatch::cancelled_permission_maps_to_cancelled_workflow_outcome`、`dispatch::required_permission_maps_to_denied_workflow_outcome` | `dispatch::permission_decisions_map_to_typed_workflow_outcomes`（表驱动合并，2 个具名案例：cancelled / required） | `tool_result_to_outcome`（src/runtime/workflow_dispatch.rs）的 `fallback_status` 对 `PermissionTerminalDecision` 的 match：`Cancelled` → `WorkflowOutcomeStatus::Cancelled`、`Required` → `Denied`，配合 `permission_error`（src/runtime/permission.rs）的 `details["decision"]`（"cancelled"/"required"）与 `side_effect_occurred: false` | 调用路径：run_one → `resolve_approval`（handler 返回 Cancelled 与无 handler 两个输入）→ `AppliedApproval::Terminal { permission_decision: Some(Cancelled/Required) }` → `batch_to_outcome` → 同一 `tool_result_to_outcome` fallback_status match；两测试断言形状完全相同（status + details["decision"]），仅输入枚举变体不同；合并后每个案例具名，失败可直接定位 | `cargo test --package neo-agent-core --test workflow_behavior -- dispatch::permission_decisions_map_to_typed_workflow_outcomes --exact --nocapture` | 1（2 案例） |

本批次其余检查结论（无删除）：dispatch.rs 委托三测试（failed/cancelled/interrupted）断言形状不同（usage 字段、child_refs 数量、reason 有无），`background_or_running`/`malformed`/`expected_child_kind`/`non_child_spoof`/swarm 组各走 canonical_child_outcome 不同分支或不同工具名，全部保留；launch.rs 的 `invalid_workflow_input_never_opens_approval`、`source_and_run_metadata_limits_return_typed_invalid_input`、`compile_schema_and_storage_failures_create_no_run` 已是内部表驱动，`invalid_saved_run_args_fail_before_approval_opens`（run_saved 路径）、`invalid_preflight_creates_no_run`（意图哈希预检）、`ask_launch/ask_revise/ask_save`（审批链路）分支不同，保留；lua.rs 的 denied/unknown neo.tool 错误码不同（tool_not_workflow_eligible vs unknown_tool），走 registry 查找与策略拒绝两个不同分支，保留；instruction/memory 资源限制为两个不同强制点，保留；`lua_return_conversion_*` 两测试一个验证合法标记保留、一个验证非法值拒绝表，断言形状不同，保留；journal.rs、schema.rs、admission.rs、registry.rs、tool_policy.rs、user_input.rs、builtins.rs 全部测试为已表驱动或不同分支/不同行为，无同分支同断言输入枚举变体可合并；runtime_lifecycle/effects/recovery 三文件仅保留两主守护不动，其余测试各自独立行为，无合并。

## 过程偏差记录（Task 6）

- 提交 `96c69b5e`（本应只含旧 workflow 顶层目标删除）意外包含 `workflow_behavior/dispatch.rs` 的语义合并（104 行变更，permission_decisions 表驱动）。原因：协调者使用 `git add -u crates/neo-agent-core/tests/` 暂存删除时把同目录的语义修改一并暂存。
- 未改写历史（政策禁止 amend/rebase）。终态正确：合并测试 `dispatch::permission_decisions_map_to_typed_workflow_outcomes` 存在于 HEAD 并精确通过（1 passed），旧测试已删，两主守护独立保留；§5.10 记录在 `3acc7509` 提交的证据文件中。
- 教训：后续批次删除与修改并存时，分别用精确路径暂存。

## Task 7 语义精简轨 §5.10 退役记录（neo-agent-core，多代理批次）

零合并组。逐个审查 `crates/neo-agent-core/tests/multi_agent_behavior/` 全部 11 个文件、111 个测试（`cargo nextest list -p neo-agent-core --test multi_agent_behavior` 确认发现数 111），未发现符合“纯输入矩阵展示名”合并条件（同一格式化分支 + 同一断言形状 + 仅展示名/状态标签输入变体）的测试组，因此无退役行。候选组逐一检查及保留原因：

- **展示名类**：`lifecycle::display_name_pool_is_deterministic`（默认序列前缀）与 `display_name_pool_combines_names_after_default_names`（耗尽默认名后拼接）是 `DisplayNamePool::next_name` 的两个不同分支（未耗尽 vs 越界拼接），非同一分支输入变体；`roles::built_in_profiles_have_expected_labels_and_tool_policies` 与 `delegate_and_swarm_schemas_surface_role_guide` 已各自合并为单测试内逐角色断言；`lifecycle::foreground_delegate_lifecycle_records_running_and_completed_state` 的 display_name 断言仅是生命周期记录的一行，主行为不同。
- **进度标签类**：`progress::retry_activity_stays_inside_child_snapshot` 是唯一覆盖 "Reconnecting N/M" 格式化分支的测试，无同分支变体；`progress::child_text_delta_accumulation_preserves_repeated_fragments` 与 `child_text_and_thinking_deltas_accumulate_into_live_activity` 断言形状不同（去重语义 vs 双流累积）；`child_shell_activity_keeps_command_and_output_with_or_without_queue` 已是内部表驱动。
- **已表驱动，无需合并**：`usage::delegate_tools_reject_empty_tasks_bad_context_and_zero_concurrency`、`progress::child_shell_activity_keeps_command_and_output_with_or_without_queue`、`background::list_delegates_treats_blank_cursor_as_first_page_but_rejects_zero`、`event_routing::resumed_child_turn_fails_when_agent_wire_is_missing_or_corrupt` 均为单测试内部案例循环。
- **同工具但不同分支，保留**：`lifecycle::delegate_resume_rejects_role_override`（"role must be omitted when resume is set"）与 `delegate_resume_rejects_swarm_id`（"resume must be an agent_id"）是两个不同校验分支；`background_messaging::message_delegate_unknown_id_errors_without_creating_mailbox`（id 查找失败）、`message_delegate_background_agent_without_live_steer_returns_resume_hint`（已注册未 live）与 `message_delegate_rejects_completed_agent_with_resume_hint`（终态拒绝）分支各异；`scheduler::swarm_scheduler_reduces_concurrency_on_rate_limit` 与 `swarm_scheduler_recover_restores_concurrency` 是 rate-limit 递减与 recovery 递增两个状态转换；`progress_estimate_*` 三测试覆盖 active/全终态/时长增长三个不同计算分支；`background::background_manager_lists_delegate_tasks` 与 `background_manager_lists_swarm_tasks` 走 delegate 与 swarm 两个登记路径；`lifecycle::list_delegates_defaults_to_meta_only_rows_with_title` 与 `list_delegates_includes_requested_summary_in_model_content` 是 meta-only 与 summary 两个投影分支。
- **受保护域，全部原样保留**：持久化恢复（`lifecycle::replayed_*`、`resumed_child_turn_replays_prior_messages_from_agent_wire`、`event_routing::child_run_appends_events_to_agent_wire`、`failed_child_run_discards_partial_model_attempt_from_agent_wire`、`background::restored_running_delegate_is_reported_lost_with_resume_hint` 等）、晚到事件（`progress::older_terminal_progress_cannot_clear_a_newer_outcome`、`background_task_stop::task_stop_cancels_delegate_runtime_and_completion_cannot_overwrite_cancelled` 等）、原子终态（`lifecycle::delegate_swarm_invalid_late_resume_is_atomic`、`cancel_swarm_preserves_completed_canonical_child_when_swarm_snapshot_is_stale`、`background_worker_panics_terminalize_delegate_and_swarm`）、取消（`cancellation.rs` 4 个、`background_interrupt.rs` 4 个、`background_task_stop.rs` 6 个全部保留）。已知慢测 `event_routing::subagent_cannot_force_call_hidden_parent_tools` 字节级未动（`git diff HEAD -- crates/neo-agent-core/tests/` 为空）。

**验证证据**（本批次零代码变更，运行计划 Task 7 精确验证命令确认基线无破坏）：

```bash
cargo test --package neo-agent-core --test multi_agent_behavior -- progress::older_terminal_progress_cannot_clear_a_newer_outcome --exact --nocapture   # 1 passed
cargo test --package neo-agent-core --test multi_agent_behavior -- lifecycle::resumed_child_turn_replays_prior_messages_from_agent_wire --exact --nocapture   # 1 passed
```

前后测试数：111 → 111（`cargo nextest list` 发现数一致）；全部文件 ≤1200 行 / ≤30 测试（最大 lifecycle.rs 1158 行 / 26 测试）。

## Task 8 语义精简轨 §5.10 退役记录（neo-tui）

### 陈旧断言重写（§6 重写，生产行为已提交，仅同步测试断言）

| 删除或合并的测试 | 保留的主要守护 | 两者共同捕获的生产故障 | 证明方式 | 精确命令 | 实际运行数 |
|---|---|---|---|---|---:|
| 重写：`transcript_behavior::pane_approval::transcript_pane_renders_only_earliest_pending_approval` → `earliest_pending_approval_owns_visible_window_until_resolved` | 同一测试重写（断言对齐 `706d9021` 阻塞焦点契约） | `706d9021 fix(tui): keep blocking transcript entries visible`：`DocumentLayout::set_blocking_focus`/`visible_row_range` 把可视窗口约束到最早未决阻塞条目的卡片；后续审批卡片留在文档（`total_rows` 计入）但不在可视切片内，直到解析后焦点前进 | 旧断言在 HEAD 确定性失败（`first.is_some() && ...` 于 pane_approval.rs:518），重写后精确通过 | `cargo test --package neo-tui --test transcript_behavior -- pane_approval::earliest_pending_approval_owns_visible_window_until_resolved --exact --nocapture` | 1 |
| 重写：`transcript_behavior::pane_approval::transcript_pane_edit_approval_follows_global_expansion` | 同一测试重写（断言对齐 `40541cd9` 折叠统计行） | `40541cd9 fix: show collapsed edit file stats`：`edit_tool_presentation::render_prepared_or_committed` 折叠时经 `render_omission` 输出 "M 路径 +n -m" 逐文件统计行并隐藏折叠文件的 diff，展开后才显示完整 diff 细节 | 旧断言在 HEAD 确定性失败（`!collapsed.contains("verified_2.rs")` 于 pane_approval.rs:354），重写后精确通过 | `cargo test --package neo-tui --test transcript_behavior -- pane_approval::transcript_pane_edit_approval_follows_global_expansion --exact --nocapture` | 1 |
| 重写：`agent_transcript_behavior::delegate_cards::delegate_card_marks_unfinished_tool_as_using_with_neutral_marker` → `delegate_card_marks_ongoing_tool_as_using_with_active_color` | 同一测试重写（断言对齐 `58160c44`）；兄弟测试 `delegate_family_tool_activity_uses_theme_and_collapsed_file_hint` 覆盖 Done/Failed 动词色，本测试独立覆盖 Ongoing 分支，非重复，保留独立 | `58160c44 fix: color active delegate tools green`：`child_activity::child_tool_phase_style` 的 `Ongoing => theme.status_ok`（"Using" 标记从 text_primary 改为 status_ok） | 旧断言在 HEAD 确定性失败（期望 text_primary=230,230,230，实际 status_ok=1,220,120），重写后精确通过 | `cargo test --package neo-tui --test agent_transcript_behavior -- delegate_cards::delegate_card_marks_ongoing_tool_as_using_with_active_color --exact --nocapture` | 1 |
| 重写：`transcript::entry::render::edit_approval_prompt_follows_global_expansion`（lib 内联） | 同一测试重写（断言对齐 `40541cd9` 折叠统计行） | 同上：折叠编辑审批卡片显示 "M src/file2.rs +1 -1" 统计行与 "diff details hidden" 省略提示并保留 ╭/╰ 框，展开后显示 `12 - old2`/`12 + new2` 完整 diff | 旧断言在 HEAD 确定性失败（render.rs:434 `contains("files · 1 replacements")`），重写后精确通过 | `cargo test --package neo-tui --lib -- transcript::entry::render::edit_approval_prompt_follows_global_expansion --exact --nocapture` | 1 |

### Markdown 代码框与无断言测试（精简轨第 1、2 项）

| 删除或合并的测试 | 保留的主要守护 | 两者共同捕获的生产故障 | 证明方式 | 精确命令 | 实际运行数 |
|---|---|---|---|---|---:|
| `rendering_behavior::markdown_rendering::fenced_bash_block_does_not_leak_thinking_or_prompt_chrome`、`rendering_behavior::markdown_rendering::diff_code_block_colors_add_remove` | `rendering_behavior::markdown_rendering::code_block_has_rounded_box_borders_and_language_header`（合并后同时覆盖真实文档围栏不泄漏 chrome、围栏反引号剥离、diff 围栏内容行；集成层只保留一个完整代码框展示案例） | `src/markdown.rs` 代码框渲染：`finish_code_block`/`emit_code_content_line` 的边框与语言头、`emit_diff_box_line` 的 diff 围栏内容分支、围栏反引号必须剥离（`code_block_no_fence_backticks` 内联层同分支）；真实文档中的围栏不得混入周边 chrome/陈旧思考文本 | 合并：三个测试渲染同一生产入口 `render_markdown`，保留测试吸收全部断言（3 个输入渲染），内联层 `src/markdown/test_cases/code_block.rs` 已有解析/宽度守护（`code_block_no_fence_backticks`、`code_block_width_is_consistent_and_within_bounds`） | `cargo test --package neo-tui --test rendering_behavior -- markdown_rendering::code_block_has_rounded_box_borders_and_language_header --exact --nocapture` | 1 |
| `markdown::code_block::code_block_honors_min_width`（无断言） | `markdown::code_block::code_block_width_is_consistent_and_within_bounds`（并入 min-width 段：4 列渲染不 panic 且产出非空行；20–80 列保留等宽与宽度上界断言） | 极小宽度（4 列）下 `render_markdown` 的代码框宽度下限回退分支（`hard_wrap_line`/`clip_plain_to_width` 下限钳制；生产在 4 列时回退为不换行裸行，不保证行宽上界——旧测试注释明示"just ensure no panic"） | 合并：无断言测试并入保留测试的 min-width 段（保留测试现在带断言覆盖同一不 panic 故障）；20–80 列的等宽/上界断言不变 | `cargo test --package neo-tui --lib -- markdown::code_block::code_block_width_is_consistent_and_within_bounds --exact` | 1 |

### 输入选择真实等待重写（精简轨第 4 项）

| 删除或合并的测试 | 保留的主要守护 | 两者共同捕获的生产故障 | 证明方式 | 精确命令 | 实际运行数 |
|---|---|---|---|---|---:|
| 重写：`transcript_behavior::selection::click_never_selects_but_long_press_activates_after_delay` → `quick_click_never_selects_and_held_press_stays_tentative` | 同一测试重写（删除 350ms `thread::sleep(LONG_PRESS_DELAY+50ms)` 与长按激活段）+ 单元层 `transcript::selection::long_press_activates_after_delay_without_movement`（显式 `now + LONG_PRESS_DELAY ± 10ms` 参数） | 长按延迟激活分支：`DocumentSelection::tick` 在按住超过 `LONG_PRESS_DELAY` 后激活（单元层显式时间守护）；快捷点击绝不选择、按住未过延迟不产生高亮（集成层保留的确定性断言）；pane 接线 `mouse_press → selection.press(..., Instant::now())` | 重写：集成层移除真实等待（§6：优先暂停时间/确定性替代），延迟分支由单元层显式时间测试守护；保留断言在重写后精确通过 | `cargo test --package neo-tui --test transcript_behavior -- selection::quick_click_never_selects_and_held_press_stays_tentative --exact --nocapture` | 1 |

### 表驱动合并（精简轨第 5 项）

| 删除或合并的测试 | 保留的主要守护 | 两者共同捕获的生产故障 | 证明方式 | 精确命令 | 实际运行数 |
|---|---|---|---|---|---:|
| `tool_card_behavior::cards::exit_plan_mode_header_shows_approved_with_label`、`exit_plan_mode_header_shows_approved_without_label`、`exit_plan_mode_header_shows_rejected_on_failure` | `tool_card_behavior::cards::exit_plan_mode_header_covers_approved_label_and_rejected`（表驱动合并，3 个具名案例） | `tool_renderers::exit_plan_mode_header_spans` 的状态分支：Succeeded 且带 `plan_selected_label` → "Approved: {label}"、Succeeded 无标签 → "Approved"、Failed → "Rejected"（并断言不出现泛化工具名/反向状态词） | 合并：三测试调用同一生产函数、同一断言形状（"Current plan" + 状态词），仅 (status, details) 输入变体 | `cargo test --package neo-tui --test tool_card_behavior -- cards::exit_plan_mode_header_covers_approved_label_and_rejected --exact --nocapture` | 1（3 案例） |
| `transcript_behavior::pane::mcp_startup_status_updates_pending_spinner_to_green_connected_row`、`mcp_startup_status_updates_pending_spinner_to_interrupted_row`、`mcp_startup_status_updates_pending_spinner_to_red_failed_row` | `transcript_behavior::pane::mcp_startup_status_transitions_render_terminal_phase_rows`（表驱动合并，3 个具名案例） | `entry::McpStartupStatus` 的相位渲染分支：Connecting → Connected（status_ok 色、`connected · {n} tools`）、Cancelled（"startup interrupted" 且旧 "connecting..." 行消失）、Failed（status_error 色、错误消息），全部经 `upsert_mcp_startup_status` 单条目替换（条目数恒 1） | 合并：三测试同一 upsert→渲染链路、同一断言形状（先行 Connecting 断言 + 终态文本/颜色断言），仅终态相位与颜色输入变体；案例逐个命名 | `cargo test --package neo-tui --test transcript_behavior -- pane::mcp_startup_status_transitions_render_terminal_phase_rows --exact --nocapture` | 1（3 案例） |
| `app_behavior::theme_manager::wide_layout_shows_list_and_preview_side_by_side`、`medium_layout_stacks_list_over_preview`、`narrow_layout_renders_one_focused_panel`、`very_short_layout_keeps_header_and_essential_action` | `app_behavior::theme_manager::theme_manager_breakpoints_map_width_to_layout`（表驱动合并，5 个具名案例） | `render_focused_full_screen_overlay` 的布局断点分支：宽（≥~100 双栏同框）、中（80 列表在上预览在下）、窄（60 单面板 + Tab 焦点路由切换）、极矮（高 5 保留标题/焦点/Enter apply） | 合并：四测试重复同一 `open_manager()` 夹具验证同一 宽度→布局 映射表，断言形状相同（渲染文本标记）；窄案例保留 Tab 焦点路由（集成层职责），极矮案例保留三宽度循环 | `cargo test --package neo-tui --test app_behavior -- theme_manager::theme_manager_breakpoints_map_width_to_layout --exact --nocapture` | 1（5 案例） |
| `input::test_cases::feed::feed_bytes_cjk_character_produces_insert`、`feed_bytes_space_produces_insert`、`feed_bytes_fullwidth_symbol_produces_insert` | `input::test_cases::feed::feed_bytes_printable_chars_map_to_insert`（表驱动合并，3 个具名案例） | `InputParser::feed_bytes` 的可打印字符映射分支（CJK 字符、空格、全角符号 → `InputEvent::Insert`）；`feed_bytes_split_cjk_character_waits_for_complete_utf8` 为 UTF-8 分片缓冲分支，独立保留 | 合并：三测试同一生产入口、同一断言形状（len==1 + `Insert(ch)`），仅输入字节变体 | `cargo test --package neo-tui --lib -- input::feed::feed_bytes_printable_chars_map_to_insert --exact` | 1（3 案例） |

### 本批次其余检查结论（无删除）

- **主题无效条目分层**（精简轨第 3 项）：内联层 `src/shell/test_cases/manager.rs::invalid_entry_cannot_be_applied_or_defaulted` 保留状态转换（Submit/D 均不产生 action 且状态为 error），集成层 `app_behavior/theme_manager.rs::invalid_entry_cannot_be_applied_or_defaulted` 只保留焦点路由（j 选择 broken.json）与可见错误（overlay 渲染 "invalid"），两层已按 §7 分层，无重复案例集，全部保留。
- **精确快照**（精简轨第 6 项）：审查全部 `assert_eq!(..., vec![...])` 展示断言，仅终端转义/图片协议占位（`terminal_behavior/images.rs`）与冻结 Delegate 卡片行断言、selection 高亮区间等语义级行断言，无整帧快照；`store_thinking.rs` 的行级精确断言为小型语义检查（前缀+内容+换行顺序），保留。
- **真实等待**：`src/input/test_cases/raw.rs::raw_esc_alone_flushed_after_timeout` 的 50ms 等待是 `ESC_ENTER_NEWLINE_WINDOW`（30ms）真实超时窗口的固有语义，生产 `flush_timeout` 无时间注入点且不属于"输入选择"（计划第 4 项范围为输入选择），保留并注明。

## Task 9 语义精简轨 §5.10 退役记录（neo-agent，结构轨提交 9fcf8734 之后）

### 陈旧断言重写（§6 重写，生产行为已提交，仅同步测试断言）

| 删除或合并的测试 | 保留的主要守护 | 两者共同捕获的生产故障 | 证明方式 | 精确命令 | 实际运行数 |
|---|---|---|---|---|---:|
| 重写：`modes::interactive::test_cases::approvals::approval_transcript_holds_every_request_and_focuses_earliest` → `approval_focus_owns_visible_window_until_resolved` | 同一测试重写（断言对齐 `706d9021` 阻塞焦点契约） | `706d9021 fix(tui): keep blocking transcript entries visible`：`render_visible_slice`→`DocumentLayout::visible_row_range` 把可视窗口约束到最早未决阻塞条目的卡片；全文档（`render_frame`/`render_snapshot`）仍按到达顺序保留全部审批卡片（`total_rows` 计入），解析后阻塞焦点前进到下一张 | 旧断言在 HEAD 确定性失败（approvals.rs:352 切片断言 `text.contains("printf two")`，切片已只含第一张卡片），重写后精确通过 | `cargo test --package neo-agent --bin neo -- modes::interactive::test_cases::approvals::approval_focus_owns_visible_window_until_resolved --exact --nocapture` | 1 |

### 表驱动合并（精简轨第 1 项：推理配置 / 权限模式 / 令牌上限）

| 删除或合并的测试 | 保留的主要守护 | 两者共同捕获的生产故障 | 证明方式 | 精确命令 | 实际运行数 |
|---|---|---|---|---|---:|
| `configured_low_reasoning_reaches_interactive_turn_unchanged`、`configured_max_reasoning_reaches_interactive_turn_unchanged`、`configured_budget_reasoning_reaches_interactive_turn_unchanged` | `sessions_config::configured_reasoning_selections_reach_interactive_turn_unchanged`（表驱动合并，3 个具名案例：low / max / budget） | 交互 turn 把 `config.runtime.reasoning`（`ReasoningSelection`：Effort low/max、BudgetTokens）原样送入 `TurnRequest.reasoning`（`capture_configured_interactive_turn_reasoning` 探针捕获）；三测试同走 controller 提交→run_turn 捕获链路，同一断言形状（`actual == expected`），仅推理选择输入变体 | 合并；案例逐个命名断言 | `cargo test --package neo-agent --bin neo -- modes::interactive::test_cases::sessions_config::configured_reasoning_selections_reach_interactive_turn_unchanged --exact --nocapture` | 1（3 案例） |
| `slash_ask_sets_ask_permission_mode`、`slash_auto_sets_auto_permission_mode`、`slash_yolo_sets_yolo_permission_mode` | `input_permissions::slash_permission_commands_set_mode_status_and_footer`（表驱动合并，3 个具名案例：ask / auto / yolo） | 交互 `/ask`、`/auto`、`/yolo` 斜杠命令提交后设置 `chrome().permission_mode()` 并写入 "Permission Mode: x" 状态行与 "[x]" footer 标记；三测试同走 type_text→Submit→三断言形状，仅（命令、模式、标签）输入变体 | 合并；案例逐个命名断言 | `cargo test --package neo-agent --bin neo -- modes::interactive::test_cases::input_permissions::slash_permission_commands_set_mode_status_and_footer --exact --nocapture` | 1（3 案例） |
| `cli_yolo_overrides_config_permission_mode`、`cli_auto_overrides_config_permission_mode` | `config::test_cases::loader::cli_permission_flags_override_config_permission_mode`（表驱动合并，2 个具名案例：yolo / auto） | `AppConfig::load`（src/config/loader.rs）权限模式解析中 CLI 标志优先分支：`if overrides.yolo { Yolo } else if overrides.auto { Auto }`；两测试同一断言形状（load 后 `permission_mode` 等于期望值），仅（yolo, auto）标志输入变体 | 合并；案例逐个命名断言 | `cargo test --package neo-agent --bin neo -- config::test_cases::loader::cli_permission_flags_override_config_permission_mode --exact --nocapture` | 1（2 案例） |
| `agent_config_for_app_falls_back_to_model_max_output_tokens`（连同 `agent_config_for_app_applies_runtime_config` 中 max_tokens 断言行） | `modes::run::test_cases::context::agent_config_max_tokens_uses_runtime_value_then_model_capability`（表驱动合并，3 个具名案例：runtime_wins / model_fallback / neither_unset） | `agent_config_for_app`（src/modes/run/runtime/agent.rs）的 max_tokens 优先级表达式 `config.runtime.max_tokens.or(model.capabilities.max_output_tokens)`：显式 runtime 值优先、否则回退模型 `max_output_tokens`、两者皆无时留空由 provider 决定；两测试重复同一完整 AppConfig 夹具，同一断言形状（`agent_config.max_tokens` 等于期望值），仅（runtime、模型能力）输入变体 | 合并；案例逐个命名断言；公共夹具收敛为 `app_config_with_runtime` 助手（两原测试与表测试共用） | `cargo test --package neo-agent --bin neo -- modes::run::test_cases::context::agent_config_max_tokens_uses_runtime_value_then_model_capability --exact --nocapture` | 1（3 案例） |

### 工作流命令合并（精简轨第 2 项）

| 删除或合并的测试 | 保留的主要守护 | 两者共同捕获的生产故障 | 证明方式 | 精确命令 | 实际运行数 |
|---|---|---|---|---|---:|
| `workflow_run_jsonl_streams_before_terminal` | `workflow_behavior::cli::workflow_run_jsonl_streams_in_order_and_returns_exact_exit_codes`（合并，吸收 `workflow_run_non_tty_streams_events_and_returns_exact_exit_codes` 全部断言并改名：事件顺序 + 精确退出码） | `neo workflow run --output jsonl` 非 TTY 流的事件顺序（首行 started、末行 terminal）与退出码契约（completed 退出 0、已删除命令非零退出）；两测试同走同一生产命令与同一断言形状（首行/末行 JSON `type` 字段），仅工作流名与脚本输入变体 | 合并；保留测试吸收全部断言（事件顺序、`lines.len() >= 2`、末行 state=completed、已删除命令拒绝），删除测试无独立断言 | `cargo test --package neo-agent --test workflow_behavior -- cli::workflow_run_jsonl_streams_in_order_and_returns_exact_exit_codes --exact --nocapture` | 1 |

### RPC 派生类型回原结构断言删除（精简轨第 3 项）

| 删除或合并的测试 | 保留的主要守护 | 两者共同捕获的生产故障 | 证明方式 | 精确命令 | 实际运行数 |
|---|---|---|---|---|---:|
| `rpc_sessions_get_returns_local_session_metadata_and_messages`（删除 `messages` 逐条内容回原结构断言） | 重命名保留：`rpc_sessions_get_returns_local_session_metadata_and_wire_path`；消息回放内容由 `rpc_get_messages_replays_session_jsonl_messages` 独立守护 | `handle_sessions_get` 与 `get_messages` 共用 `JsonlSessionReader::replay_messages`，消息经 `serde_json::to_value` 再序列化回原 JSONL 结构（`{"User":{"content":[{"Text":...}]}}`）；逐条内容断言为派生类型往返、无 sessions.get 自定义逻辑；字段名边界（`id`/`name`/`summary`/`parent_id`/`children`/`path` 与 `messages` 数组长度）保留 | 调用路径：handle_sessions_get → `JsonlSessionReader::replay_messages`（与 get_messages 同一生产函数）；断言删除后字段名边界断言不变 | `cargo test --package neo-agent --test rpc_behavior -- sessions::rpc_sessions_get_returns_local_session_metadata_and_wire_path --exact --nocapture` | 1 |
| `rpc_sessions_export_json_returns_sanitized_replayed_session_artifact`（删除 `messages` 逐条内容回原结构断言） | 重命名保留：`rpc_sessions_export_json_exposes_artifact_metadata_and_sanitizes_paths`；消息回放内容由 `rpc_get_messages_replays_session_jsonl_messages` 独立守护 | `export_json_artifact` 与 `get_messages` 共用 `JsonlSessionReader::replay_messages`，artifact 的逐条消息内容断言为派生类型往返、无 export 自定义逻辑；字段名与净化边界（`format`/`schema_version`/metadata `id`/`name`/`summary`/`parent_id`/`children`/`message_count`、绝对路径不泄漏、`share_url` 缺失）保留 | 调用路径：handle_sessions_export_json → `export_json_artifact` → `JsonlSessionReader::replay_messages`（与 get_messages 同一生产函数）；断言删除后字段名/净化边界断言不变 | `cargo test --package neo-agent --test rpc_behavior -- sessions::rpc_sessions_export_json_exposes_artifact_metadata_and_sanitizes_paths --exact --nocapture` | 1 |

### 本批次其余检查结论（无删除）

- **旧配置名测试**（精简轨第 4 项）：本计划无生产兼容路径退役权限；逐一检查 `config/test_cases/runtime.rs` 的 `runtime_shell_rejects_removed_limit_names`、`runtime_shell_rejects_removed_timeout_keys`——它们断言的是"已移除的键名被拒绝"（当前生产行为），不是旧配置名兼容测试，保留；未发现任何断言旧配置名仍被接受的测试，无删除。
- **保留域**（精简轨第 5 项）：真实进程（process_behavior 全部目标）、session 恢复（`rpc_behavior/recovery.rs`、`sessions_replay.rs`）、RPC 流（`rpc_behavior/streaming.rs`）、Unix 进程树与 Windows 作业对象（process_guard_unix.rs / process_guard_windows.rs，本批未触碰）、`rpc_get_messages_replays_session_jsonl_messages`（消息回放唯一主守护）原样保留。
- `config_defaults_to_ask_permission_mode`（无配置时 `unwrap_or_default` 默认分支）与 `config_loads_permission_mode_auto`（TOML 反序列化映射分支）是权限模式解析的两个不同生产层分支，非同一分支输入变体，保留；`permissions_picker_selects_auto_mode` 走 `/permissions` 选择器交互（不同于斜杠命令直设分支），保留。
- `model_selection_with_thinking_preserves_current_structured_reasoning`（选模型保留推理）与 `model_selection_without_thinking_sets_reasoning_off`（关推理）是模型选择器的两个相反分支，保留；`runtime_reasoning_uses_structured_config_and_migrates_legacy_effort`（遗留字段迁移）分支不同，保留。
- `agent_config_for_app_applies_runtime_config` 在删除 max_tokens 断言行后仍是运行时配置应用（temperature/retry/reasoning/队列/compaction/指令注册表）的主守护；`agent_config_for_app_scales_default_compaction_to_model_context_window`、`agent_config_for_app_keeps_explicit_custom_compaction_threshold` 分支不同，保留。
- `rpc_sessions_list_returns_local_session_metadata`（sessions.list 处理器字段名边界）与 `rpc_sessions_get_*`（sessions.get 处理器）是两个不同 RPC 方法、两个不同生产函数，保留；`rpc_get_messages_replays_session_jsonl_messages` 与 `rpc_get_messages_returns_empty_replay_for_empty_session` 是回放的两种状态（有内容/空），保留。

## Task 10 调度精确化证据

- 串行成员证据表（2026-08-07，本机）：三个原串行二进制（process_behavior 74 + runtime_behavior 173 + tool_behavior ~90 = 294 测试）。
  - 串行（旧配置 max-threads=1）：real 151.36s（current-serial.time）。
  - 真并行（无 test-group）：real 49.68s，0 失败（parallel-true.time/log）。
  - 结论：无任何测试在并行下失败 → 无测试满足"共享资源+并行失败证据"门槛 → shell-guardian 组整体删除（spec §5.5/§9"没有证据就保持普通并行"）。
- 资源测试保留：`complete_agent_output_survives_preview_queue_pressure`（12 MiB，45s 覆盖，并行下 20.4s 通过）、`child_pages_cover_thousand_and_ten_thousand_rows_with_stable_cursor`（10k 记录，45s 覆盖）、`authenticate_tool_reports_unwired_oauth_flow_without_success_claim`（30s 覆盖）。
- 陈旧覆盖删除：`mcp_manager_auth_action_shows_status_on_oauth_failure` 覆盖已移除（测试在治理前已由 759584ec 删除）。
- CI：`cargo nextest run --workspace --all-features --profile ci`；独立 `cargo build -p neo-agent` 步骤删除 —— 全新 CARGO_TARGET_DIR 无预构建下 `commands::root_command_reports_interactive_entrypoint_without_placeholders` 精确运行 1 passed（binexe-cold.time 77.26s 冷编译含二进制构建，hot 2.75s）。
- 最终串行过滤表达式：空（无成员），见 target/test-governance/final-serial-filter.txt。
- 死代码清理：共享夹具 http_server.rs 加 `#![allow(dead_code)]`（28 警告，各消费者二进制只用子集）；theme_manager.rs 表驱动用例补 case.name 断言（1 警告）。

## Task 11 最终验收（部分执行，用户中止）

- 最终冷构建（全新目录）：real 121.65s（基线 203.68s，-40%）。
- 最终热执行（5 次测量，均 EXIT=0，3455 测试全过）：154.39s / 157.12s / 160.06s / 160.63s / 165.07s；取最优 154.39s。基线热执行 179.50s。降幅约 14%，未达 60% 目标。
- 性能未达标根因（证据）：热执行墙钟被磁盘绑定测试主导；外部 macOS StorageManagement 扫描（CPU 峰值 130-164%）使同代码测试膨胀 28-63%（child_pages 39.0s→63.6s、complete_agent_output 18.0s→23.1s），CPU 绑定测试稳定（subagent 35.7→35.1s、clamps 18.2→17.7s）。串行组删除已生效：原 152.68s 串行组在并行下 49.68s 完成且 0 失败。
- 结论：热执行 60% 目标未达成，按计划 Task 11 step 8 提交瓶颈排序并停止删除高价值测试；<20 分钟目标达成（154s << 1200s）。冷阶段 -40%。
- 最终串行过滤表达式：空（无成员），`final-serial-filter.txt`；Task 11 的 `-E` 空表达式命令以"no tests to run"退出（预期，EXIT=4），串行组时间为 0。
- 平台矩阵：未执行（用户中止）。macOS 主机 Unix 进程树与全屏转录守护、Fedora/Windows VM 均未运行。
- 远端：未推送，当前提交远端未验证。
