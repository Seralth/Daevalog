import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { loadScripts } from "./scripts.mjs";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");

function columns() {
  const window = {};
  vm.runInNewContext(read("../public/src/js/meterColumns.js"), { window });
  return window.MeterColumns;
}

function app(MeterColumns) {
  const window = { addEventListener() {}, MeterColumns };
  const context = loadScripts(["shared/format.js", "shared/jobs.js", "shared/targetModes.js", "core.js"], { window, console, document: { readyState: "loading", addEventListener() {} } });
  return vm.runInContext("Object.create(DpsApp.prototype)", context);
}

const fmt = {
  rate: (v) => `${Math.round(v)}`,
  amount: (v) => `${v / 1000}k`,
  count: (v) => `${v}`,
};

test("a saved column choice keeps the list order and drops unknown keys", () => {
  const C = columns();
  assert.deepEqual([...C.parse("hits, nope ,encDps,pct")], ["encDps", "pct", "hits"]);
  assert.equal(C.parse(""), null);
  assert.equal(C.parse(null), null);
  assert.equal(C.parse("nope"), null);
  assert.equal(C.serialize(["last10", "encDps"]), "encDps,last10");
});

test("before a choice is saved, Settings shows what the row shows today", () => {
  const C = columns();
  assert.deepEqual([...C.defaultsFor("dps")], ["encDps", "pct"]);
  assert.deepEqual([...C.defaultsFor("totalDamage")], ["total", "pct"]);
  assert.deepEqual([...C.defaultsFor("both")], ["encDps", "pct", "total"]);
});

test("a narrow meter drops columns from the end, never the first", () => {
  const C = columns();
  const all = [...C.COLUMNS.map((c) => c.key)];
  assert.deepEqual([...C.fit(all, 2000)], all);
  const at400 = C.fit(all, 400);
  assert.ok(at400.length > 1 && at400.length < all.length);
  assert.deepEqual([...at400], all.slice(0, at400.length));
  assert.ok(C.fit(all, 300).length < at400.length);
  assert.deepEqual([...C.fit(["hits", "maxHit"], 100)], ["hits"]);
  assert.deepEqual([...C.fit(["encDps", "pct"], 0)], ["encDps", "pct"], "no width known yet");
});

test("each column shows its own figure", () => {
  const C = columns();
  const row = {
    dps: 1200, activeDps: 2400, totalDamage: 50000, hits: 8, critHits: 3, maxHit: 9000,
    last10Dps: 10, last30Dps: 30, last60Dps: 60,
  };
  const text = (key) => C.cellText(key, row, fmt, 12.345);
  assert.equal(text("encDps"), "1200");
  assert.equal(text("dps"), "2400");
  assert.equal(text("pct"), "12.3%");
  assert.equal(text("total"), "50k");
  assert.equal(text("crit"), "38%");
  assert.equal(text("last10"), "10");
  assert.equal(text("last30"), "30");
  assert.equal(text("last60"), "60");
  assert.equal(text("maxHit"), "9k");
  assert.equal(text("hits"), "8");
  assert.equal(C.cellText("crit", { hits: 0 }, fmt, 0), "-");
});

test("rows carry the backend's ENC figures, and only ENC uses the columns", () => {
  const C = columns();
  const dps = app(C);
  dps.USER_NAME = "";
  const [row] = dps.buildRowsFromMapObject({
    7: {
      job: "Gladiator", nickname: "Aki", dps: 100, amount: 1000, damageContribution: 50,
      activeDps: 150, last10Dps: 1, last30Dps: 2, last60Dps: 3, hits: 9, critHits: 4, maxHit: 400,
    },
  });
  assert.deepEqual(
    [row.activeDps, row.last10Dps, row.last30Dps, row.last60Dps, row.hits, row.critHits, row.maxHit],
    [150, 1, 2, 3, 9, 4, 400],
  );

  dps.encColumns = null;
  dps.targetSelection = "encounter";
  assert.equal(dps.getRowColumns(), null, "no choice saved: the usual readout");
  dps.encColumns = C.parse("encDps,hits");
  assert.deepEqual([...dps.getRowColumns()], ["encDps", "hits"]);
  dps.targetSelection = "bossTargets";
  assert.equal(dps.getRowColumns(), null);
});

test("summons and effects tied to no player are one labelled row, not a player", () => {
  const dps = app(columns());
  dps.USER_NAME = "";
  const rows = dps.buildRowsFromMapObject({
    80000000: { job: "Unknown", nickname: "80000000", dps: 50, amount: 500 },
    4321: { job: "Sorcerer", nickname: "4321", dps: 10, amount: 100 },
  });
  const byId = Object.fromEntries(rows.map((r) => [r.id, r]));
  assert.equal(byId["80000000"].name, "Unattributed summons and effects");
  assert.equal(byId["80000000"].isIdentifying, false);
  assert.equal(byId["4321"].isIdentifying, true, "a player without a name still shows by id");
});
