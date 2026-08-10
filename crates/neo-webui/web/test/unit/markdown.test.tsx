import { render, screen } from "@testing-library/react";
import React from "react";
import { describe, expect, it } from "vitest";
import { Markdown } from "../../src/components/markdown";

describe("Markdown", () => {
  it("renders GitHub-style tables as semantic table elements", () => {
    const { container } = render(
      React.createElement(Markdown, {
        text: "| 文件 | 状态 |\n| --- | --- |\n| src/app.ts | 已修改 |",
      }),
    );

    expect(container.querySelector("table")).not.toBeNull();
    expect(screen.getByRole("columnheader", { name: "文件" })).toBeTruthy();
    expect(screen.getByRole("cell", { name: "src/app.ts" })).toBeTruthy();
  });
});
