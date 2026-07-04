# 目标模式

目标模式（Goal Mode）让 Neo 把一个**可验证的目标**作为会话级状态，跨多个 turn 自主推进，直到完成、阻塞或被你暂停。

## 什么是目标模式

普通对话里，每个 turn 都是独立的请求/响应。目标模式则不同：Neo 会维护一份**持久化目标记录**（objective、完成判据、阶段计划、状态），在每个 turn 结束时判断是否继续推进、是否已完成、是否阻塞。

| 元素 | 作用 |
| --- | --- |
| **objective** | 目标描述，必须有可检查的终止状态 |
| **completion_criterion** | 完成判据，例如「`cargo test` 全部通过」 |
| **phases** | 有序的阶段列表，每阶段是一个自包含里程碑 |
| **status** | `active` / `paused` / `blocked` / `complete` / `queued` |
| **artifact_dir** | 存放目标相关产物（阶段文件等）的目录 |

目标记录持久化在会话目录下，恢复会话时一同还原。

## /goal 命令

`/goal` 是面向用户的目标管理入口。常见用法：

| 命令 | 作用 |
| --- | --- |
| `/goal <objective>` | 直接创建/替换当前目标 |
| `/goal` 或 `/goal status` | 查看当前目标状态、耗时、队列长度 |
| `/goal pause` | 暂停当前目标（可恢复） |
| `/goal resume` | 恢复一个暂停或阻塞的目标 |
| `/goal cancel` | 取消当前目标 |
| `/goal replace <objective>` | 用新目标替换当前目标 |
| `/goal next <objective>` | 把目标排入队列（若当前无活动目标则立即开始） |
| `/goal next manage` | 查看已排队的目标 |

也可以让 AI 通过 `EnterPlanMode` 风格的对话起草结构化目标，再由 `ExitGoalMode` 工具提交给你审批。

## 目标生命周期

目标模式有两条等价的创建路径：**AI 起草 → 用户审批**，或**用户直接 `/goal`**。

```text
          ┌──────────────┐
   /goal  │   Draft      │  AI 通过对话起草
 ───────▶ │  (authoring) │  objective / criterion / phases
          └──────┬───────┘
                 │ ExitGoalMode
                 ▼
          ┌──────────────┐   Reject    ┌────────┐
          │   Implement  │ ──────────▶ │ Draft  │
          │   (active)   │             └────────┘
          └──────┬───────┘
                 │ UpdateGoalStatus
                 ▼
          ┌──────────────┐
          │    Audit     │  complete / blocked / paused
          └──────────────┘
```

| 阶段 | 状态 | 谁来驱动 |
| --- | --- | --- |
| **Draft** | goal mode authoring | AI 起草，用户可 Revise/Reject |
| **Implement** | `active` | 运行时自动连续推进，每 turn 末判断是否继续 |
| **Audit** | `complete` / `blocked` / `paused` | 由 AI 调用 `UpdateGoalStatus` 切换 |

> 在 **Auto** 权限模式下，`ExitGoalMode` 不会弹审批框，目标直接开始；在 **Ask / YOLO** 模式下，用户需在阻塞对话框里 Approve / Reject / Revise。

每个 turn 结束后，若目标仍为 `active`，运行时会自动注入一条 goal-continuation 系统消息，提示 Neo 继续推进；若已完成且队列中还有目标，会自动开始下一个。

## 工具一览

| 工具 | 谁调用 | 作用 |
| --- | --- | --- |
| `StartGoal` | AI | 直接启动一个持久化目标（用户明确要求时） |
| `ExitGoalMode` | AI | 把起草好的结构化目标提交给用户审批 |
| `GetGoalStatus` | AI | 读取当前目标快照 |
| `UpdateGoalStatus` | AI | 切换 `active` / `complete` / `paused` / `blocked` |

用户侧则通过 `/goal` 系列命令操作。

## 示例

### 直接起一个目标

```
/goal 让 cargo clippy 在整个 workspace 上零警告
```

Neo 会自主选择工具、连续多 turn 修复 lint，每 turn 末自行判断是否完成。你随时可以 `/goal status` 查看进度，或 `/goal pause` 暂停。

### 让 AI 起草结构化目标

```
帮我设计并实现一个 CLI 子命令 neo foo，先出方案
```

Neo 在目标模式下会先起草 objective、completion_criterion 与 phases，调用 `ExitGoalMode` 弹出审批对话框，你可以选择 Approve、Revise（附反馈）或 Reject。

### 排队后续目标

```
/goal next 给新命令补一份中文文档
```

当前目标完成时，排队的下一个目标会自动开始。

## 下一步

- [计划模式](plan-mode.md) — 在动手前先获得方案审批
- [交互模式](interaction.md) — 审批对话框与权限模式
- [会话管理](sessions.md) — 目标产物随会话一起持久化
