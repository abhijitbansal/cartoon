import { test, expect } from "vitest";

test("passes", () => {
  expect(1 + 1).toBe(2);
});

test("fails", () => {
  console.log("debug value", 41);
  expect(1 + 1).toBe(3);
});
