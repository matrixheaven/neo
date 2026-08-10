import { render } from "@testing-library/react";
import React from "react";
import { describe, expect, it } from "vitest";
import { CodeBlock } from "../../src/components/codeBlock";

describe("CodeBlock", () => {
  it("renders Prism tokens as safe React text for a known language", () => {
    const code = 'const label = "<script>not executed</script>";';
    const { container } = render(React.createElement(CodeBlock, { code, language: "ts" }));
    const codeElement = container.querySelector("code.language-typescript");

    expect(codeElement?.textContent).toBe(code);
    expect(codeElement?.querySelector(".token.keyword")?.textContent).toBe("const");
    expect(codeElement?.querySelector(".token.keyword")?.getAttribute("style")).toContain("--accent");
    expect(codeElement?.querySelector(".token.string")?.textContent).toBe('"<script>not executed</script>"');
    expect(codeElement?.querySelector("script")).toBeNull();
  });

  it("recognizes the parameter label and file extensions", () => {
    const parameter = render(
      React.createElement(CodeBlock, { code: '{"path":"src/main.rs"}', language: "参数" }),
    );
    expect(parameter.container.querySelector("code.language-json .token.property")?.textContent).toBe('"path"');

    const rust = render(React.createElement(CodeBlock, { code: "fn main() {}", language: "src/main.rs" }));
    expect(rust.container.querySelector("code.language-rust .token.keyword")?.textContent).toBe("fn");
  });

  it("keeps unsupported input as plain escaped text", () => {
    const code = "<img src=x onerror=alert(1)>";
    const { container } = render(React.createElement(CodeBlock, { code, language: "future-language" }));
    const codeElement = container.querySelector("code");

    expect(codeElement?.textContent).toBe(code);
    expect(codeElement?.querySelector(".token")).toBeNull();
    expect(codeElement?.querySelector("img")).toBeNull();
  });
});
