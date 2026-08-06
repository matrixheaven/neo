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
