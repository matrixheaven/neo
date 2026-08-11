/**
 * Composer redesign (R6) tests: attachment queue (enqueue / remove /
 * over-limit rejection / upload failure keeps draft / send carries ids),
 * two-level model selection, explicit per-turn permission and development
 * menus, ContextRing numbers and no-data hiding, and the welcome banner
 * showing only until the first canonical user message.
 */

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "../../src/app";
import { AppProvider } from "../../src/state/store";
import {
  FakeWebSocket,
  fixture,
  mockFetch,
  recordedRequests,
  resetHarness,
} from "./harness";
import { asServerMessage } from "./fixture";

function renderApp() {
  return render(React.createElement(AppProvider, null, React.createElement(App)));
}

async function renderReady() {
  const utils = renderApp();
  await screen.findByLabelText("会话列表");
  await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
  return { ...utils, socket: FakeWebSocket.instances[0] };
}

function fileInput(): HTMLInputElement {
  return screen.getByLabelText("选择附件文件") as HTMLInputElement;
}

function addFiles(files: File[]) {
  fireEvent.change(fileInput(), { target: { files } });
}

function textFile(name: string, bytes: number, type = "text/plain"): File {
  return new File(["x".repeat(bytes)], name, { type });
}

function chipStatus(name: string): string | null {
  const chip = screen.getByText(name).closest(".attachment-chip");
  return chip === null ? null : chip.getAttribute("data-status");
}

async function waitUploads(count: number) {
  await waitFor(() =>
    expect(
      recordedRequests.filter((entry) => entry.url === "/api/attachments"),
    ).toHaveLength(count),
  );
}

function postedBodies(url: string): Array<Record<string, unknown>> {
  return recordedRequests
    .filter((entry) => entry.url === url && entry.method === "POST")
    .map((entry) => entry.body as Record<string, unknown>);
}

/** Idle session_0002 snapshot with no user message and the given state extras. */
function bareSnapshot(session: Record<string, unknown>) {
  return {
    stream_id: fixture.stream_id,
    session_id: "session_0002",
    watermark: 0,
    session: {
      phase: "idle",
      waiting_approval: false,
      waiting_question: false,
      current_turn_id: null,
      ...session,
    },
    metadata: {},
    history: [],
  };
}

describe("composer R6", () => {
  beforeEach(() => {
    resetHarness();
    vi.stubGlobal("fetch", vi.fn(mockFetch));
    vi.stubGlobal("WebSocket", FakeWebSocket);
    window.history.replaceState(null, "", "/");
  });

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it("enqueues attachments, uploads them and removes chips", async () => {
    const user = userEvent.setup();
    await renderReady();
    addFiles([textFile("notes.txt", 16)]);
    await screen.findByText("notes.txt");
    await waitUploads(1);
    await waitFor(() => expect(chipStatus("notes.txt")).toBe("ready"));
    expect(recordedRequests[recordedRequests.length - 1].body).toMatchObject({
      mime: "text/plain",
    });

    await user.click(screen.getByRole("button", { name: "移除附件 notes.txt" }));
    expect(screen.queryByText("notes.txt")).toBeNull();
  });

  it("rejects the fifth attachment and files over 8MiB with a non-sensitive hint", async () => {
    const user = userEvent.setup();
    await renderReady();
    addFiles([
      textFile("a.txt", 8),
      textFile("b.txt", 8),
      textFile("c.txt", 8),
      textFile("d.txt", 8),
      textFile("e.txt", 8),
    ]);
    await screen.findByText("最多 4 个附件。");
    await waitUploads(4);
    expect(screen.queryByText("e.txt")).toBeNull();

    // Free a slot, then the oversize file hits the 8MiB pre-check.
    await user.click(screen.getByRole("button", { name: "移除附件 a.txt" }));
    addFiles([textFile("huge.bin", 9 * 1024 * 1024, "application/octet-stream")]);
    await screen.findByText(/超过 8MiB 上限/);
    // The oversize file never entered the queue and was never uploaded.
    expect(screen.queryByText("huge.bin")).toBeNull();
    await waitUploads(4);
  });

  it("keeps the draft and flags the chip when the upload fails", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
        const url = typeof input === "string" ? input : input.toString();
        if (url === "/api/attachments") {
          recordedRequests.push({
            method: "POST",
            url,
            body: init?.body ? JSON.parse(String(init.body)) : undefined,
          });
          return Promise.resolve(
            new Response(JSON.stringify({ code: "too_large" }), {
              status: 413,
              headers: { "content-type": "application/json" },
            }),
          );
        }
        return mockFetch(input, init);
      }),
    );
    const user = userEvent.setup();
    await renderReady();
    const input = screen.getByLabelText("输入消息") as HTMLTextAreaElement;
    await user.type(input, "这段草稿不能丢");

    addFiles([textFile("broken.png", 32, "image/png")]);
    await screen.findByText("broken.png");
    await waitUploads(1);
    await screen.findByText("附件上传失败，请重试或移除。");
    await waitFor(() => expect(chipStatus("broken.png")).toBe("error"));
    expect(input.value).toBe("这段草稿不能丢");
  });

  it("sends ready attachment ids with the create body and clears the queue", async () => {
    const user = userEvent.setup();
    await renderReady();
    addFiles([textFile("diagram.png", 24, "image/png")]);
    await screen.findByText("diagram.png");
    await waitUploads(1);
    await waitFor(() => expect(chipStatus("diagram.png")).toBe("ready"));

    await user.type(screen.getByLabelText("输入消息"), "看图说话{Enter}");
    await waitFor(() => {
      const creates = postedBodies("/api/sessions");
      expect(creates).toHaveLength(1);
      expect(creates[0].attachments).toEqual(["att_1"]);
    });
    // Queue cleared after the create succeeded.
    await waitFor(() => expect(screen.queryByText("diagram.png")).toBeNull());
  });

  it("searches the grouped model submenu and writes the choice into the composer field", async () => {
    const user = userEvent.setup();
    await renderReady();
    await user.click(screen.getByRole("button", { name: "模型与推理（仅下一回合）" }));
    const dialog = screen.getByRole("dialog", { name: "选择模型与推理" });
    await user.click(within(dialog).getByRole("button", { name: /^模型/ }));
    const search = await screen.findByLabelText("搜索模型");
    screen.getByText("openai · 256k");

    await user.type(search, "claude");
    expect(screen.queryByText("openai · 256k")).toBeNull();
    await user.click(screen.getByRole("option", { name: /claude-sonnet/ }));
    expect(
      screen.getByRole("button", { name: "模型与推理（仅下一回合）" }).textContent,
    ).toContain("claude-sonnet");
    expect(within(dialog).getByText("不支持")).toBeTruthy();

    await user.type(screen.getByLabelText("输入消息"), "换个模型试试{Enter}");
    await waitFor(() => {
      const creates = postedBodies("/api/sessions");
      expect(creates).toHaveLength(1);
      expect((creates[0].composer as { model?: string }).model).toBe("claude-sonnet");
    });
  });

  it("opens the full model list and selects a model with the keyboard", async () => {
    const user = userEvent.setup();
    await renderReady();
    const pill = screen.getByRole("button", { name: "模型与推理（仅下一回合）" });

    pill.focus();
    await user.keyboard("{Enter}");
    const dialog = await screen.findByRole("dialog", { name: "选择模型与推理" });
    const modelRow = within(dialog).getByRole("button", { name: /^模型/ });
    modelRow.focus();
    await user.keyboard("{Enter}");

    const search = await screen.findByLabelText("搜索模型");
    expect(document.activeElement).toBe(search);
    expect(screen.getByRole("button", { name: "返回" })).toBeTruthy();

    const model = screen.getByRole("option", { name: /claude-sonnet/ });
    model.focus();
    await user.keyboard("{Enter}");
    expect(screen.getByRole("dialog", { name: "选择模型与推理" })).toBeTruthy();
    expect(pill.textContent).toContain("claude-sonnet");
  });

  it("closes each per-turn menu on Escape, restores focus, and closes on outside click", async () => {
    const user = userEvent.setup();
    await renderReady();
    for (const [buttonName, dialogName] of [
      ["模型与推理（仅下一回合）", "选择模型与推理"],
      ["权限模式（仅下一回合）", "选择权限模式"],
      ["开发模式（仅下一回合）", "选择开发模式"],
    ]) {
      const pill = screen.getByRole("button", { name: buttonName });
      await user.click(pill);
      await screen.findByRole("dialog", { name: dialogName });
      await user.keyboard("{Escape}");
      await waitFor(() => expect(screen.queryByRole("dialog", { name: dialogName })).toBeNull());
      expect(document.activeElement).toBe(pill);
    }

    await user.click(screen.getByRole("button", { name: "模型与推理（仅下一回合）" }));
    await screen.findByRole("dialog", { name: "选择模型与推理" });
    await user.click(screen.getByLabelText("输入消息"));
    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: "选择模型与推理" })).toBeNull(),
    );
    // Outside click leaves focus where the user clicked.
    expect(document.activeElement).toBe(screen.getByLabelText("输入消息"));
  });

  it("offers only the selected model reasoning values and sends the structured choice", async () => {
    const user = userEvent.setup();
    await renderReady();
    await user.click(screen.getByRole("button", { name: "模型与推理（仅下一回合）" }));
    const dialog = screen.getByRole("dialog", { name: "选择模型与推理" });
    await user.click(within(dialog).getByRole("button", { name: /^推理强度/ }));
    await user.click(screen.getByRole("option", { name: "极高" }));
    await user.type(screen.getByLabelText("输入消息"), "带推理{Enter}");
    await waitFor(() => {
      const creates = postedBodies("/api/sessions");
      expect((creates[0].composer as { reasoning?: unknown }).reasoning).toEqual({
        mode: "effort",
        effort: "xhigh",
      });
    });
  });

  it("accepts a model-bounded custom reasoning budget", async () => {
    const user = userEvent.setup();
    await renderReady();
    await user.click(screen.getByRole("button", { name: "模型与推理（仅下一回合）" }));
    const dialog = screen.getByRole("dialog", { name: "选择模型与推理" });
    await user.click(within(dialog).getByRole("button", { name: /^模型/ }));
    await user.click(screen.getByRole("option", { name: /deepseek-reasoner/ }));
    await user.click(within(dialog).getByRole("button", { name: /^推理强度/ }));
    await user.type(screen.getByLabelText("自定义推理预算"), "12000");
    await user.click(screen.getByRole("button", { name: "应用" }));
    await user.type(screen.getByLabelText("输入消息"), "预算推理{Enter}");
    await waitFor(() => {
      const creates = postedBodies("/api/sessions");
      expect((creates[0].composer as { reasoning?: unknown }).reasoning).toEqual({
        mode: "budget_tokens",
        budget_tokens: 12000,
      });
    });
  });

  it("opens permission and development menus without adding overrides", async () => {
    const user = userEvent.setup();
    await renderReady();
    const permission = screen.getByRole("button", { name: "权限模式（仅下一回合）" });
    const development = screen.getByRole("button", { name: "开发模式（仅下一回合）" });

    await user.click(permission);
    await screen.findByRole("option", { name: /默认/ });
    expect(permission.getAttribute("data-mode")).toBe("default");
    await user.keyboard("{Escape}");

    await user.click(development);
    await screen.findByRole("option", { name: /默认/ });
    expect(development.getAttribute("data-active")).toBe("false");
    await user.click(screen.getByLabelText("输入消息"));
    await user.type(screen.getByLabelText("输入消息"), "默认发送{Enter}");
    await waitFor(() => {
      const creates = postedBodies("/api/sessions");
      expect(creates).toHaveLength(1);
      expect(creates[0].composer).toBeUndefined();
    });
  });

  it("sends selected permission and development mode overrides", async () => {
    const user = userEvent.setup();
    await renderReady();

    await user.click(screen.getByRole("button", { name: "权限模式（仅下一回合）" }));
    await user.click(await screen.findByRole("option", { name: "自动" }));
    await user.click(screen.getByRole("button", { name: "开发模式（仅下一回合）" }));
    await user.click(await screen.findByRole("option", { name: "计划" }));
    await user.type(screen.getByLabelText("输入消息"), "按计划自动运行{Enter}");
    await waitFor(() => {
      const creates = postedBodies("/api/sessions");
      expect(creates).toHaveLength(1);
      expect(creates[0].composer).toMatchObject({
        permission_mode: "auto",
        development_mode: "plan",
      });
    });
  });

  it("does not send a model override after returning to the default model", async () => {
    const user = userEvent.setup();
    await renderReady();

    await user.click(screen.getByRole("button", { name: "模型与推理（仅下一回合）" }));
    const dialog = screen.getByRole("dialog", { name: "选择模型与推理" });
    await user.click(within(dialog).getByRole("button", { name: /^模型/ }));
    await user.click(await screen.findByRole("option", { name: /gpt-5-codex openai/ }));
    await user.click(within(dialog).getByRole("button", { name: /^模型/ }));
    await user.click(await screen.findByRole("option", { name: /跟随会话配置/ }));
    await user.type(screen.getByLabelText("输入消息"), "使用默认模型{Enter}");
    await waitFor(() => {
      const creates = postedBodies("/api/sessions");
      expect(creates).toHaveLength(1);
      expect(creates[0].composer).toBeUndefined();
    });
  });

  it("renders the ContextRing with usage numbers and hides it without data", async () => {
    const { socket } = await renderReady();
    // New-session composer has no usage data: no ring.
    expect(screen.queryByRole("img", { name: /上下文占用/ })).toBeNull();

    socket.emit(asServerMessage(fixture.long_connection.workspace_snapshot));
    screen.getByText("并行格式化").click();
    socket.emit({
      type: "session_snapshot",
      snapshot: bareSnapshot({
        context_window: { used_tokens: 83700, max_tokens: 256000 },
      }),
    } as never);

    const ring = await screen.findByRole("img", { name: "上下文占用 33%" });
    expect(ring.getAttribute("title")).toBe("83.7k / 256k tokens (33%)");
    expect(ring.textContent).toContain("33%");
  });

  it("shows the welcome banner only until the first canonical user message", async () => {
    const { socket } = await renderReady();
    // Centered new session: banner visible above the input.
    await screen.findByText(/描述你的任务/);

    socket.emit(asServerMessage(fixture.long_connection.workspace_snapshot));
    screen.getByText("并行格式化").click();
    socket.emit({ type: "session_snapshot", snapshot: bareSnapshot({}) } as never);
    // A session without any canonical user message still shows the banner.
    await screen.findByText(/描述你的任务/);

    socket.emit({
      type: "session_event",
      stream_id: fixture.stream_id,
      session_id: "session_0002",
      sequence: 1,
      event: {
        MessageAppended: {
          message: { User: { content: [{ Text: { text: "第一条用户消息" } }] } },
        },
      },
    } as never);
    await waitFor(() => expect(screen.queryByText(/描述你的任务/)).toBeNull());
  });

  it("shows slash and file candidates below an empty composer and inserts them", async () => {
    const user = userEvent.setup();
    await renderReady();
    const input = screen.getByLabelText("输入消息") as HTMLTextAreaElement;

    await user.type(input, "/");
    const popup = await screen.findByRole("listbox", { name: "输入候选" });
    expect(popup.className).toContain("below");
    await user.keyboard("{ArrowDown}{Enter}");
    expect(input.value).toBe("/plan");

    await user.clear(input);
    await user.type(input, "@");
    await screen.findByRole("option", { name: /@\[src\/main\.rs\]/ });
    await user.click(screen.getByRole("option", { name: /@\[src\/main\.rs\]/ }));
    expect(input.value).toBe("@[src/main.rs]");
  });

  it("places the completion popup above a composer with transcript", async () => {
    const user = userEvent.setup();
    const { socket } = await renderReady();
    socket.emit(asServerMessage(fixture.long_connection.workspace_snapshot));
    await user.click(screen.getByText("并行格式化"));
    socket.emit({ type: "session_snapshot", snapshot: fixture.sessions[1].snapshot } as never);
    const input = await screen.findByLabelText("输入消息");
    await user.type(input, "/");
    const popup = await screen.findByRole("listbox", { name: "输入候选" });
    expect(popup.className).toContain("above");
  });
});
