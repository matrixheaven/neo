# NEO-36 ~ NEO-45 Tool Schema Description 完整性修复 Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 Neo 所有 Tool 的 `description()` 提示词补齐到 AI 可独立使用的水平——每个 Tool 的 description 必须覆盖使用时机、参数说明、输出格式、边界与异常、协作关系五个维度。

**Architecture:** 所有改动局限于各 Tool 源文件中的 `description()` 返回值（`&str` 字面量）。不涉及参数 schema 结构变更、trait 修改或运行时逻辑变更。核心原则：description 是写给 AI 模型看的 prompt，不是给人类看的 API 文档。

**Tech Stack:** Rust 2024, `neo-agent-core` crate, `schemars` JSON Schema derive, `cargo run -p xtask -- test`.

---

## Linear Context

| Issue | Title | Priority | Project | Label |
|-------|-------|----------|---------|-------|
| NEO-36 | Expand Write tool description from 7 words to full prompt | Urgent | Tool System | Enhancement |
| NEO-37 | Expand Edit tool description from 8 words to full prompt | Urgent | Tool System | Enhancement |
| NEO-38 | Rewrite ExitGoalMode description to match ExitPlanMode quality | High | Tool System | Enhancement |
| NEO-39 | Add frontmatter format example to CreateSkill description | High | Tool System | Enhancement |
| NEO-40 | Add return format and completion_criterion example to StartGoal | Medium | Tool System | Enhancement |
| NEO-41 | Add use-case explanation and bundle concept to MoveSkill | Medium | Tool System | Enhancement |
| NEO-42 | Add status enum values and exit_code semantics to TaskOutput | Medium | Tool System | Enhancement |
| NEO-43 | Add return JSON structure examples to TaskList and TaskStop | Low | Tool System | Enhancement |
| NEO-44 | Add tier system intro and Skill tool relationship to ListSkills | Low | Tool System | Enhancement |
| NEO-45 | Clarify session_id/days mutual exclusion in SummarizeSessions | Low | Tool System | Enhancement |

## 审计报告

完整的审计报告在 `outputs/neo-tool-schema-audit.html`（2026-06-22 生成）。

24 个 Tool 中：13 个 A 级（优秀）、6 个 B 级（可用）、5 个 C/D 级（需修复）。本 Plan 覆盖全部 10 个待修复项。

## 评估维度

每个 Tool description 从 5 个维度评估：

1. **使用时机** — AI 是否知道何时调用此 Tool（vs 其他 Tool）
2. **参数说明** — 每个参数的含义、类型、默认值是否清晰
3. **输出格式** — AI 是否知道返回值长什么样
4. **边界与异常** — 错误处理、限制条件是否说明
5. **协作关系** — 与其他 Tool 的配合/替代关系是否清晰

## 参考标杆

Read（500+ 字）和 Bash（1500+ 字）的 description 是质量标杆。关键特征：

- **使用时机**有 when-to-use 和 when-NOT-to-use 双向指引
- **协作关系**有"如果场景不是我，应该用哪个 Tool"的交叉引用
- **参数说明**包含路径解析规则、默认值、边界值
- **输出格式**有具体描述（行号前缀格式、截断标记、system 状态块）
- **边界与异常**有文件大小限制、截断行为、权限约束

---

## Task 1: NEO-36 — Expand Write tool description (P0)

**File:** `crates/neo-agent-core/src/tools/write.rs`

当前 description 仅 `"Write a UTF-8 file inside the workspace."`（7 个单词），5 个维度中有 4 个 ✗。

- [ ] **Step 1: 阅读当前 source**

```bash
cat crates/neo-agent-core/src/tools/write.rs
```

- [ ] **Step 2: 替换 `description()` 返回值**

新 description 必须覆盖：

```
Write a UTF-8 file inside the workspace.

Use Write to create new files or completely replace the contents of existing files.
For targeted modifications to existing files (find-and-replace a specific block),
use Edit instead — Edit returns a unified diff and preserves unchanged content.

Parameters:
- path: Path to the file to write. Relative paths resolve against the working
  directory; paths outside the working directory must be absolute.
- content: Full UTF-8 text content to write to the file.

Behavior:
- Overwrites the file if it already exists; creates the file if it does not.
- Creates parent directories as needed.
- Returns a confirmation with the number of bytes written.
- Only UTF-8 text content is supported.

Guidelines:
- Prefer Edit for surgical changes to existing files; use Write when the entire
  file content is new or being fully replaced.
- When writing code, ensure the content is complete and syntactically valid —
  partial writes can leave files in a broken state.
- For large files, consider whether Edit (targeted replacement) would be more
  appropriate than rewriting the entire file.
```

- [ ] **Step 3: 验证编译**

```bash
cargo run -p xtask -- check
```

- [ ] **Step 4: 验证测试**

```bash
cargo run -p xtask -- test -p neo-agent-core write
```

---

## Task 2: NEO-37 — Expand Edit tool description (P0)

**File:** `crates/neo-agent-core/src/tools/edit.rs`

当前 description 仅 `"Replace text in a UTF-8 workspace file."`（8 个单词），5 个维度中有 4 个 ✗。

- [ ] **Step 1: 阅读当前 source**

```bash
cat crates/neo-agent-core/src/tools/edit.rs
```

- [ ] **Step 2: 替换 `description()` 返回值**

新 description 必须覆盖：

```
Replace text in a UTF-8 workspace file. Use Edit for targeted modifications to
existing files — it finds the exact `old` text and replaces it with `new`.
For creating new files or full content replacement, use Write instead.

Parameters:
- path: Path to the file to edit. Relative paths resolve against the working
  directory; paths outside the working directory must be absolute.
- old: Exact existing text to find and replace. Must match the file content
  character-for-character, including whitespace and indentation.
- new: The replacement text that will be inserted in place of old.
- replace_all: When false (default), old must match exactly one location in the
  file; if it matches multiple locations, the edit fails with the match count so
  you can provide more context. When true, every occurrence of old is replaced.

CRITICAL — unique match requirement:
When replace_all is false, old must match exactly one location. If old matches
zero locations, the edit fails and the file is unchanged — re-read the file and
adjust old to match the current content. If old matches multiple locations, the
edit fails with the count — either add more surrounding context to old to make
it unique, or set replace_all to true.

Output:
Returns a unified diff showing the changes made, so you can verify the edit
produced the intended result.

Guidelines:
- Always read the file first to confirm the exact current content of old.
- Include enough surrounding context (imports, function signatures, closing
  braces) in old to ensure a unique match.
- For renaming a variable or symbol across the entire file, use replace_all=true.
- If an edit fails, do not guess — re-read the file and try again with corrected
  old text.
```

- [ ] **Step 3: 验证编译**

```bash
cargo run -p xtask -- check
```

- [ ] **Step 4: 验证测试**

```bash
cargo run -p xtask -- test -p neo-agent-core edit
```

---

## Task 3: NEO-38 — Rewrite ExitGoalMode description (P1)

**File:** `crates/neo-agent-core/src/tools/goal.rs` (ExitGoalModeTool impl)

当前 description 过短，缺乏 phases 格式示例、审批流程说明、与 StartGoal 的路径差异。

- [ ] **Step 1: 阅读 goal.rs 中 ExitGoalModeTool 的 description**

- [ ] **Step 2: 重写 description，参照 ExitPlanMode 的详细程度**

必须覆盖：

```
Use this when goal mode has produced a reviewed goal draft and is ready for
user approval. The user will review the objective, completion criterion, and
phases in a blocking dialog.

How this tool works:
- This tool submits the drafted goal for user review. It does NOT start the goal
  directly — the user must approve it first.
- If approved, the durable goal is created and the runtime begins autonomous
  turns to pursue it.
- If rejected, goal mode remains active so you can revise the draft.
- If the user requests revisions, update the objective/phases and call this
  tool again.

Two paths to create a goal:
1. Goal mode (this tool) — the AI drafts a structured goal through
   conversation, then submits it via ExitGoalMode for blocking review.
2. Direct /goal command — the user authors the goal objective directly via
   the /goal <objective> slash command, bypassing the AI draft step.

Parameters:
- objective: The approved goal objective. Must have a verifiable end state.
- completion_criterion: How to verify the goal is complete. Example:
  "all integration tests pass" or "the API returns 200 for all documented
  endpoints".
- phases: Ordered list of phase descriptions. Each phase should be a
  self-contained milestone. Example: ["Phase 1: Set up test fixtures and
  data models", "Phase 2: Implement core API endpoints", "Phase 3: Add
  error handling and integration tests"].

Permission mode notes:
- In yolo and ask modes, the user reviews the goal in a blocking dialog.
- In auto permission mode, the goal starts without user review.

Before using:
- Make sure the objective has a checkable completion condition.
- If the user's request is vague, ask for the missing completion criterion
  before calling this tool.
```

- [ ] **Step 3: 验证编译和测试**

```bash
cargo run -p xtask -- check
cargo run -p xtask -- test -p neo-agent-core goal
```

---

## Task 4: NEO-39 — Add frontmatter example to CreateSkill (P1)

**File:** `crates/neo-agent-core/src/tools/skills_manager.rs` (CreateSkillTool impl)

- [ ] **Step 1: 阅读 skills_manager.rs 中 CreateSkillTool 的 description**

- [ ] **Step 2: 重写 description**

必须添加：
- 使用时机（"when a complex task has been completed and the workflow should be preserved for reuse"）
- frontmatter 格式示例
- type 三种值差异说明

```
Create a new skill under ~/.neo/skills/<name>/SKILL.md for reuse in future
sessions.

When to use:
- After completing a complex, multi-step task whose workflow should be preserved.
- When the user explicitly asks to save a procedure as a skill.
- When an error was overcome and the resolution should be recorded.

When NOT to use:
- For trivial one-off tasks that are unlikely to recur.
- For information that is already documented in AGENTS.md or project docs.

The skill file must include valid YAML frontmatter followed by Markdown content.
Example:

  ---
  name: deploy-staging
  description: Deploys the app to staging. Use when the user asks to deploy or
    push to the staging environment.
  type: prompt
  whenToUse: When the user asks to deploy to staging, push to staging, or
    update the staging environment.
  ---

  # Deploy to Staging

  ## Steps
  1. Run `cargo build --release`
  2. ...

Frontmatter fields:
- name (required): Skill identifier, must match the directory name.
- description (required): One-line summary of what the skill does.
- type (required): One of "prompt" (injected as a context message before the
  user's message), "inline" (expanded directly into the prompt), or "flow"
  (multi-step interactive workflow).
- whenToUse (recommended): Natural language trigger description for automatic
  skill selection.

If a skill with the same name already exists, the existing file is backed up
under ~/.neo/backups/skills/<timestamp>/<name>/SKILL.md before being overwritten.

After creation, the skill can be activated via the Skill tool or the
/skill:<name> slash command.

Parameters:
- name: Directory name for the skill under ~/.neo/skills/.
- description: Short description of what the skill does.
- skill_type: "prompt", "inline", or "flow". Defaults to "prompt".
- body: Full Markdown content including YAML frontmatter and the skill body.
```

- [ ] **Step 3: 验证编译和测试**

```bash
cargo run -p xtask -- check
cargo run -p xtask -- test -p neo-agent-core skills
```

---

## Task 5: NEO-40 — Add return format to StartGoal (P2)

**File:** `crates/neo-agent-core/src/tools/goal.rs` (StartGoalTool impl)

- [ ] **Step 1: 在现有 description 末尾追加返回格式和 completion_criterion 示例**

追加内容：

```
Returns:
On success, returns the created goal's ID and initial status ("active").
If an active goal already exists and replace is false, the call fails with
an error identifying the existing goal.

completion_criterion examples:
- "All integration tests in tests/api/ pass without errors"
- "The README.md contains a quickstart section with a working code example"
- "cargo clippy reports zero warnings across the workspace"
```

- [ ] **Step 2: 验证编译和测试**

```bash
cargo run -p xtask -- check
cargo run -p xtask -- test -p neo-agent-core goal
```

---

## Task 6: NEO-41 — Add use-case and bundle concept to MoveSkill (P2)

**File:** `crates/neo-agent-core/src/tools/skills_manager.rs` (MoveSkillTool impl)

- [ ] **Step 1: 重写 MoveSkill description**

```
Move a skill directory into a parent bundle directory, creating timestamped
backups of every affected directory.

When to use:
- To group related skills under a shared parent directory (a "bundle").
  A bundle is simply a directory under ~/.neo/skills/ that contains multiple
  skill subdirectories, e.g. ~/.neo/skills/deploy-bundle/deploy-staging/
  and ~/.neo/skills/deploy-bundle/deploy-prod/.
- To reorganize skills after they have been created.

When NOT to use:
- To rename a skill (create a new one and delete the old one instead).
- To move a skill to a different machine or workspace.

Parameters:
- source: Absolute path to the skill directory to move. Must contain a
  SKILL.md file.
- destination_parent: Absolute path to the parent directory where the skill
  directory should be moved. The skill's directory name is preserved under
  this parent.

Behavior:
- Before the move, a timestamped backup of the source directory is created
  under ~/.neo/backups/skills/<timestamp>/.
- If the destination already exists (a skill with the same name already lives
  under destination_parent), the move is rejected and no changes are made.
- Returns the new absolute path of the moved skill directory.

After the move, the skill is discovered from its new location on the next
skill scan. No manual re-registration is needed.
```

- [ ] **Step 2: 验证编译和测试**

```bash
cargo run -p xtask -- check
cargo run -p xtask -- test -p neo-agent-core skills
```

---

## Task 7: NEO-42 — Add status enum and exit_code to TaskOutput (P2)

**File:** `crates/neo-agent-core/src/tools/background_tasks.rs` (TaskOutputTool impl)

- [ ] **Step 1: 在现有 description 的 Guidelines 部分追加 status/exit_code 说明**

追加内容：

```
Return fields:
- status: One of "running" (the task is still executing), "completed"
  (the task finished successfully), "failed" (the task exited with a
  non-zero exit code), or "stopped" (the task was cancelled via TaskStop).
- exit_code: The process exit code for terminal tasks. 0 means success;
  non-zero means failure. Only present when status is "completed", "failed",
  or "stopped".
- output: A preview of the task's stdout/stderr, capped at max_output_bytes.
```

- [ ] **Step 2: 验证编译和测试**

```bash
cargo run -p xtask -- check
cargo run -p xtask -- test -p neo-agent-core background
```

---

## Task 8: NEO-43 — Add return JSON examples to TaskList and TaskStop (P3)

**File:** `crates/neo-agent-core/src/tools/background_tasks.rs`

- [ ] **Step 1: 在 TaskListTool description 追加返回结构示例**

```
Return format:
Returns a list of background tasks. Each entry includes:
- task_id: Unique identifier for the task (use this with TaskOutput/TaskStop).
- status: "running", "completed", "failed", or "stopped".
- kind: The type of background task (e.g. "bash", "ask_user").
- description: Short human-readable description provided at creation time.
- elapsed: Time since the task was started (e.g. "2m 30s").
```

- [ ] **Step 2: 在 TaskStopTool description 追加返回说明**

```
Return format:
Returns the task's final status after the stop attempt. If the task was
still running, it is stopped and the output collected so far is included.
If the task had already finished, the current status and output are
returned without any action taken.
```

- [ ] **Step 3: 验证编译和测试**

```bash
cargo run -p xtask -- check
cargo run -p xtask -- test -p neo-agent-core background
```

---

## Task 9: NEO-44 — Add tier system intro to ListSkills (P3)

**File:** `crates/neo-agent-core/src/tools/skills_manager.rs` (ListSkillsTool impl)

- [ ] **Step 1: 重写 ListSkills description**

```
List all discoverable skills by tier (user, extra, builtin) with their names
and filesystem paths.

Use this tool to inspect which skills are available in the current environment
before invoking one with the Skill tool or a slash command.

Skill discovery tiers (in priority order):
1. user: Skills in ~/.neo/skills/ — created by the user or the CreateSkill
   tool. These take highest priority when multiple skills share a name.
2. extra: Skills in directories listed in the config's extra_skill_dirs
   setting. Useful for team-shared skill directories.
3. builtin: Skills shipped with Neo (e.g. sub-skill, self-evo). These are
   extracted into ~/.neo/skills/.builtin/ on startup. Only included in the
   listing when include_builtin=true.

Output format:
Skills are grouped by tier and each entry shows the skill name and its
absolute filesystem path. Skills discovered at a higher tier shadow
lower-tier skills with the same name.

After identifying a skill, activate it via:
- The Skill tool (programmatic invocation).
- The /skill:<name> slash command (manual invocation in the TUI).

Parameters:
- include_builtin: When true, also list built-in skills shipped with Neo.
  Defaults to false to keep the listing focused on user-managed skills.
```

- [ ] **Step 2: 验证编译和测试**

```bash
cargo run -p xtask -- check
cargo run -p xtask -- test -p neo-agent-core skills
```

---

## Task 10: NEO-45 — Clarify mutual exclusion in SummarizeSessions (P3)

**File:** `crates/neo-agent-core/src/tools/sessions.rs` (SummarizeSessionsTool impl)

- [ ] **Step 1: 在现有 description 中补充互斥说明和错误行为**

在 Parameters 部分追加：

```
Note: session_id and days are mutually exclusive — provide exactly one.
If session_id is given, only that specific session is summarized. If days
is given, all sessions from the last N days are summarized.

If the specified session_id does not exist, the tool returns an error
listing available session IDs. If days yields no sessions, the tool
returns a status message indicating no sessions were found in the given
time range.
```

- [ ] **Step 2: 验证编译和测试**

```bash
cargo run -p xtask -- check
cargo run -p xtask -- test -p neo-agent-core sessions
```

---

## Verification

全部 10 个 task 完成后：

```bash
# Full workspace check
cargo run -p xtask -- check

# Run all neo-agent-core tests
cargo run -p xtask -- test -p neo-agent-core
```

每个 Task 的改动都是纯字符串替换（`description()` 返回值），不涉及逻辑变更，因此不需要 LCOV/CRAP。
