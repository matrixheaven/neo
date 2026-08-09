/**
 * Browser verification against the test-only mock server replaying the fixed
 * sample. Produces the six required screenshots under screenshots/.
 */

import { test, expect, type Page } from "@playwright/test";

const SHOTS = "screenshots";

async function openApp(page: Page, width: number, height: number) {
  await page.setViewportSize({ width, height });
  await page.goto("/#access=playwright-verification");
  // The fragment is claimed once and cleared.
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByLabel("会话列表", { exact: true })).toBeVisible();
}

async function openRunningSession(page: Page) {
  await page.getByText("有界中继测试", { exact: true }).click();
  // Full sample replay: transcript up to the appended assistant message.
  await expect(
    page.getByText("42 个行为测试全部通过，慢连接现在会先收到 1013 再被注销。"),
  ).toBeVisible();
}

test("宽屏新会话", async ({ page }) => {
  await openApp(page, 1440, 900);
  await expect(page.getByLabel("输入消息", { exact: true })).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/01-wide-new-session.png` });
});

test("宽屏运行会话与展开任务清单", async ({ page }) => {
  await openApp(page, 1440, 900);
  await openRunningSession(page);
  await page.getByRole("button", { name: "展开任务清单" }).click();
  await expect(page.getByText("编写行为测试").nth(1)).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/02-wide-running-session-tasks.png`, fullPage: false });
});

test("窄桌面抽屉", async ({ page }) => {
  await openApp(page, 900, 780);
  await page.getByRole("button", { name: "打开会话列表" }).click();
  await expect(page.getByText("并行格式化", { exact: true })).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/03-narrow-desktop-drawer.png` });
});

test("手机单列", async ({ page }) => {
  await openApp(page, 390, 844);
  await page.getByRole("button", { name: "打开会话列表" }).click();
  await page.getByText("有界中继测试", { exact: true }).click();
  await expect(
    page.getByText("42 个行为测试全部通过，慢连接现在会先收到 1013 再被注销。"),
  ).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/04-mobile-single-column.png` });
});

test("思考与工具展开", async ({ page }) => {
  await openApp(page, 1440, 900);
  await openRunningSession(page);
  await page.getByRole("button", { name: /展开思考/ }).click();
  await expect(page.getByText("先检查有界中继的边界条件。")).toBeVisible();
  await page.getByRole("button", { name: /展开工具 bash/ }).click();
  await expect(page.getByText("test result: ok. 42 passed")).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/05-expanded-thinking-tool.png` });
});

test("右键菜单", async ({ page }) => {
  await openApp(page, 1440, 900);
  const row = page.locator(".session-row", { hasText: "并行格式化" });
  await row.click({ button: "right" });
  await expect(page.getByRole("menu", { name: "会话操作" })).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/06-context-menu.png` });
});
