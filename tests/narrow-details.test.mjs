import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { loadScripts } from "./scripts.mjs";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const html = read("../index.html");
const en = JSON.parse(read("../src/data/i18n/ui/en.json"));
const lookup = (key) => key.split(".").reduce((o, k) => o?.[k], en);

// Text is 7 px a character here; a class icon is 19 px wide.
const CHAR = 7;
const ICON = 19;
const kebab = (name) => name.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`);
// The width of the elements the code measures, by class, set per test.
const widthOf = new Map();
class El {
  constructor(tag) {
    this.tagName = tag.toUpperCase();
    this.children = [];
    this.parentNode = null;
    this.attrs = new Map();
    this.classes = new Set();
    this.listeners = new Map();
    this.text = "";
    this.style = {
      props: {}, display: "",
      setProperty(k, v) { this.props[k] = v; }, removeProperty(k) { delete this.props[k]; },
    };
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
  }
  get clientWidth() { return [...this.classes].reduce((w, c) => w || widthOf.get(c) || 0, 0); }
  get offsetWidth() { return this.classes.has("detailsPartyBarIcon") ? ICON : this.clientWidth; }
  get parentElement() { return this.parentNode; }
  set className(v) { this.classes = new Set(String(v).split(/\s+/).filter(Boolean)); }
  get className() { return [...this.classes].join(" "); }
  set textContent(v) { this.children = []; this.text = String(v); }
  get textContent() { return this.text + this.children.map((c) => c.textContent).join(""); }
  set innerHTML(v) { this.children = []; this.text = String(v).replace(/<[^>]*>/g, ""); }
  setAttribute(k, v) { if (k === "class") this.className = v; else this.attrs.set(k, String(v)); }
  getAttribute(k) { return this.attrs.get(k) ?? null; }
  removeAttribute(k) { this.attrs.delete(k); }
  appendChild(c) { c.parentNode = this; this.children.push(c); return c; }
  insertBefore(c, ref) {
    c.parentNode = this;
    const i = this.children.indexOf(ref);
    this.children.splice(i < 0 ? this.children.length : i, 0, c);
    return c;
  }
  addEventListener(type, fn) { this.listeners.set(type, [...(this.listeners.get(type) || []), fn]); }
  getContext() { return { measureText: (t) => ({ width: String(t).length * CHAR }) }; }
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
  closest(selector) { for (let el = this; el; el = el.parentNode) if (el.matches(selector)) return el; return null; }
}
const el = (parent, className) => {
  const e = parent.appendChild(new El("div"));
  e.className = className;
  return e;
};
const headerFrom = (header, markup) => {
  for (const [, attrText, text] of markup.matchAll(/<div (class="cell[^>]*)>([^<]*)<\/div>/g)) {
    const cell = header.appendChild(new El("div"));
    for (const [, k, v] of attrText.matchAll(/([\w-]+)="([^"]*)"/g)) cell.setAttribute(k, v);
    cell.text = text;
  }
};
const between = (from, to) => html.slice(html.indexOf(from), html.indexOf(to, html.indexOf(from)));

function setup() {
  const panel = new El("div");
  panel.className = "detailsPanel";
  const party = el(panel, "detailsPartyList");
  const stats = el(panel, "detailsStats");
  const skills = el(panel, "detailsSkills");
  headerFrom(el(skills, "skillHeader"), between('<div class="detailsSkills">', '<div class="skills">'));
  const list = el(skills, "skills");
  const takenSection = el(panel, "detailsSection takenSection");
  const takenTable = el(takenSection, "detailsSkills takenSkills");
  headerFrom(el(takenTable, "skillHeader"), between('<div class="detailsSkills takenSkills">', '<div class="skills takenList">'));
  const takenList = el(takenTable, "skills takenList");
  const window = { i18n: { t: (key, fallback) => lookup(key) ?? fallback } };
  const context = loadScripts(["shared/format.js", "shared/jobs.js", "shared/players.js", "details.js"], {
    window, console,
    document: {
      createElement: (tag) => new El(tag),
      documentElement: new El("html"),
      // The text of an element, as drawn: 7 px a character.
      createRange: () => {
        let node = null;
        return { selectNodeContents: (n) => { node = n; }, getBoundingClientRect: () => ({ width: node.textContent.length * CHAR }) };
      },
    },
    getComputedStyle: (e) => ({
      getPropertyValue: () => "24", fontSize: "16px", fontFamily: "sans-serif", paddingLeft: "14px", paddingRight: "14px",
      columnGap: e?.classes?.has("detailsPartyBarContent") ? "6px" : "8px",
    }),
    requestAnimationFrame: () => {},
  });
  vm.runInContext("playerNames.hideOthers = true", context);
  const ui = vm.runInContext("createDetailsUI", context)({
    detailsPanel: panel, detailsPartyListEl: party, detailsStatsEl: stats, skillsListEl: list, dpsFormatter: new Intl.NumberFormat("en-US"),
  });
  return { ui, party, takenTable, takenList };
}

// Five players over 90 s; "Gladiator 1" and the like are 11 characters.
const fight = {
  battleTimeMs: 90_000,
  skills: [{ actorId: 1, code: 11010000, name: "Strike", time: 10, dmg: 4_440_000 }],
  perActorStats: [
    // The unattributed row's label is 32 characters, cut at any width.
    { actorId: 80_000_000, job: "", totalDmg: 500_000, contributionPct: 3.4 },
    { actorId: 1, job: "검성", totalDmg: 4_440_000, contributionPct: 31.4 },
    { actorId: 2, job: "수호성", totalDmg: 4_270_000, contributionPct: 30.2 },
    { actorId: 3, job: "궁성", totalDmg: 3_930_000, contributionPct: 27.8 },
    { actorId: 4, job: "치유성", totalDmg: 1_500_000, contributionPct: 10.6 },
  ],
  takenSkills: [
    { actorId: 1, code: 1218730, name: "Attack", sourceCode: 0, damage: 3_225, hits: 12, shieldBlock: 0, parry: 3, perfectBlock: 2, ironWall: 1, regeneration: 1 },
    { actorId: 2, code: 1218731, name: "Energy Bullet", sourceCode: 0, damage: 2_918, hits: 3 },
  ],
};
const FIGURES = ["dps", "dmg", "pct"];
const dropped = (party) => FIGURES.filter((f) => party.classes.has(`drop-bar-${f}`));
const barWidth = (party, f) => Number(String(party.style.props[`--bar-${f}-w`]).replace("px", ""));

test("narrow party bars drop figures from the right end and keep each name whole", () => {
  // The party list at a 520 px window: 170 px inside a bar.
  widthOf.clear();
  widthOf.set("detailsPartyBarContent", 170);
  const { ui, party } = setup();
  ui.render(fight, { id: 1, name: "" });
  assert.deepEqual(dropped(party), ["dmg", "pct"], "% gives way first, then damage");
  // "49.33k/s" is 8 characters: the figure is as wide as its widest text.
  assert.equal(barWidth(party, "dps"), 8 * CHAR);
  const name = 170 - (ICON + 6) - (6 + barWidth(party, "dps"));
  assert.ok(name >= "Gladiator 1".length * CHAR, `"Gladiator 1" fits whole (${name} px)`);
});

test("a name that would not fit whole beside the first figure keeps its room, and the figures go", () => {
  widthOf.clear();
  widthOf.set("detailsPartyBarContent", 120);
  const { ui, party } = setup();
  ui.render(fight, { id: 1, name: "" });
  assert.deepEqual(dropped(party), ["dps", "dmg", "pct"]);
});

test("wide party bars keep every figure at its usual width", () => {
  widthOf.clear();
  widthOf.set("detailsPartyBarContent", 513);
  const { ui, party } = setup();
  ui.render(fight, { id: 1, name: "" });
  assert.deepEqual(dropped(party), []);
  assert.deepEqual(FIGURES.map((f) => barWidth(party, f)), [62, 56, 40]);
});

const takenColumns = (table) => table.style.props["--skill-grid-cols"].split(/ (?=minmax)/);
const shownHeader = (table) => table.querySelectorAll(".skillHeader .cell").filter((c) => c.style.display !== "none").map((c) => c.textContent);

test("the damage received table gives each figure its widest text and drops figures from the right end", () => {
  widthOf.clear();
  // A 520 px window: a 492 px list.
  widthOf.set("takenList", 492);
  const { ui, takenTable } = setup();
  ui.render(fight, { id: 1, name: "" });
  const tracks = takenColumns(takenTable);
  const header = shownHeader(takenTable);
  assert.deepEqual(header, ["Skill", "Monster", "Dmg", "Hits", "BLOC", "PBlk", "ENDR", "RSTO"]);
  // "ENDR" (4 characters) was cut in a 24 px column.
  const least = tracks.slice(2).map((t) => Number(/minmax\((\d+)px/.exec(t)[1]));
  header.slice(2).forEach((text, i) => assert.ok(least[i] >= text.length * CHAR, `${text} fits its column`));

  // A narrower table: the hit results give way from the right; damage stays.
  widthOf.set("takenList", 300);
  const narrow = setup();
  narrow.ui.render(fight, { id: 1, name: "" });
  const kept = shownHeader(narrow.takenTable);
  assert.deepEqual(kept, ["Skill", "Monster", "Dmg", "Hits"].slice(0, kept.length));
  assert.ok(kept.includes("Dmg"));
  assert.equal(takenColumns(narrow.takenTable).length, kept.length);
  const rowCells = narrow.takenList.children[0].children.filter((c) => c.classes.has("cell") && c.style.display !== "none");
  assert.equal(rowCells.length, kept.length, "each row shows the same columns");
});
