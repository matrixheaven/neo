import { cleanup, fireEvent, render, within } from "@testing-library/react";
import React from "react";
import { afterEach, describe, expect, it } from "vitest";
import {
  PLAN_PREVIEW_LINES,
  PlanBox,
  planPreviewClamp,
} from "../../src/components/planBox";

function planLines(lineCount: number): string {
  return Array.from(
    { length: lineCount },
    (_, index) => `- 步骤 ${index + 1}`,
  ).join("\n");
}

describe("PlanBox", () => {
  afterEach(cleanup);

  it("renders a short plan fully without any expand affordance", () => {
    const { container } = render(
      React.createElement(PlanBox, {
        markdown: planLines(PLAN_PREVIEW_LINES),
        path: "docs/plan.md",
      }),
    );

    expect(within(container).getByText("步骤 10")).toBeTruthy();
    expect(container.querySelector(".plan-box-expand")).toBeNull();
    expect(container.querySelector(".plan-box-collapse")).toBeNull();
    expect(container.querySelector(".plan-box-path")?.textContent).toBe("docs/plan.md");
  });

  it("clamps a long plan to the preview budget and expands via 显示全部", () => {
    const { container } = render(
      React.createElement(PlanBox, {
        markdown: planLines(PLAN_PREVIEW_LINES + 5),
        title: "plan: feature.md",
      }),
    );

    expect(within(container).getByText("步骤 10")).toBeTruthy();
    expect(within(container).queryByText("步骤 11")).toBeNull();
    expect(planPreviewClamp(planLines(PLAN_PREVIEW_LINES + 5))?.split("\n")).toHaveLength(
      PLAN_PREVIEW_LINES,
    );

    fireEvent.click(within(container).getByRole("button", { name: "显示全部" }));
    expect(within(container).getByText("步骤 15")).toBeTruthy();
    const collapse = container.querySelector<HTMLButtonElement>(".plan-box-collapse");
    expect(collapse).not.toBeNull();

    fireEvent.click(collapse as HTMLButtonElement);
    expect(within(container).queryByText("步骤 15")).toBeNull();
    expect(within(container).getByRole("button", { name: "显示全部" })).toBeTruthy();
  });
});
