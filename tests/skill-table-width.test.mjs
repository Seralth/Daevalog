import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { loadScripts } from "./scripts.mjs";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const html = read("../index.html");
const en = JSON.parse(read("../src/data/i18n/ui/en.json"));
const lookup = (key) => key.split(".").reduce((o, k) => o?.[k], en);

// Text is 7 px a character here.
const CHAR = 7;
const kebab = (name) => name.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`);
class El {
  constructor(tag) {
    this.tagName = tag.toUpperCase();
    this.children = [];
    this.parentNode = null;
    this.attrs = new Map();
    this.classes = new Set();
    this.listeners = new Map();
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

function setup(listWidth) {
  const panel = new El("div");
  panel.className = "detailsPanel";
  const skillsContainer = panel.appendChild(new El("div"));
  skillsContainer.className = "detailsSkills";
  const header = skillsContainer.appendChild(new El("div"));
  header.className = "skillHeader";
  const markup = html.slice(html.indexOf('<div class="skillHeader">'), html.indexOf('<div class="skills">'));
  for (const [, attrText, text] of markup.matchAll(/<div (class="cell[^>]*)>([^<]*)<\/div>/g)) {
    const cell = header.appendChild(new El("div"));
    for (const [, k, v] of attrText.matchAll(/([\w-]+)="([^"]*)"/g)) cell.setAttribute(k, v);
    cell.text = text;
  }
  const list = skillsContainer.appendChild(new El("div"));
  list.className = "skills";
  list.clientWidth = listWidth;
  const window = { i18n: { t: (key, fallback) => lookup(key) ?? fallback } };
  const context = loadScripts(["shared/format.js", "shared/jobs.js", "shared/players.js", "details.js"], {
    window, console,
    document: {
      createElement: (tag) => new El(tag),
      documentElement: new El("html"),
      // The text of a cell, as drawn: 7 px a character.
      createRange: () => {
        let node = null;
        return { selectNodeContents: (n) => { node = n; }, getBoundingClientRect: () => ({ width: node.textContent.length * CHAR }) };
      },
    },
    getComputedStyle: () => ({ getPropertyValue: () => "24", fontSize: "16px", fontFamily: "sans-serif", paddingLeft: "14px", paddingRight: "14px", columnGap: "8px" }),
    requestAnimationFrame: () => {},
  });
  const ui = vm.runInContext("createDetailsUI", context)({
    detailsPanel: panel, detailsStatsEl: new El("div"), skillsListEl: list, dpsFormatter: new Intl.NumberFormat("en-US"),
  });
  return { ui, panel, skillsContainer, list };
}

const fight = {
  totalDmg: 300_000,
  skills: [
    { actorId: 1, code: 16010000, name: "Combustion", time: 59, dmg: 146_870, crit: 9, multiHitCount: 12, multiHitDamage: 25_490, minDmg: 1_200, maxDmg: 9_870 },
    { actorId: 1, code: 16020000, name: "Summon: Earth Spirit", time: 16, dmg: 110_500, crit: 1, minDmg: 2_000, maxDmg: 12_100 },
    { actorId: 1, code: 16030000, name: "Elemental Fusion", time: 8, dmg: 42_630, minDmg: 4_000, maxDmg: 7_660 },
  ],
};

const ORDER = ["hit", "dmg", "dmgpct", "mhit", "mdmg", "crit", "parry", "perfect", "double", "back", "frontal",
  "block", "perfectblock", "ironwall", "regeneration", "miss", "resist", "regen", "mindmg", "avgdmg", "maxdmg"];
const widest = (container, col) => Math.max(...container.querySelectorAll(`.cell.${col}`).map((c) => c.textContent.length * CHAR));
const columns = (container) => {
  const [name, ...tracks] = container.style.props["--skill-grid-cols"].split(/ (?=minmax)/);
  return { name: parseFloat(name), mins: tracks.map((t) => Number(/minmax\((\d+)px/.exec(t)[1])) };
};

test("a narrow Details table drops columns from the right end, and no column is narrower than its text", () => {
  const { ui, panel, skillsContainer } = setup(492);
  ui.render(fight, { id: 1, name: "Me" });
  const dropped = ORDER.filter((col) => panel.classList.contains(`drop-col-${col}`));
  const kept = ORDER.filter((col) => !dropped.includes(col));
  assert.ok(dropped.length > 0, "not every column fits 520 px");
  assert.deepEqual(dropped, ORDER.slice(kept.length), "the columns at the right end give way");
  assert.ok(kept.includes("dmg") && kept.includes("dmgpct"), "damage and its share stay");
  const { name, mins } = columns(skillsContainer);
  assert.equal(mins.length, kept.length);
  kept.forEach((col, i) => assert.ok(mins[i] >= widest(skillsContainer, col), `${col} is as wide as "146.87k" and its header`));
  assert.ok(name + mins.reduce((a, b) => a + b, 0) + 8 * kept.length <= 492 - 28, "the kept columns fit");
});

test("a wide Details table keeps every column", () => {
  const { ui, panel } = setup(1400);
  ui.render(fight, { id: 1, name: "Me" });
  assert.deepEqual(ORDER.filter((col) => panel.classList.contains(`drop-col-${col}`)), []);
});

test("the first column stays however narrow the table is, and the skill name gives way", () => {
  const { ui, panel, skillsContainer } = setup(200);
  ui.render(fight, { id: 1, name: "Me" });
  assert.ok(!panel.classList.contains("drop-col-hit"));
  assert.ok(panel.classList.contains("drop-col-dmg"));
  const { name, mins } = columns(skillsContainer);
  assert.equal(mins.length, 1);
  assert.ok(name < 24 + 6 + 14 + "Summon: Earth Spirit".length * CHAR + 4);
});
