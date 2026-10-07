import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { loadScripts } from "./scripts.mjs";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const html = read("../index.html");
const en = JSON.parse(read("../src/data/i18n/ui/en.json"));
const lookup = (key) => key.split(".").reduce((o, k) => o?.[k], en);

// Transcendent Bakarma on 2026-10-06, as the backend saves it: the player
// (9200) took 20,374 from seven hits and an immune; another player took one
// parried hit and two Poison ticks.
const BAKARMA = 2310471;
const fight = [
  { actorId: 9200, code: 1800650, name: "Rapid Strike", sourceCode: BAKARMA, damage: 9000, hits: 2, front: 1 },
  { actorId: 9200, code: 1800640, name: "Crush", sourceCode: BAKARMA, damage: 11374, hits: 5, immune: 1, back: 1 },
  { actorId: 9300, code: 1800650, name: "Rapid Strike", sourceCode: BAKARMA, damage: 3000, hits: 1, parry: 1 },
  { actorId: 9300, code: 1200010, name: "Poison", sourceCode: 0, damage: 600, ticks: 2 },
];

test("a saved fight's damage received follows the chosen player", async () => {
  const details = async (takenSkills, attackerIds) => {
    const window = { addEventListener() {}, _historyDetailsOverride: { skills: [], battleTime: 1000, takenSkills } };
    const context = loadScripts(["shared/format.js", "shared/jobs.js", "shared/players.js", "shared/targetModes.js", "core.js"],
      { window, console, document: { readyState: "loading", addEventListener() {} } });
    const app = vm.runInContext("Object.create(DpsApp.prototype)", context);
    app.dpsFormatter = new Intl.NumberFormat("en-US");
    return app.getDetails({ id: null }, { targetId: 1, attackerIds });
  };
  assert.equal((await details(fight, null)).takenSkills.length, 4);
  assert.deepEqual((await details(fight, [9200])).takenSkills.map((e) => e.code), [1800650, 1800640]);
  assert.equal((await details(undefined, null)).takenSkills, null, "saved before damage received was kept");
});

test("damage received adds up per skill and monster, attacks as the game counts them", () => {
  const context = loadScripts(["details.js"], { window: {}, console });
  const totals = vm.runInContext("takenTotals", context);
  // Hits with a value, reflects, immunes and misses are attacks; ticks are not.
  assert.deepEqual({ ...totals(fight.filter((e) => e.actorId === 9200)) }, { damage: 20374, attacks: 8 });
  assert.equal(totals(null), null);
  const rows = vm.runInContext("takenRows", context)(fight);
  assert.deepEqual(Array.from(rows, (r) => [r.code, r.sourceCode, r.damage, r.attacks]),
    [[1800650, BAKARMA, 12000, 3], [1800640, BAKARMA, 11374, 6], [1200010, 0, 600, 0]]);
  assert.equal(rows.find((r) => r.code === 1800650).parry, 1);
  // Two skills of one name, an attack and the immune to it, are one row.
  const named = vm.runInContext("takenRows", context)([
    { actorId: 9200, code: 1800554, name: "Surging Wrath", sourceCode: BAKARMA, damage: 2736, hits: 1 },
    { actorId: 9200, code: 1800559, name: "Surging Wrath", sourceCode: BAKARMA, damage: 0, immune: 1 },
  ]);
  assert.deepEqual(Array.from(named, (r) => [r.name, r.damage, r.attacks]), [["Surging Wrath", 2736, 2]]);
});

// Just enough DOM for Details: class and attribute selectors joined by spaces.
const kebab = (name) => name.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`);
class El {
  constructor(tag) {
    this.tagName = tag.toUpperCase();
    this.children = [];
    this.parentNode = null;
    this.attrs = new Map();
    this.classes = new Set();
    this.text = "";
    this.style = { props: {}, setProperty(k, v) { this.props[k] = v; }, removeProperty(k) { delete this.props[k]; } };
    this.classList = {
      add: (...c) => c.forEach((x) => this.classes.add(x)),
      remove: (...c) => c.forEach((x) => this.classes.delete(x)),
      contains: (c) => this.classes.has(c),
      toggle: (c, force = !this.classes.has(c)) => (force ? this.classes.add(c) : this.classes.delete(c), force),
    };
    this.dataset = new Proxy({}, {
      get: (_, k) => this.attrs.get(`data-${kebab(String(k))}`),
      set: (_, k, v) => (this.attrs.set(`data-${kebab(String(k))}`, String(v)), true),
    });
    this.offsetWidth = 0;
    this.clientWidth = 0;
  }
  set className(v) { this.classes = new Set(String(v).split(/\s+/).filter(Boolean)); }
  get className() { return [...this.classes].join(" "); }
  set textContent(v) { this.children = []; this.text = String(v); }
  get textContent() { return this.text + this.children.map((c) => c.textContent).join(""); }
  set innerHTML(v) { this.children = []; this.text = ""; }
  setAttribute(k, v) { if (k === "class") this.className = v; else this.attrs.set(k, String(v)); }
  getAttribute(k) { return this.attrs.get(k) ?? null; }
  removeAttribute(k) { this.attrs.delete(k); }
  appendChild(c) { c.parentNode = this; this.children.push(c); return c; }
  addEventListener() {}
  getContext() { return { measureText: (t) => ({ width: String(t).length * 6 }) }; }
  matches(compound) {
    const classes = [...compound.matchAll(/\.([\w-]+)/g)].map((m) => m[1]);
    const attrs = [...compound.matchAll(/\[([\w-]+)\]/g)].map((m) => m[1]);
    return classes.every((c) => this.classes.has(c)) && attrs.every((a) => this.attrs.has(a));
  }
  descendants() { return this.children.flatMap((c) => [c, ...c.descendants()]); }
  querySelectorAll(selector) {
    const parts = selector.trim().split(/\s+/);
    return this.descendants().filter((el) => {
      if (!el.matches(parts.at(-1))) return false;
      let i = parts.length - 2;
      for (let up = el.parentNode; up && i >= 0; up = up.parentNode) if (up.matches(parts[i])) i--;
      return i < 0;
    });
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
  closest(selector) {
    for (let el = this; el; el = el.parentNode) if (el.matches(selector)) return el;
    return null;
  }
}
const el = (parent, className) => {
  const e = parent.appendChild(new El("div"));
  e.className = className;
  return e;
};
// A header's cells as index.html writes them.
const headerFrom = (header, markup) => {
  for (const [, attrText, text] of markup.matchAll(/<div (class="cell[^>]*)>([^<]*)<\/div>/g)) {
    const cell = header.appendChild(new El("div"));
    for (const [, k, v] of attrText.matchAll(/([\w-]+)="([^"]*)"/g)) cell.setAttribute(k, v);
    cell.text = text;
  }
};

function setup() {
  const panel = new El("div");
  panel.className = "detailsPanel";
  // The skill table comes first, as in the page.
  const skills = el(panel, "detailsSkills");
  headerFrom(el(skills, "skillHeader"), html.slice(html.indexOf('<div class="skillHeader">'), html.indexOf('<div class="skills">')));
  const skillsListEl = el(skills, "skills");
  const takenMarkup = html.slice(html.indexOf('<div class="detailsSection takenSection">'), html.indexOf('<div class="skills takenList">'));
  assert.ok(takenMarkup.length > 0, "index.html has the damage received section");
  const section = el(panel, "detailsSection takenSection");
  const table = el(section, "detailsSkills takenSkills");
  headerFrom(el(table, "skillHeader"), takenMarkup);
  const list = el(table, "skills takenList");
  const statsEl = new El("div");
  const window = { i18n: { t: (key, fallback) => lookup(key) ?? fallback, getNpcName: (id, fb) => (id === BAKARMA ? "Transcendent Bakarma" : fb) } };
  const context = loadScripts(["shared/format.js", "shared/jobs.js", "shared/players.js", "details.js"], {
    window, console,
    document: { createElement: (tag) => new El(tag), documentElement: new El("html") },
    getComputedStyle: () => ({ getPropertyValue: () => "36", fontSize: "14px", fontFamily: "sans-serif" }),
    requestAnimationFrame: () => {},
  });
  const ui = vm.runInContext("createDetailsUI", context)({
    detailsPanel: panel, detailsStatsEl: statsEl, skillsListEl, dpsFormatter: new Intl.NumberFormat("en-US"),
  });
  const stat = (label) => statsEl.children.find((s) => s.children[0].textContent === label)?.children[1].textContent;
  return { ui, section, table, list, stat };
}

test("Details shows the damage received in full, and what hit the player", () => {
  const { ui, section, table, list, stat } = setup();
  ui.render({ skills: [], takenSkills: fight.filter((e) => e.actorId === 9200) }, { id: 9200, name: "Me" });
  assert.equal(stat("Damage Received"), "20,374");
  assert.equal(stat("Received Hits"), "8");
  assert.notEqual(section.style.display, "none");
  const cells = (row) => row.children.filter((c) => c.classes.has("cell")).map((c) => c.textContent);
  assert.deepEqual(list.children.map(cells), [
    ["Crush", "Transcendent Bakarma", "11,374", "6"],
    ["Rapid Strike", "Transcendent Bakarma", "9,000", "2"],
  ]);
  // No block, parry, endurance or regeneration: none of their columns.
  const shown = table.querySelectorAll(".skillHeader .cell[data-taken]").filter((c) => c.style.display !== "none");
  assert.deepEqual(shown, []);

  ui.render({ skills: [], takenSkills: fight }, { id: null, name: "" });
  assert.deepEqual(table.querySelectorAll(".skillHeader .cell[data-taken]").filter((c) => c.style.display !== "none")
    .map((c) => c.dataset.taken), ["parry"]);
  const rapid = list.children.find((r) => cells(r)[0] === "Rapid Strike");
  assert.deepEqual(cells(rapid), ["Rapid Strike", "Transcendent Bakarma", "12,000", "3", "33%"]);
  const poison = list.children.find((r) => cells(r)[0] === "Poison");
  assert.deepEqual(cells(poison), ["Poison", "-", "600", "0", ""]);

  // A fight saved before damage received was kept: no number, no list.
  ui.render({ skills: [], takenSkills: null }, { id: 9200, name: "Me" });
  assert.equal(stat("Damage Received"), "-");
  assert.equal(section.style.display, "none");
});
