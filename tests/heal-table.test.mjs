import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { loadScripts } from "./scripts.mjs";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const css = read("../src/styles.css");
const html = read("../index.html");
const en = JSON.parse(read("../src/data/i18n/ui/en.json"));
const lookup = (key) => key.split(".").reduce((o, k) => o?.[k], en);

const HEAL_HIDDEN = ["mhit", "mdmg", "crit", "parry", "perfect", "double", "back", "frontal",
  "block", "perfectblock", "ironwall", "regeneration", "miss", "resist", "regen", "mindmg", "maxdmg"];
const HEAL_SHOWN = ["hit", "dmg", "dmgpct", "avgdmg"];

// Just enough DOM for the Details skill table: class and attribute selectors
// joined by spaces.
const camel = (name) => name.replace(/-([a-z])/g, (_, c) => c.toUpperCase());
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
  click() { (this.listeners.get("click") || []).forEach((fn) => fn({ target: this })); }
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

// Elements from index.html, attributes and text as written there.
const fromMarkup = (tag, attrText, text) => {
  const el = new El(tag);
  for (const [, k, v] of attrText.matchAll(/([\w-]+)="([^"]*)"/g)) el.setAttribute(k, v);
  el.text = text;
  return el;
};

function setup() {
  const panel = new El("div");
  panel.className = "detailsPanel";
  for (const m of html.matchAll(/<button (class="detailsModeBtn[^>]*)>([^<]*)<\/button>/g)) {
    panel.appendChild(fromMarkup("button", m[1], m[2]));
  }
  const skillsContainer = panel.appendChild(new El("div"));
  skillsContainer.className = "detailsSkills";
  const header = skillsContainer.appendChild(new El("div"));
  header.className = "skillHeader";
  const headerMarkup = html.slice(html.indexOf('<div class="skillHeader">'), html.indexOf('<div class="skills">'));
  for (const m of headerMarkup.matchAll(/<div (class="cell[^>]*)>([^<]*)<\/div>/g)) {
    header.appendChild(fromMarkup("div", m[1], m[2]));
  }
  const skillsListEl = skillsContainer.appendChild(new El("div"));
  skillsListEl.className = "skills";
  const statsEl = new El("div");

  const window = { i18n: { t: (key, fallback) => lookup(key) ?? fallback } };
  const context = loadScripts(["shared/format.js", "shared/jobs.js", "shared/players.js", "details.js"], {
    window, console,
    document: { createElement: (tag) => new El(tag), documentElement: new El("html") },
    getComputedStyle: () => ({ getPropertyValue: () => "36", fontSize: "14px", fontFamily: "sans-serif" }),
    requestAnimationFrame: () => {},
  });
  const ui = vm.runInContext("createDetailsUI", context)({
    detailsPanel: panel,
    detailsStatsEl: statsEl,
    skillsListEl,
    dpsFormatter: new Intl.NumberFormat("en-US"),
  });
  const headerCell = (col) => header.children.find((c) => c.dataset.sortKey === col);
  const modeButton = (mode) => panel.children.find((c) => c.dataset.mode === mode);
  return { ui, panel, skillsContainer, skillsListEl, headerCell, modeButton };
}

const fight = {
  skills: [{ actorId: 1, code: 16000000, name: "Strike", time: 4, dmg: 4000, crit: 2, minDmg: 500, maxDmg: 1500 }],
  healSkills: [
    { actorId: 1, code: 17800000, name: "Radiant Benediction", time: 3, dmg: 900, isDot: false },
    { actorId: 1, code: 17810000, name: "Healing Light", time: 1, dmg: 100, isDot: false },
  ],
  totalDmg: 4000, totalHeal: 1000, healTicks: 4, healHotTicks: 0, healSkillCount: 2, healPerSecText: "100",
  battleTimeMs: 10000,
};

const gridTracks = (skillsContainer) => {
  const template = skillsContainer.style.props["--skill-grid-cols"];
  return (template.match(/minmax\(/g) || []).length + (/^\d+px /.test(template) ? 1 : 0);
};

test("HEAL shows ticks, heal, share and average, and the heal header", () => {
  const { ui, panel, skillsContainer, skillsListEl, headerCell, modeButton } = setup();
  ui.render(fight, { id: 1, name: "Healer" });
  const allTracks = gridTracks(skillsContainer);
  modeButton("heal").click();

  assert.ok(panel.classList.contains("isHealMode"));
  assert.equal(headerCell("dmg").textContent, en.details.skills.heal);
  assert.equal(headerCell("dmg").textContent, "Heal");
  assert.equal(headerCell("dmg").getAttribute("data-tip"), "Total Heal");
  assert.equal(headerCell("dmg").dataset.i18n, "details.skills.heal");
  assert.equal(headerCell("hit").textContent, "Ticks");
  assert.equal(headerCell("dmgpct").textContent, "H%");
  assert.equal(headerCell("avgdmg").getAttribute("data-tip"), "Avg Heal");

  // Grid tracks: the name and the four heal columns, nothing else.
  assert.equal(gridTracks(skillsContainer), 1 + HEAL_SHOWN.length);
  assert.ok(allTracks > gridTracks(skillsContainer));
  for (const col of HEAL_HIDDEN) {
    assert.ok(css.includes(`.detailsPanel.isHealMode .detailsSkills .cell.${col},`)
      || css.includes(`.detailsPanel.isHealMode .detailsSkills .cell.${col} {`), `${col} hidden in HEAL`);
  }
  for (const col of HEAL_SHOWN) {
    assert.ok(!css.includes(`.isHealMode .detailsSkills .cell.${col}`), `${col} shown in HEAL`);
  }

  const row = skillsListEl.children[0];
  const cell = (col) => row.children.find((c) => c.classList.contains(col));
  assert.equal(row.querySelector(".skillNameText").textContent, "Radiant Benediction");
  assert.equal(cell("hit").textContent, "3");
  assert.equal(cell("dmgpct").textContent, "90.0%");
  assert.equal(cell("avgdmg").textContent, "300");
});

test("DMG brings the damage columns and labels back", () => {
  const { ui, panel, skillsContainer, headerCell, modeButton } = setup();
  ui.render(fight, { id: 1, name: "Healer" });
  const allTracks = gridTracks(skillsContainer);
  modeButton("heal").click();
  modeButton("dmg").click();
  assert.ok(!panel.classList.contains("isHealMode"));
  assert.equal(headerCell("dmg").textContent, "Dmg");
  assert.equal(headerCell("dmg").getAttribute("data-tip"), "Damage");
  assert.equal(headerCell("dmg").dataset.i18n, "details.skills.dmg");
  assert.equal(headerCell("hit").textContent, "Casts");
  assert.equal(gridTracks(skillsContainer), allTracks);
});
