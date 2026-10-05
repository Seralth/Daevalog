import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { loadScripts } from "./scripts.mjs";

const tick = () => new Promise((resolve) => setImmediate(resolve));

function setup(getBattleDetail) {
  const logs = [];
  const window = { addEventListener() {}, dpsData: { getBattleDetail }, javaBridge: { logToDebug: (s) => logs.push(s) } };
  const context = loadScripts(["shared/format.js", "shared/jobs.js", "shared/targetModes.js", "core.js"], { window, console, document: { readyState: "loading", addEventListener() {} } });
  const app = vm.runInContext("Object.create(DpsApp.prototype)", context);
  app.dpsFormatter = new Intl.NumberFormat("en-US");
  app.elList = { querySelector: () => ({}) };
  app.hoveredDetailsRowId = 1;
  app.hoverTooltipCacheByRowId = new Map();
  app.hoverTooltipPendingRowIds = new Set();
  app.hoverTooltipRequestSeqByRowId = new Map();
  const rendered = [];
  app.renderHoverTooltip = (details) => rendered.push(details);
  return { app, rendered, logs, window };
}

test("hover replaces loading with the player's highest-damage skills", async () => {
  const { app, rendered } = setup(async () => JSON.stringify({
    battleTime: 10000,
    skills: Array.from({ length: 7 }, (_, i) => ({ code: 18010000 + i * 10000, name: `Skill ${i}`, dmg: 100 * (i + 1), time: 1, actorId: 1 })),
  }));
  app.applyHoverTooltip({ id: 1 }, { forceRefresh: true });
  assert.equal(rendered[0].state, "loading");
  await tick();
  assert.equal(rendered.at(-1).skills.length, 5);
  assert.equal(rendered.at(-1).skills[0].dmg, 700);
  assert.notEqual(rendered.at(-1).state, "loading");
});

test("an empty response does not leave a perpetual loading state", async () => {
  const { app, rendered } = setup(async () => null);
  app.applyHoverTooltip({ id: 1 }, { forceRefresh: true });
  await tick();
  assert.equal(rendered.at(-1).skills.length, 0);
  assert.notEqual(rendered.at(-1).state, "loading");
});

test("request failures are visible and logged instead of masquerading as loading", async () => {
  const { app, rendered, logs } = setup(async () => { throw new Error("IPC failed"); });
  app.applyHoverTooltip({ id: 1 }, { forceRefresh: true });
  await tick();
  assert.equal(rendered.at(-1).state, "error");
  assert.match(logs[0], /IPC failed/);
});

test("rendered tooltip text distinguishes loading, empty data and errors", () => {
  const { app } = setup();
  app.hoverTooltipEl = { style: {}, classList: { add() {} }, offsetWidth: 100, offsetHeight: 100 };
  const render = Object.getPrototypeOf(app).renderHoverTooltip;
  const row = { id: 1, name: "Test", dps: 100, totalDamage: 1000 };
  for (const [state, text] of [["loading", "Loading..."], ["empty", "No skill data for this fight"], ["error", "Could not load skills"]]) {
    render.call(app, { skills: [], state }, row, {});
    assert.ok(app.hoverTooltipEl.innerHTML.includes(text));
    if (state !== "loading") assert.ok(!app.hoverTooltipEl.innerHTML.includes("Loading..."));
  }
});

test("hover asks for the summary, without hit timelines", async () => {
  const asked = [];
  const { app } = setup(async (id, summaryOnly) => {
    asked.push(summaryOnly);
    return JSON.stringify({ skills: [] });
  });
  app.applyHoverTooltip({ id: 1 }, { forceRefresh: true });
  await tick();
  assert.deepEqual(asked, [true]);
});

test("an open tooltip refreshes with the meter, at most once a second, without a loading state", async () => {
  let dmg = 100;
  const { app, rendered } = setup(async () => JSON.stringify({ battleTime: 1000, skills: [{ code: 18010000, name: "A", dmg, time: 1, actorId: 1 }] }));
  let now = 5000;
  app.nowMs = () => now;
  app.pinnedDetailsRowId = null;
  app.hoverTooltipEl = { classList: { contains: (c) => c === "isVisible" } };
  app.latestRowsById = new Map([["1", { id: 1 }]]);
  app.refreshHoverTooltip();
  await tick();
  assert.equal(rendered.length, 1);
  assert.equal(rendered[0].skills[0].dmg, 100);
  dmg = 250;
  now += 500;
  app.refreshHoverTooltip();
  await tick();
  assert.equal(rendered.length, 1);
  now += 600;
  app.refreshHoverTooltip();
  await tick();
  assert.equal(rendered.length, 2);
  assert.equal(rendered[1].skills[0].dmg, 250);
  assert.ok(rendered.every((r) => r.state !== "loading"));
});

test("a failed refresh keeps the numbers already shown", async () => {
  const { app, rendered } = setup(async () => { throw new Error("IPC failed"); });
  app.nowMs = () => 5000;
  app.pinnedDetailsRowId = null;
  app.hoverTooltipEl = { classList: { contains: () => true } };
  app.latestRowsById = new Map([["1", { id: 1 }]]);
  app.refreshHoverTooltip();
  await tick();
  assert.equal(rendered.length, 0);
});

test("a hidden tooltip is not refreshed", async () => {
  const asked = [];
  const { app } = setup(async () => { asked.push(1); return "{}"; });
  app.nowMs = () => 5000;
  app.pinnedDetailsRowId = null;
  app.hoverTooltipEl = { classList: { contains: () => false } };
  app.latestRowsById = new Map([["1", { id: 1 }]]);
  app.refreshHoverTooltip();
  await tick();
  assert.equal(asked.length, 0);
});

test("short numbers are whole: a heal rate reads 368, not 368.036", () => {
  const { app } = setup();
  assert.equal(app.formatAbbreviatedNumber(368.036), "368");
  assert.equal(app.formatAbbreviatedNumber(40480), "40.48k");
});
