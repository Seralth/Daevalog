import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { loadScripts } from "./scripts.mjs";

const context = loadScripts(["details.js"], { window: {}, console });

test("merged Details keep the healing, once", () => {
  const heals = [{ actorId: 1, code: 17800000, name: "Radiant Benediction", dmg: 600, time: 2, isDot: false }];
  const perTarget = { skills: [], healSkills: heals, totalHeal: 600, healTicks: 2, healHotTicks: 0, healSkillCount: 1, healPerSecText: "60" };
  const merged = vm.runInContext("combinedHealFields", context)([perTarget, { ...perTarget }]);
  assert.equal(merged.healSkills.length, 1);
  assert.deepEqual([merged.totalHeal, merged.healTicks, merged.healSkillCount, merged.healPerSecText], [600, 2, 1, "60"]);
});
