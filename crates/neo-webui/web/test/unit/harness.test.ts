import { expect, test } from "vitest";

import { resetHarness } from "./harness";

test("harness supplies a complete local storage interface", () => {
  resetHarness();

  window.localStorage.setItem("first", "one");
  window.localStorage.setItem("second", "two");

  expect(window.localStorage.length).toBe(2);
  expect(window.localStorage.getItem("first")).toBe("one");
  expect(window.localStorage.key(1)).toBe("second");

  window.localStorage.removeItem("first");
  expect(window.localStorage.getItem("first")).toBeNull();

  window.localStorage.clear();
  expect(window.localStorage.length).toBe(0);
});
