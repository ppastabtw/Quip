import assert from "node:assert/strict";
import test from "node:test";
import {
  protectBoundaryWhitespace,
  replacePreservingBoundaryWhitespace,
} from "./replacement.ts";

test("protects separators on both sides of a replacement", () => {
  const value = "hello  wrld next";
  const range = protectBoundaryWhitespace(value, 5, 12);
  assert.deepEqual(range, { start: 7, end: 11 });
  assert.equal(
    replacePreservingBoundaryWhitespace(value, 5, 12, "world").value,
    "hello  world next",
  );
});

test("does not invent whitespace beside punctuation", () => {
  const value = "say wrld,please";
  assert.equal(
    replacePreservingBoundaryWhitespace(value, 4, 8, "world").value,
    "say world,please",
  );
});
