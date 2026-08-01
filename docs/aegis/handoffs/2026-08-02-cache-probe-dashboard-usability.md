# Handoff: Cache Probe Dashboard Usability

在 `/Users/chenyuanhao/Workspace/neo` 实施本交接。需求已经确定，不要重新设计，
不要扩展到 Neo 运行时。

## Spec

只修改现有所有者：

- `tools/cache_probe.py`：提供历史运行列表与指定运行报告。
- `tools/cache_probe.html`：运行切换、请求表分页和顶部指标布局。
- `docs/aegis/specs/2026-08-01-deepseek-cache-probe-design.md`：同步最终行为。

确定行为：

1. 顶部删除 `First requests` 和 `Stable` 两张卡片。前者与 `Sequences`
   重复，后者已有 `Stable rate`；字段仍保留在 `report.json`，只是不再作为
   顶部卡片显示。适当降低卡片最小宽度，使桌面宽度不小于 1440 像素时剩余
   10 张卡片保持一行；窄屏允许自然换行，标签不得溢出。
2. 请求表固定每页 50 条，不提供页大小设置。工具栏显示上一页、下一页和
   `当前页 / 总页数`。切换运行或 Sequence 时回到第一页并清除选中详情；
   实时刷新时保留当前页，但总页数缩小时要自动收回有效页码。
3. 分页只影响请求表。顶部汇总、趋势图、工具方差图仍使用所选运行和
   Sequence 的完整数据，不能按当前页截断。
4. 页头把静态的 `run <id>` 改成 Run 下拉框。首次打开默认选择当前代理的
   active run；用户切到历史运行后，轮询不得自动跳回 active run。
5. 后端直接枚举 `--output-root` 下具有 `report.json` 的一级目录，不创建
   数据库、索引文件、迁移或缓存。
6. 新增只读接口：
   - `GET /runs.json` -> `{active_run_id, runs:[{id, active, updated_at}]}`，按
     `report.json` 修改时间倒序。
   - `GET /runs/<run-id>/report.json` -> 对应原始报告。
7. 删除旧的 `GET /report.json` 页面路径，并同步页面与自测，只保留上述唯一
   报告读取路径。运行编号必须来自服务器枚举结果；拒绝路径穿越和任意文件读取。
8. 无效、缺失或不可读取的历史报告不进入运行列表。当前选中的历史报告暂时
   读取失败时，保留最后一次有效页面并显示现有错误横幅。

明确不做：后端分页、历史删除、重命名、收藏、比较两个运行、数据库、依赖、
报告格式升级、旧字段转换或完整页面重构。

## Plan

`TDD Route: off / skipped`。使用现有自测做修改后回归，不引入测试框架。

### Task 1: 历史运行接口

在 `tools/cache_probe.py` 复用 `RunStore.output_root`：实现一级目录枚举、安全的
运行编号解析和两个只读接口。扩展现有自测，覆盖倒序、active 标记、缺失报告、
未知运行和 `../` 路径拒绝。

### Task 2: 页面运行切换与分页

在 `tools/cache_probe.html`：删除 `First requests` 和 `Stable` 卡片；增加 Run
下拉框和固定 50 条分页状态；将现有过滤、表格、详情和轮询统一到选中的运行。
不要复制统计逻辑，也不要改变图表的数据范围。

### Task 3: 验证与提交

运行：

```bash
python3 tools/cache_probe.py --self-test
git diff --check -- tools/cache_probe.py tools/cache_probe.html \
  docs/aegis/specs/2026-08-01-deepseek-cache-probe-design.md
```

再用 `--self-test-server` 创建至少 3 个历史运行，浏览器验证 1440 像素桌面和
390 像素窄屏；构造 120 条请求，确认分页为 `50 / 50 / 20`，切换 Run 和
Sequence 后状态正确，控制台无错误。最后只提交上述三个文件：

```text
feat(dev): improve cache probe run navigation
```

## Stop Conditions

- 不要修改 `crates/`、`.gitignore` 或其他并发文件。
- 不要推送、切换分支、建立工作树或清理用户改动。
- 若现有报告无法由当前页面直接读取，报告具体字段差异并停止；不要增加旧格式
  兼容层。
- 返回改动摘要、精确验证结果、提交编号和未覆盖风险，交回原任务做一次终审。
