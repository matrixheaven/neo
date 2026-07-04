# 计划模式

计划模式（Plan Mode）让 Neo 在动手改代码之前，先用只读工具调研代码库、写出方案、交给你审批。适合不确定路径、有多种方案或多文件改动的任务。

## 计划模式概念

进入计划模式后，Neo **只能使用只读工具**（Read / Grep / Glob 等）调研，并允许写入**计划文件**；其他文件写入与 shell 命令被禁止，直到退出计划模式。

```text
  EnterPlanMode  ──▶  只读探索 + 写计划文件  ──▶  ExitPlanMode ──▶ 审批
                                                            │
                                            ┌───────────────┼───────────────┐
                                            ▼               ▼               ▼
                                         Approve        Revise          Reject
                                            │               │               │
                                         继续执行      回到计划修改       取消计划
```

## 进入 / 退出计划模式

| 方式 | 操作 |
| --- | --- |
| 斜杠命令 | `/plan`、`/plan on`、`/plan off` |
| 清除计划文件 | `/plan clear` |
| 快捷键 | `Shift+Tab` 在 Normal → Plan → Goal 间循环切换 |
| AI 触发 | Neo 自主调用 `EnterPlanMode` 工具（适合非平凡的实现任务） |

`EnterPlanMode` 在**所有权限模式下都直接进入**，不弹审批框。进入后状态栏会显示 `Plan Mode On`。

## 审批流程

当 Neo 完成方案、调用 `ExitPlanMode` 时（或在 Ask/YOLO 模式下），会弹出审批对话框：

| 选项 | 含义 |
| --- | --- |
| **Approve** | 批准方案，退出计划模式并开始执行 |
| **Reject** | 拒绝方案，退出计划模式但不执行 |
| **Revise** | 附反馈拒绝，Neo 据此改写计划文件后再次提交 |

`ExitPlanMode` 还可携带：

- **`plan_summary`**：方案的简短概述（方案本体应已写入计划文件）。
- **`options`**：最多 3 个备选方案，每个含 label 与 description；推荐项的 label 加 `(Recommended)` 后缀并排在第一位。系统会自动附加 Reject / Revise 控件。
- **`suggestions`**：最多 5 个预设的「修订建议」，选中后自动填入反馈文本。

> 保留字 label：`approve` / `reject` / `revise` / `reject and exit` 不能作为 option 标签。

### 各权限模式下的行为

| 权限模式 | `EnterPlanMode` | `ExitPlanMode` |
| --- | --- | --- |
| **Ask** | 直接进入 | 弹审批对话框 |
| **YOLO** | 直接进入 | 弹审批对话框 |
| **Auto** | 直接进入 | **不弹框**，直接退出计划模式开始执行 |

## 计划文件

计划模式下的「方案本体」是一份**计划文件**，存放在当前会话目录下的 `plans/` 子目录里：

```
<会话桶>/agents/main/plans/<plan-file>
```

Neo 用 `Write` / `Edit` 把方案写入该文件，`ExitPlanMode` 不接受方案内容参数——它读取你写好的计划文件并展示给用户。`/plan clear` 可清空当前计划文件。

退出计划模式后，已批准的方案会作为后续执行 turn 的上下文，Neo 据此开始实际修改代码。

## 何时用、何时不用

| 场景 | 建议 |
| --- | --- |
| 新功能实现、架构决策、多文件改动 | ✅ 用计划模式 |
| 有多种合理方案需要你拍板 | ✅ 用，并配合 `options` |
| 一两行的明显修复、错别字 | ❌ 直接做 |
| 用户已给出详尽步骤 | ❌ 直接执行 |
| 纯调研/理解代码 | ❌ 直接用只读工具，无需 `ExitPlanMode` |

## 下一步

- [目标模式](goals.md) — 跨多 turn 自主推进可验证目标
- [交互模式](interaction.md) — 审批对话框与权限模式
- [用例配方](use-cases.md) — 实现功能、修复 bug、重构等场景模板
