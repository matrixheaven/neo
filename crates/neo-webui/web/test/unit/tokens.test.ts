/**
 * Token layering guard: raw palette blocks (html[data-theme]) must contain
 * only literal values — a var() pointing at the semantic layer would form a
 * same-element cycle and compute to invalid (transparent). The semantic
 * layer (:root) must reference only --p-* palette entries.
 */

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const css = readFileSync(resolve(process.cwd(), "src/styles.css"), "utf8");

function block(selector: string): string {
  const start = css.indexOf(`${selector} {`);
  if (start === -1) throw new Error(`missing block: ${selector}`);
  const end = css.indexOf("\n}", start);
  if (end === -1) throw new Error(`unterminated block: ${selector}`);
  return css.slice(start, end);
}

describe("token layering", () => {
  it("raw palettes contain only literals, semantics reference only the palette", () => {
    for (const theme of ["dark", "light"]) {
      const palette = block(`html[data-theme="${theme}"]`);
      const cyclic = palette.match(/var\(--/g);
      expect(cyclic, `${theme} palette must not reference other custom properties`).toBeNull();
    }

    const semantic = block(":root");
    const colorSection = semantic.slice(0, semantic.indexOf("/* Shadow grading */"));
    for (const line of colorSection.split("\n")) {
      const declaration = /^\s+--[\w-]+:\s*(.+);$/.exec(line);
      if (!declaration) continue;
      expect(declaration[1], line.trim()).toMatch(/^var\(--p-[\w-]+\)$/);
    }
  });
});
