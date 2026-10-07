import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { loadScripts } from "./scripts.mjs";

function details(skills) {
  const window = { addEventListener() {}, _historyDetailsOverride: { skills, battleTime: 1000 } };
  const context = loadScripts(["shared/format.js", "shared/jobs.js", "shared/players.js", "shared/targetModes.js", "details.js", "core.js"], { window, console, document: { readyState: "loading", addEventListener() {} } });
  const app = vm.runInContext("Object.create(DpsApp.prototype)", context);
  app.dpsFormatter = new Intl.NumberFormat("en-US");
  return app.getDetails({ id: 1 }, {});
}

test("a skill that only missed keeps its row and adds nothing to the totals", async () => {
  const d = await details([
    { actorId: 1, code: 16000000, name: "Hit", time: 4, dmg: 400, parry: 1, shieldBlock: 2, perfectBlock: 1 },
    { actorId: 1, code: 16330007, name: "Miss only", time: 0, dmg: 0, miss: 3 },
    { actorId: 1, code: 16330008, name: "Nothing", time: 0, dmg: 0 },
  ]);
  assert.deepEqual([...d.skills].map((s) => s.name), ["Hit", "Miss only"]);
  assert.equal(d.totalDmg, 400);
  assert.equal(d.totalHits, 4);
  assert.deepEqual([d.skills[0].shieldBlock, d.skills[0].perfectBlock, d.skills[1].miss], [2, 1, 3]);
});

test("a fight saved before the rename reads its flags under the new names", async () => {
  const d = await details([{ actorId: 1, code: 16000000, name: "Hit", time: 2, dmg: 200, smite: 1, powershard: 2 }]);
  assert.deepEqual([d.skills[0].regeneration, d.skills[0].perfectBlock], [1, 2]);
});

test("the additional-hit rate counts hits that had additional hits, as the skill rows do", async () => {
  // Four hits, one of them with three additional hits.
  const d = await details([{ actorId: 1, code: 16000000, name: "Hit", time: 4, dmg: 400, multiHitCount: 1, multiHitHits: 3 }]);
  assert.equal(d.multiHitPct, 25);
});
