/**
 * Auth flow: the address fragment is exchanged exactly once and cleared via
 * history.replaceState; the token never reaches storage, the console or the
 * page. Missing or rejected tokens show a non-sensitive prompt.
 */

import { cleanup, render, screen, waitFor } from "@testing-library/react";
import React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "../../src/app";
import { AppProvider } from "../../src/state/store";
import { FakeWebSocket, fixture, mockFetch, recordedRequests, resetHarness } from "./harness";
import { asServerMessage } from "./fixture";

function renderApp() {
  return render(
    React.createElement(
      AppProvider,
      null,
      React.createElement(App),
    ),
  );
}

describe("access token claim", () => {
  beforeEach(() => {
    resetHarness();
    vi.stubGlobal("fetch", vi.fn(mockFetch));
    vi.stubGlobal("WebSocket", FakeWebSocket);
    window.history.replaceState(null, "", "/#access=test-token-123");
  });

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it("claims the fragment once and clears it with replaceState", async () => {
    renderApp();
    await screen.findByLabelText("会话列表");

    const claims = recordedRequests.filter((entry) => entry.url === "/api/auth/claim");
    expect(claims).toHaveLength(1);
    expect(claims[0].method).toBe("POST");
    expect(claims[0].body).toEqual({ token: "test-token-123" });
    expect(window.location.hash).toBe("");

    // The token never reaches storage or the rendered page.
    expect(window.localStorage.getItem("neo-webui.sidebar-width")).toBeNull();
    expect(window.localStorage.length).toBe(0);
    expect(document.body.textContent ?? "").not.toContain("test-token-123");
  });

  it("shows a non-sensitive prompt when the claim fails", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
        const url = typeof input === "string" ? input : input.toString();
        if (url === "/api/auth/claim") {
          return Promise.resolve(
            new Response(JSON.stringify({ code: "unauthorized" }), {
              status: 401,
              headers: { "content-type": "application/json" },
            }),
          );
        }
        return mockFetch(input, init);
      }),
    );
    renderApp();
    await screen.findByRole("alert");
    expect(document.body.textContent ?? "").toContain("访问链接无效或已过期");
    expect(document.body.textContent ?? "").not.toContain("test-token-123");
    expect(window.location.hash).toBe("");
  });

  it("subscribes the workspace and keeps it across session switches", async () => {
    renderApp();
    await screen.findByLabelText("会话列表");
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    const socket = FakeWebSocket.instances[0];
    await waitFor(() =>
      expect(socket.sent.map((data) => (JSON.parse(data) as { type: string }).type)).toContain(
        "watch_workspace",
      ),
    );
    // Selecting a session adds one watch_session; the workspace subscription
    // is never re-sent or cancelled.
    socket.emit(asServerMessage(fixture.long_connection.workspace_snapshot));
    const sessionButton = await screen.findByText("有界中继测试");
    sessionButton.click();
    await waitFor(() => expect(socket.watchSessionIds()).toEqual(["session_0001"]));
    const watchWorkspaceCount = socket.sent.filter(
      (data) => (JSON.parse(data) as { type: string }).type === "watch_workspace",
    );
    expect(watchWorkspaceCount).toHaveLength(1);
  });
});
