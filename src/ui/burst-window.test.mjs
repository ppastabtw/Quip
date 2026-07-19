import assert from "node:assert/strict";
import test from "node:test";
import { completedBurstWindows, trailingBurstStart } from "./burst-window.ts";

test("splits a long paste into completed five-word windows", () => {
  const value = "one two three four five six seven eight nine ten eleven twelve";
  assert.deepEqual(completedBurstWindows(value, 0, value.length, 5), [
    { start: 0, end: 24 },
    { start: 24, end: 49 },
  ]);
  assert.equal(value.slice(49), "eleven twelve");
});

test("waits for a boundary before freezing the fifth word", () => {
  const unfinished = "one two three four five";
  assert.deepEqual(completedBurstWindows(unfinished, 0, unfinished.length, 5), []);

  const completed = `${unfinished} `;
  assert.deepEqual(completedBurstWindows(completed, 0, completed.length, 5), [
    { start: 0, end: completed.length },
  ]);
});

test("recovers an edited composer from the trailing bounded window", () => {
  const value = "one two three four five six seven";
  assert.equal(trailingBurstStart(value, value.length, 5), 8);
  assert.equal(value.slice(trailingBurstStart(value, value.length, 5)), "three four five six seven");
  assert.equal(trailingBurstStart("", 0, 5), 0);
});
