import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { loadScripts } from "./scripts.mjs";

const context = loadScripts(["details.js"], { window: {}, console });
const skipWhileRunning = vm.runInContext("skipWhileRunning", context);

test("a Details refresh tick is skipped while the previous one runs", async () => {
  let runs = 0;
  let finish;
  const tick = skipWhileRunning(() => {
    runs++;
    return new Promise((resolve) => { finish = resolve; });
  });
  const first = tick();
  tick();
  tick();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(runs, 1);
  finish();
  await first;
  tick();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(runs, 2);
});

test("a failed Details refresh does not stop the next tick", async () => {
  let runs = 0;
  const tick = skipWhileRunning(async () => {
    runs++;
    throw new Error("backend gone");
  });
  await tick();
  await tick();
  assert.equal(runs, 2);
});
