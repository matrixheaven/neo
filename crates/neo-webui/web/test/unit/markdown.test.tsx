import { render, screen } from "@testing-library/react";
import React from "react";
import { describe, expect, it } from "vitest";
import { Markdown } from "../../src/components/markdown";

describe("Markdown", () => {
  it("renders tables, lists, and fenced code blocks as semantic elements", () => {
    const { container } = render(
      React.createElement(Markdown, {
        text: [
          "| 文件 | 状态 |",
          "| --- | --- |",
          "| src/app.ts | 已修改 |",
          "",
          "- 第一项",
          "- 第二项",
          "",
          "```ts",
          "const value = 1;",
          "```",
        ].join("\n"),
      }),
    );

    expect(container.querySelector("table")).not.toBeNull();
    expect(screen.getByRole("columnheader", { name: "文件" })).toBeTruthy();
    expect(screen.getByRole("cell", { name: "src/app.ts" })).toBeTruthy();
    expect(container.querySelectorAll("ul > li")).toHaveLength(2);
    expect(screen.getByText("第一项")).toBeTruthy();
    expect(screen.getByText("第二项")).toBeTruthy();
    expect(container.querySelector("pre > code")?.textContent).toContain("const value = 1;");
  });
});
