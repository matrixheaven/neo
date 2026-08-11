/**
 * Browser verification against the test-only mock server (final protocol:
 * grouped workspace snapshot, usage/context on session state, attachments,
 * agent history). Produces the acceptance screenshot set under screenshots/
 * (gitignored) and the two performance probes from the redesign spec §9.
 */

import { test, expect, type Page } from "@playwright/test";

const SHOTS = "screenshots";

async function openApp(page: Page, width: number, height: number) {
  // Dark is the product default; Playwright's colorScheme default is light,
  // so the dark shots pin the scheme explicitly (the light shots toggle).
  await page.emulateMedia({ colorScheme: "dark" });
  await page.setViewportSize({ width, height });
  await page.goto("/#access=playwright-verification");
  // The fragment is claimed once and cleared.
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByLabel("会话列表", { exact: true })).toBeVisible();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  // The grouped workspace snapshot has arrived (cross-workspace groups).
  await expect(page.getByRole("group", { name: "playground" })).toBeVisible();
}

function sidebar(page: Page) {
  return page.getByLabel("会话列表", { exact: true });
}

/** Open the showcase session and wait for its full snapshot to render. */
async function openShowcase(page: Page) {
  await sidebar(page).getByText("重设计走查", { exact: true }).click();
  await expect(
    page.getByText("验收改造已完成：转录改为行式层级，结束的回合折叠为工作过程摘要，回答下方列出本轮修改的文件。"),
  ).toBeVisible();
  // Final state: an in-progress tool line and the running composer affordances.
  await expect(
    page.getByRole("button", { name: "展开运行 npm run test:browser，状态：运行中" }),
  ).toBeVisible();
}

async function switchToLight(page: Page) {
  await page.getByRole("button", { name: "切换主题" }).click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
}

test("01 宽屏新会话：欢迎 banner 与 pill 行", async ({ page }) => {
  await openApp(page, 1440, 900);
  await expect(page.getByLabel("输入消息", { exact: true })).toBeVisible();
  await expect(page.getByRole("note")).toContainText("描述你的任务，回车发送");
  await expect(page.getByRole("button", { name: "模型与推理（仅下一回合）" })).toBeVisible();
  await expect(page.getByLabel("选择项目")).toHaveValue("workspace_sample");
  await expect(page.locator(".workspace-branch")).toContainText("main");
  await page.screenshot({ path: `${SHOTS}/01-wide-new-session.png` });
});

test("01b 添加项目", async ({ page }) => {
  await openApp(page, 1440, 900);
  await page.getByRole("button", { name: "添加项目" }).click();
  await expect(page.getByRole("dialog", { name: "添加项目" })).toBeVisible();
  await expect(page.getByLabel("项目文件夹")).toBeFocused();
  await page.screenshot({ path: `${SHOTS}/01b-add-workspace.png` });
});

test("02 宽屏运行会话：tool-line 行态与 TurnFold", async ({ page }) => {
  await openApp(page, 1440, 900);
  await openShowcase(page);
  // Finished turn collapses behind the fold summary; the running turn's
  // process rows stay visible.
  await expect(
    page.getByRole("button", { name: /展开工作过程（.*个步骤）/ }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: /查看子代理详情：检查测试覆盖/ }),
  ).toBeVisible();
  await expect(page.getByRole("img", { name: /上下文占用/ })).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/02-wide-running-session.png` });
});

test("03 思考展开", async ({ page }) => {
  await openApp(page, 1440, 900);
  await openShowcase(page);
  // Two finished think lines share the same accessible name: turn 1's is
  // inside the collapsed fold, turn 2's (the visible one) is last in DOM.
  await page.getByRole("button", { name: "展开思考，状态：已完成" }).last().click();
  await expect(
    page.getByText("子代理的历史已经落盘，面板直接按同一条投影线渲染即可。"),
  ).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/03-thinking-expanded.png` });
});

test("04 工具展开详情：命令回显、参数、输出与元信息", async ({ page }) => {
  await openApp(page, 1440, 900);
  await openShowcase(page);
  await page.getByRole("button", { name: /展开工作过程（.*个步骤）/ }).click();
  await page.getByRole("button", { name: "展开运行 cargo test -p neo-webui，状态：已完成" }).click();
  await expect(page.getByText("$ cargo test -p neo-webui")).toBeVisible();
  await expect(page.getByText("test result: ok. 42 passed").first()).toBeVisible();
  await expect(page.getByText("状态：已完成", { exact: true }).first()).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/04-tool-expanded-detail.png` });
});

test("05 三栏 Subagents 与子转录", async ({ page }) => {
  await openApp(page, 1440, 900);
  await openShowcase(page);
  const trigger = page.getByRole("button", { name: /查看子代理详情：检查测试覆盖/ });
  const panel = page.getByLabel("会话信息区", { exact: true });
  await expect(panel).toBeVisible();
  await expect(panel.getByRole("tab", { name: "Subagents" })).toHaveAttribute("aria-selected", "true");
  await expect(panel.getByRole("heading", { name: "Done 1" })).toBeVisible();
  await expect(page.getByRole("dialog", { name: /子代理详情/ })).toHaveCount(0);
  await expect.poll(() => panel.evaluate((element) => element.getBoundingClientRect().width))
    .toBeGreaterThan(400);
  const columns = await page.evaluate(() => {
    const left = document.querySelector<HTMLElement>(".sidebar");
    const middle = document.querySelector<HTMLElement>(".main-area");
    const right = document.querySelector<HTMLElement>(".information-panel.open");
    if (!left || !middle || !right) throw new Error("三栏未渲染");
    const leftRect = left.getBoundingClientRect();
    const middleRect = middle.getBoundingClientRect();
    const rightRect = right.getBoundingClientRect();
    return {
      leftRight: leftRect.right,
      middleLeft: middleRect.left,
      middleRight: middleRect.right,
      rightLeft: rightRect.left,
      middleWidth: middleRect.width,
      rightWidth: rightRect.width,
    };
  });
  expect(columns.leftRight).toBeLessThanOrEqual(columns.middleLeft + 1);
  expect(columns.middleRight).toBeLessThanOrEqual(columns.rightLeft + 1);
  expect(columns.middleWidth).toBeGreaterThan(400);
  expect(columns.rightWidth).toBeGreaterThan(400);

  await trigger.click();
  await expect(
    panel.getByText("relay 测试覆盖良好：慢连接注销与 1013 关闭都有行为测试。"),
  ).toBeVisible();
  await expect(panel.getByText("已编辑 1 个文件")).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/05-three-column-subagents.png` });
  await panel.getByRole("button", { name: "关闭会话信息区" }).click();
  await expect(panel).toBeHidden();
  await expect(trigger).toBeFocused();
});

test("06 swarm 块：成员列表与聚合条", async ({ page }) => {
  await openApp(page, 1440, 900);
  await sidebar(page).getByText("并行格式化", { exact: true }).click();
  await expect(page.getByText("完成 2/2", { exact: true })).toBeVisible();
  await expect(page.getByRole("progressbar", { name: "聚合进度：已结束 2/2" })).toBeVisible();
  // Member list: one agent-line per child (the per-item text span is
  // tooltip-only metadata, display:none by design).
  const members = page.locator(".swarm-members .swarm-member");
  await expect(members).toHaveCount(2);
  await expect(
    members.getByRole("button", { name: /查看子代理详情：检查测试覆盖/ }),
  ).toHaveCount(2);
  await expect(members.locator(".swarm-member-item").first()).toBeAttached();
  await page.screenshot({ path: `${SHOTS}/06-swarm-block.png` });
});

test("07 用户长消息渐变折叠态", async ({ page }) => {
  await openApp(page, 1440, 900);
  await openShowcase(page);
  await page.locator(".transcript-scroll").evaluate((element) => {
    element.scrollTo({ top: 0 });
  });
  const clamped = page.locator(".u-text-wrap.is-clamped");
  await expect(clamped).toBeVisible();
  await expect(clamped.getByRole("button", { name: "展开" })).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/07-long-user-message-collapsed.png` });
});

test("08 answer-ft 浮层与 Review 工作区", async ({ page }) => {
  await openApp(page, 1440, 900);
  await openShowcase(page);
  const footer = page.locator(".answer-ft");
  await footer.scrollIntoViewIfNeeded();
  await expect(footer.getByText("已编辑 2 个文件")).toBeVisible();
  await expect(footer.locator(".ft-path", { hasText: "web/src/styles.css" })).toBeVisible();
  await expect(footer.locator(".ft-path", { hasText: "web/src/acceptance-notes.md" })).toBeVisible();
  await expect(
    footer.getByRole("button", { name: "在 Review 中查看 web/src/styles.css 的局部差异" }),
  ).toBeVisible();
  const createdFile = footer.getByRole("button", {
    name: "在 Review 中查看 web/src/acceptance-notes.md 的新建文件内容",
  });
  const createdRow = createdFile.locator("..");
  const rowBeforeHover = await createdRow.boundingBox();
  await expect(createdFile).toBeVisible();
  await createdFile.hover();
  const preview = page.getByRole("region", { name: "web/src/acceptance-notes.md 的文件内容" });
  await expect(preview).toBeVisible();
  await expect(preview.getByText("# 验收记录")).toBeVisible();
  await expect(preview.locator(".ft-preview-header")).toContainText("acceptance-notes.md");
  await expect(preview.locator(".ft-line-no").first()).toBeVisible();
  const rowAfterHover = await createdRow.boundingBox();
  expect(rowAfterHover?.height).toBe(rowBeforeHover?.height);
  await createdFile.focus();
  await expect(preview).toBeVisible();
  const previewLayout = await preview.evaluate((element) => {
    const composerDock = document.querySelector<HTMLElement>(".session-view > .composer-dock");
    if (!composerDock) throw new Error("输入框未渲染");
    return {
      attachedToBody: element.parentElement === document.body,
      previewBottom: element.getBoundingClientRect().bottom,
      composerTop: composerDock.getBoundingClientRect().top,
      clientHeight: element.clientHeight,
      scrollHeight: element.scrollHeight,
    };
  });
  expect(previewLayout.attachedToBody).toBe(true);
  expect(previewLayout.previewBottom).toBeLessThanOrEqual(previewLayout.composerTop + 1);
  expect(previewLayout.scrollHeight).toBeGreaterThan(previewLayout.clientHeight);
  await expect(footer.locator(".ft-summary .ft-add")).toHaveText("+74");
  await expect(footer.getByRole("button", { name: "复制回答" })).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/08-answer-footer-preview.png` });

  await createdFile.click();
  const panel = page.getByLabel("会话信息区", { exact: true });
  await expect(panel.getByRole("tab", { name: "Review" })).toHaveAttribute("aria-selected", "true");
  await expect(panel.getByLabel("修改文件树")).toBeVisible();
  await panel.getByRole("treeitem", { name: "styles.css" }).click();
  await expect(panel.locator(".review-file.selected")).toHaveAttribute(
    "data-review-path",
    "web/src/styles.css",
  );
  await expect(panel.getByRole("table", { name: "上下对比" }).first()).toBeVisible();
  const unifiedScrollbar = panel.getByLabel("上下对比横向滚动");
  await expect(unifiedScrollbar).toHaveCount(1);
  await expect(unifiedScrollbar).toBeVisible();
  await unifiedScrollbar.evaluate((element) => {
    element.scrollLeft = 48;
    element.dispatchEvent(new Event("scroll"));
  });
  await expect.poll(async () => panel.locator(".review-file.selected .review-code-scroll").evaluate(
    (element) => element.scrollLeft,
  )).toBeGreaterThan(0);
  const reviewViewport = panel.locator(".review-diff-scroll");
  await reviewViewport.evaluate((element) => {
    const longFile = element.querySelectorAll<HTMLElement>(".review-file")[1];
    element.scrollTop = (longFile?.offsetTop ?? 0) + 80;
  });
  await expect.poll(async () => reviewViewport.evaluate((element) => element.scrollTop))
    .toBeGreaterThan(200);
  await expect(panel.locator(".review-file.selected")).toHaveAttribute(
    "data-review-path",
    "web/src/acceptance-notes.md",
  );
  await expect(unifiedScrollbar).toBeVisible();
  const [scrollbarBox, viewportBox] = await Promise.all([
    unifiedScrollbar.boundingBox(),
    reviewViewport.boundingBox(),
  ]);
  if (!scrollbarBox || !viewportBox) throw new Error("Review 横向滚动条不可测量");
  expect(scrollbarBox.y).toBeGreaterThanOrEqual(viewportBox.y + viewportBox.height - 1);
  await unifiedScrollbar.evaluate((element) => {
    element.scrollLeft = 32;
    element.dispatchEvent(new Event("scroll"));
  });
  await expect.poll(async () => panel.locator(".review-file.selected .review-code-scroll").evaluate(
    (element) => element.scrollLeft,
  )).toBeGreaterThan(0);
  await page.screenshot({ path: `${SHOTS}/08-review-sticky-scroll.png` });
  await reviewViewport.evaluate((element) => { element.scrollTop = 0; });
  await panel.getByRole("button", { name: "左右对比" }).click();
  const splitDiff = panel.getByRole("table", { name: "左右对比" }).first();
  await expect(splitDiff).toBeVisible();
  await expect(splitDiff.locator(".review-split-pane")).toHaveCount(2);
  await page.screenshot({ path: `${SHOTS}/08-review-split.png` });
  await expect(panel.getByLabel("左侧对比横向滚动")).toBeVisible();
  await expect(panel.getByLabel("右侧对比横向滚动")).toBeVisible();
  await panel.getByRole("button", { name: "上下对比" }).click();
  await expect(panel.getByRole("table", { name: "上下对比" }).first()).toBeVisible();
  await panel.getByRole("button", { name: "全部收起" }).click();
  await expect(panel.locator(".review-file-body")).toHaveCount(0);
  await panel.getByRole("button", { name: "全部展开" }).click();
  await expect(panel.locator(".review-file-body")).toHaveCount(2);
  await page.screenshot({ path: `${SHOTS}/08-review-workspace.png` });

  await panel.getByRole("button", { name: "跳转文件" }).click();
  const jump = panel.getByRole("dialog", { name: "跳转文件" });
  await jump.getByRole("textbox", { name: "搜索文件" }).fill("styles.css");
  await expect(jump.getByRole("button", { name: /styles.css/ })).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/08-review-file-jump.png` });
  await jump.getByRole("button", { name: /styles.css/ }).click();
  await expect(panel.getByRole("treeitem", { name: "styles.css" })).toHaveAttribute(
    "aria-selected",
    "true",
  );

  await panel.getByRole("button", { name: "更多 Review 选项" }).click();
  const menu = panel.getByRole("menu", { name: "Review 选项" });
  await expect(menu.getByRole("menuitem", { name: "刷新" })).toBeVisible();
  await expect(menu.getByRole("menuitem", { name: "启用换行" })).toBeVisible();
  await expect(menu.getByRole("menuitem", { name: "加载完整文件" })).toBeVisible();
  await expect(menu.getByRole("menuitem", { name: "启用富文本预览" })).toBeVisible();
  await expect(menu.getByRole("menuitem", { name: "启用字级差异" })).toBeVisible();
  await expect(menu.getByRole("menuitem", { name: "隐藏空白改动" })).toBeVisible();
  await expect(menu.getByRole("menuitem", { name: "复制应用命令" })).toBeVisible();
  await menu.getByRole("menuitem", { name: "启用富文本预览" }).click();
  await expect(panel.getByRole("region", { name: "web/src/acceptance-notes.md 的富文本预览" })).toBeVisible();
  await panel.getByRole("button", { name: "更多 Review 选项" }).click();
  await expect(menu.getByRole("menuitem", { name: "关闭富文本预览" })).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/08-review-options.png` });
  await menu.getByRole("menuitem", { name: "启用换行" }).click();
  await expect(panel.locator(".review-unified.wrap").first()).toBeVisible();
});

test("09 空会话命令候选显示在输入框下方", async ({ page }) => {
  await openApp(page, 1440, 900);
  await page.getByLabel("输入消息", { exact: true }).fill("/");
  const popup = page.getByRole("listbox", { name: "输入候选" });
  await expect(popup).toBeVisible();
  await expect(popup).toHaveClass(/below/);
  await expect(page.getByRole("option", { name: /\/plan/ })).toBeVisible();
  await expect.poll(() => popup.locator(".composer-completion-value").evaluateAll(
    (labels) => labels.every((label) => label.scrollWidth <= label.clientWidth),
  )).toBe(true);
  const longDescription = popup.getByRole("option", { name: /subagent-driven-development/ })
    .locator(".composer-completion-description");
  await expect.poll(() => longDescription.evaluate(
    (description) => description.scrollWidth > description.clientWidth,
  )).toBe(true);
  await page.screenshot({ path: `${SHOTS}/09-completion-below.png` });
});

test("10 已有转录的文件候选显示在输入框上方", async ({ page }) => {
  await openApp(page, 1440, 900);
  await openShowcase(page);
  await page.getByLabel("输入消息", { exact: true }).fill("@");
  const popup = page.getByRole("listbox", { name: "输入候选" });
  await expect(popup).toBeVisible();
  await expect(popup).toHaveClass(/above/);
  await expect(page.getByRole("option", { name: /composer\.tsx/ })).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/10-completion-above.png` });
});

test("08b 固定摘要会为输入区域保留右侧空间", async ({ page }) => {
  await openApp(page, 1440, 900);
  await openShowcase(page);

  const composer = page.locator(".session-view > .composer-dock");
  const before = await composer.boundingBox();
  if (!before) throw new Error("输入区域未渲染");

  const toggle = page.getByRole("button", { name: "切换固定摘要" });
  await toggle.click();
  const summary = page.getByLabel("固定摘要", { exact: true });
  await expect(summary).toHaveClass(/open/);
  await expect.poll(async () => (await composer.boundingBox())?.width ?? 0).toBeLessThan(before.width);
  await expect.poll(() => page.evaluate(() => {
    const summary = document.querySelector<HTMLElement>(".fixed-summary.open");
    const dock = document.querySelector<HTMLElement>(".session-view > .composer-dock");
    if (!summary || !dock) return false;
    return dock.getBoundingClientRect().right <= summary.getBoundingClientRect().left - 8;
  })).toBe(true);
  await page.screenshot({ path: `${SHOTS}/08b-fixed-summary.png` });
});

test("08c 会话信息区支持限宽拖拽、键盘调整、全屏和关闭", async ({ page }) => {
  await openApp(page, 1440, 900);
  await openShowcase(page);

  const panel = page.getByLabel("会话信息区", { exact: true });
  const resizer = page.getByRole("separator", { name: "调整会话信息区宽度" });
  await expect(panel).toBeVisible();
  await expect(resizer).toBeVisible();
  await expect.poll(async () => (await panel.boundingBox())?.width ?? 0).toBeGreaterThan(400);
  const defaultWidth = Number(await resizer.getAttribute("aria-valuenow"));

  await resizer.focus();
  for (let index = 0; index < 40; index += 1) await resizer.press("ArrowLeft");
  await expect(resizer).toHaveAttribute("aria-valuenow", await resizer.getAttribute("aria-valuemax") ?? "");
  await expect.poll(async () => Math.round((await panel.boundingBox())?.width ?? 0))
    .toBe(Number(await resizer.getAttribute("aria-valuemax")));

  const grip = await resizer.boundingBox();
  if (!grip) throw new Error("信息区拖拽线不可测量");
  await page.mouse.move(grip.x + grip.width / 2, grip.y + 80);
  await page.mouse.down();
  await page.mouse.move(grip.x + 96, grip.y + 80, { steps: 4 });
  await page.mouse.up();
  await expect.poll(async () => Number(await resizer.getAttribute("aria-valuenow")))
    .toBeLessThan(Number(await resizer.getAttribute("aria-valuemax")));
  await resizer.focus();
  for (let index = 0; index < 40; index += 1) await resizer.press("ArrowRight");
  await expect(resizer).toHaveAttribute("aria-valuenow", String(430));

  await panel.getByRole("button", { name: "全屏显示会话信息区" }).click();
  await expect(panel).toHaveClass(/fullscreen/);
  await expect(resizer).toBeHidden();
  await expect.poll(async () => (await panel.boundingBox())?.width ?? 0).toBeGreaterThan(defaultWidth);

  await panel.getByRole("button", { name: "恢复会话信息区默认宽度" }).click();
  await expect(panel).not.toHaveClass(/fullscreen/);
  await expect.poll(async () => Math.round((await panel.boundingBox())?.width ?? 0))
    .toBe(Math.round(defaultWidth));

  await panel.getByRole("button", { name: "关闭会话信息区" }).click();
  await expect(panel).toBeHidden();
});

test("09 侧栏多工作区分组", async ({ page }) => {
  await openApp(page, 1440, 900);
  // Pinned section surfaces the waiting badge; the current workspace group is
  // expanded, the other workspace stays collapsed.
  const pinned = page.getByRole("group", { name: "已置顶" });
  await expect(pinned.getByText("有界中继测试", { exact: true })).toBeVisible();
  await expect(pinned.getByText("等待回答", { exact: true })).toBeVisible();
  const neo = page.getByRole("group", { name: "neo" });
  await expect(neo.getByRole("button", { name: /^neo/ })).toHaveAttribute("aria-expanded", "true");
  await expect(neo.getByText("长会话压测", { exact: true })).toBeVisible();
  await expect(neo.locator(".session-list").first()).toHaveCSS("padding-left", "8px");
  await expect(neo.locator(".session-time")).toHaveCount(0);
  await expect(neo.getByRole("button", { name: "长会话压测" })).toHaveAttribute(
    "title",
    /更新时间：/,
  );
  await expect(neo.locator(".lucide-folder-open")).toBeVisible();
  const playground = page.getByRole("group", { name: "playground" });
  await expect(
    playground.getByRole("button", { name: /^playground/ }),
  ).toHaveAttribute("aria-expanded", "false");
  await expect(playground.getByText("原型脚本调试", { exact: true })).toBeHidden();
  await page.screenshot({ path: `${SHOTS}/09-sidebar-workspace-groups.png` });
});

test("10 composer 模型 pill 覆盖层", async ({ page }) => {
  await openApp(page, 1440, 900);
  await page.getByRole("button", { name: "模型与推理（仅下一回合）" }).click();
  const overlay = page.getByRole("dialog", { name: "选择模型与推理" });
  await expect(overlay).toBeVisible();
  const box = await overlay.boundingBox();
  if (!box) throw new Error("model menu has no box");
  const paintedDialog = await page.evaluate(
    ([x, y]) => document.elementFromPoint(x, y)?.closest('[role="dialog"]')?.getAttribute("aria-label"),
    [box.x + box.width / 2, box.y + box.height / 2] as const,
  );
  expect(paintedDialog).toBe("选择模型与推理");
  await overlay.getByRole("button", { name: /^模型/ }).click();
  await expect(overlay.getByLabel("搜索模型")).toBeVisible();
  await expect(overlay.getByRole("option", { name: /gpt-5-codex openai/ })).toBeVisible();
  await expect(overlay.getByRole("option", { name: /claude-sonnet-4.5/ })).toBeVisible();
  await expect(overlay.getByRole("option", { name: /kimi-k2/ })).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/10-composer-model-pill-overlay.png` });

  await page.setViewportSize({ width: 375, height: 800 });
  await expect(overlay.locator(".model-settings-popover")).toBeHidden();
  const narrowBox = await overlay.locator(".model-submenu").boundingBox();
  if (!narrowBox) throw new Error("narrow model menu has no box");
  expect(narrowBox.x).toBeGreaterThanOrEqual(0);
  expect(narrowBox.x + narrowBox.width).toBeLessThanOrEqual(375);
  await page.screenshot({ path: `${SHOTS}/10b-composer-model-pill-narrow.png` });

  await page.setViewportSize({ width: 1440, height: 900 });
  await overlay.getByRole("option", { name: /gpt-5-codex openai/ }).click();
  await overlay.getByRole("button", { name: /^推理强度/ }).click();
  await expect(overlay.getByRole("option", { name: "极高" })).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/10c-composer-reasoning-overlay.png` });
});

test("11 composer 附件队列", async ({ page }) => {
  await openApp(page, 1440, 900);
  // 1x1 transparent PNG.
  const png = Buffer.from(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==",
    "base64",
  );
  await page.getByLabel("选择附件文件").setInputFiles([
    { name: "ui-mock.png", mimeType: "image/png", buffer: png },
    { name: "验收笔记.txt", mimeType: "text/plain", buffer: Buffer.from("验收笔记内容") },
  ]);
  const queue = page.locator(".attachment-queue");
  await expect(queue.locator(".attachment-chip", { hasText: "ui-mock.png" })).toHaveAttribute(
    "data-status",
    "ready",
  );
  await expect(queue.locator(".attachment-chip", { hasText: "验收笔记.txt" })).toHaveAttribute(
    "data-status",
    "ready",
  );
  await page.screenshot({ path: `${SHOTS}/11-composer-attachment-queue.png` });
});

test("12 右键菜单", async ({ page }) => {
  await openApp(page, 1440, 900);
  const row = page.locator(".session-row", { hasText: "并行格式化" });
  await row.click({ button: "right" });
  await expect(page.getByRole("menu", { name: "会话操作" })).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/12-context-menu.png` });
});

test("13 窄桌面抽屉", async ({ page }) => {
  await openApp(page, 900, 780);
  await page.getByRole("button", { name: "打开会话列表" }).click();
  await expect(page.getByText("并行格式化", { exact: true })).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/13-narrow-desktop-drawer.png` });
});

test("顶栏在宽窄屏之间保持侧栏与抽屉状态独立", async ({ page }) => {
  await openApp(page, 1440, 900);
  const list = sidebar(page);
  await page.getByRole("button", { name: "收起会话列表" }).click();
  await expect(list).toHaveClass(/sidebar-collapsed/);

  await page.setViewportSize({ width: 900, height: 780 });
  await page.getByRole("button", { name: "打开会话列表" }).click();
  await expect(list).toHaveClass(/drawer-open/);

  await page.setViewportSize({ width: 1440, height: 900 });
  await page.getByRole("button", { name: "展开会话列表" }).click();
  await expect(list).not.toHaveClass(/sidebar-collapsed/);
});

test("14 手机单列", async ({ page }) => {
  await openApp(page, 390, 844);
  await page.getByRole("button", { name: "打开会话列表" }).click();
  const list = sidebar(page);
  await list.getByText("有界中继测试", { exact: true }).click();
  await expect(list).not.toHaveClass(/drawer-open/);
  await expect(list).toHaveCSS("visibility", "hidden");
  await expect(list).toHaveCSS("pointer-events", "none");
  await expect(
    page.getByText("42 个行为测试全部通过，慢连接现在会先收到 1013 再被注销。"),
  ).toBeVisible();

  await page.getByRole("button", { name: "打开会话列表" }).click();
  await expect(list).toHaveClass(/drawer-open/);
  await page.getByRole("button", { name: "关闭会话列表" }).click();
  await expect(list).not.toHaveClass(/drawer-close-immediate/);
  expect(await list.evaluate((element) => getComputedStyle(element).transitionDuration)).not.toBe(
    "0s",
  );
  await page.screenshot({ path: `${SHOTS}/14-mobile-single-column.png` });

  const informationToggle = page.getByRole("button", { name: "切换固定摘要" });
  await informationToggle.click();
  const panel = page.getByLabel("会话信息区", { exact: true });
  await expect(panel).toBeVisible();
  await expect(panel).toHaveCSS("width", "390px");
  await expect(panel.getByRole("tab", { name: "Subagents" })).toBeVisible();
  await expect(panel.getByRole("tab", { name: "Review" })).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/14-mobile-information-panel.png` });
  await panel.getByRole("button", { name: "关闭会话信息区" }).click();
  await expect(panel).toBeHidden();
  await expect(informationToggle).toBeFocused();
});

test("15 亮色：运行会话", async ({ page }) => {
  await openApp(page, 1440, 900);
  await switchToLight(page);
  await openShowcase(page);
  await expect(
    page.getByRole("button", { name: /展开工作过程（.*个步骤）/ }),
  ).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/15-light-running-session.png` });
});

test("16 亮色：侧栏分组", async ({ page }) => {
  await openApp(page, 1440, 900);
  await switchToLight(page);
  await expect(page.getByRole("group", { name: "已置顶" })).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/16-light-sidebar-groups.png` });
});

test("17 亮色：composer pill 覆盖层", async ({ page }) => {
  await openApp(page, 1440, 900);
  await switchToLight(page);
  await page.getByRole("button", { name: "模型与推理（仅下一回合）" }).click();
  await expect(page.getByRole("dialog", { name: "选择模型与推理" })).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/17-light-composer-pill.png` });
});

test("18 亮色：agent 详情面板", async ({ page }) => {
  await openApp(page, 1440, 900);
  await switchToLight(page);
  await openShowcase(page);
  await page.getByRole("button", { name: /查看子代理详情：检查测试覆盖/ }).click();
  const panel = page.getByLabel("会话信息区", { exact: true });
  await expect(panel).toBeVisible();
  await expect(
    panel.getByText("relay 测试覆盖良好：慢连接注销与 1013 关闭都有行为测试。"),
  ).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/18-light-agent-panel.png` });
});

// ---------------------------------------------------------------------------
// 性能验收（redesign §9）: 10 万字符会话 + 离屏渲染跳过 + 拖拽无长任务。
// ---------------------------------------------------------------------------

test("19 十万字符会话：content-visibility 离屏跳过渲染", async ({ page }) => {
  await openApp(page, 1440, 900);
  await page.getByText("长会话压测", { exact: true }).click();
  await expect(page.getByText("（结尾标记）", { exact: false })).toBeVisible();

  const probe = await page.evaluate(() => {
    const column = document.querySelector(".transcript-column");
    const items = [...document.querySelectorAll<HTMLElement>(".t-item")];
    const viewportH = window.innerHeight;
    let offscreenTotal = 0;
    let offscreenSkipped = 0;
    let onscreenRendered = 0;
    for (const item of items) {
      const rect = item.getBoundingClientRect();
      const offscreen = rect.bottom < 0 || rect.top > viewportH;
      // checkVisibility with contentVisibilityAuto is the designed probe:
      // an element inside a skipped content-visibility subtree reports not
      // visible even though the DOM node still exists.
      const child = item.firstElementChild;
      const rendered =
        child instanceof HTMLElement
          ? child.checkVisibility({ contentVisibilityAuto: true })
          : false;
      if (offscreen) {
        offscreenTotal += 1;
        if (!rendered) offscreenSkipped += 1;
      } else if (rendered) {
        onscreenRendered += 1;
      }
    }
    return {
      textLength: column?.textContent?.length ?? 0,
      itemCount: items.length,
      contentVisibility: items.length > 0 ? getComputedStyle(items[0]).contentVisibility : "",
      offscreenTotal,
      offscreenSkipped,
      onscreenRendered,
    };
  });

  expect(probe.textLength).toBeGreaterThan(100_000);
  expect(probe.contentVisibility).toBe("auto");
  expect(probe.itemCount).toBeGreaterThan(100);
  // The transcript is pinned to the bottom: the many items scrolled past
  // above must have skipped subtrees (zero-layout children).
  expect(probe.offscreenTotal).toBeGreaterThan(50);
  expect(probe.offscreenSkipped).toBeGreaterThan(probe.offscreenTotal * 0.9);
  expect(probe.onscreenRendered).toBeGreaterThan(0);
});

test("20 侧栏拖拽：无 >100ms 长任务且宽度收敛", async ({ page }) => {
  await openApp(page, 1440, 900);
  await page.getByText("长会话压测", { exact: true }).click();
  await expect(page.getByText("（结尾标记）", { exact: false })).toBeVisible();

  await page.evaluate(() => {
    (window as unknown as { __longtasks: Array<{ start: number; duration: number }> }).__longtasks =
      [];
    new PerformanceObserver((list) => {
      const sink = (window as unknown as { __longtasks: Array<{ start: number; duration: number }> })
        .__longtasks;
      for (const entry of list.getEntries()) {
        sink.push({ start: entry.startTime, duration: entry.duration });
      }
    }).observe({ entryTypes: ["longtask"] });
  });

  const resizer = page.getByRole("separator", { name: "调整会话列表宽度" });
  const before = Number(await resizer.getAttribute("aria-valuenow"));
  const box = await resizer.boundingBox();
  if (!box) throw new Error("resizer not measurable");
  const startX = box.x + box.width / 2;
  const startY = box.y + box.height / 2;

  const t0 = await page.evaluate(() => performance.now());
  await page.mouse.move(startX, startY);
  await page.mouse.down();
  // ~20 frames of movement, 5px per step (stays inside the min/max clamp).
  for (let step = 1; step <= 20; step += 1) {
    await page.mouse.move(startX + step * 5, startY);
  }
  await page.mouse.up();
  const t1 = await page.evaluate(() => performance.now());

  const after = Number(await resizer.getAttribute("aria-valuenow"));
  expect(after).toBe(before + 100);

  // Let the observer flush its pending entries.
  await page.waitForTimeout(100);
  const longtasks = await page.evaluate(
    ([from, to]) =>
      (window as unknown as { __longtasks: Array<{ start: number; duration: number }> }).__longtasks.filter(
        (entry) => entry.start >= from && entry.start <= to,
      ),
    [t0, t1] as const,
  );
  const blocking = longtasks.filter((entry) => entry.duration > 100);
  expect(
    blocking,
    `drag window had ${blocking.length} long task(s) >100ms: ${JSON.stringify(blocking)}`,
  ).toHaveLength(0);
});
