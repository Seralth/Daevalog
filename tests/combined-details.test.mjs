import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { loadScripts } from "./scripts.mjs";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const html = read("../index.html");
const en = JSON.parse(read("../src/data/i18n/ui/en.json"));
const lookup = (key) => key.split(".").reduce((o, k) => o?.[k], en);

const context = loadScripts(["details.js"], { window: {}, console });

test("a DoT row folds under its skill in every language", () => {
  const fold = vm.runInContext("foldDotRows", context);
  for (const dotName of ["Feuer - DOT", "Feuer - DoT", "Feuer - 継続ダメージ", "Feuer - периодический"]) {
    const rows = fold([
      { code: "16330000", name: "Feuer", isDot: false, dmg: 100 },
      { code: "16330000-dot", name: dotName, isDot: true, dmg: 50 },
      { code: "16990000-dot", name: "Gift - DoT", isDot: true, dmg: 7 },
    ]);
    assert.deepEqual([...rows].map((r) => r.code), ["16330000", "16990000-dot"], dotName);
    assert.equal(rows[0]._dotChild.dmg, 50);
    assert.equal(rows[0]._combinedDmg, 150);
  }
});

test("the fight's time leaves out the gaps between pulls and counts a boss and its adds once", () => {
  const activeTime = vm.runInContext("activeTime", context);
  assert.equal(activeTime([[0, 80_000], [30_000, 40_000], [100_000, 110_000]]), 90_000);
  assert.equal(activeTime([[5, 10], [0, 3]]), 8);
  assert.equal(activeTime([]), 0);
});

// Just enough DOM for the Details panel: class and attribute selectors joined
// by spaces, listeners, and canvases that record where they draw.
const kebab = (name) => name.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`);
class El {
  constructor(tag = "div") {
    this.tagName = tag.toUpperCase();
    this.children = [];
    this.parentNode = null;
    this.attrs = new Map();
    this.classes = new Set();
    this.listeners = new Map();
    this.text = "";
    this.style = { setProperty() {}, removeProperty() {} };
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
    this.drawn = null;
  }
  set className(v) { this.classes = new Set(String(v).split(/\s+/).filter(Boolean)); }
  get className() { return [...this.classes].join(" "); }
  set textContent(v) { this.children = []; this.text = String(v); }
  get textContent() { return this.text + this.children.map((c) => c.textContent).join(""); }
  set innerHTML(v) { this.children = []; this.text = String(v).replace(/<[^>]*>/g, ""); }
  get parentElement() { return this.parentNode; }
  get nextSibling() { const s = this.parentNode?.children ?? []; return s[s.indexOf(this) + 1] ?? null; }
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
  getContext() {
    const arcs = [];
    this.drawn = { arcs };
    const noop = () => {};
    return {
      arcs, scale: noop, clearRect: noop, fillRect: noop, fillText: noop, fill: noop, drawImage: noop, beginPath: noop,
      moveTo: noop, lineTo: noop, bezierCurveTo: noop, setLineDash: noop, stroke: noop,
      arc: (x) => arcs.push(x), measureText: (t) => ({ width: String(t).length * 6 }),
    };
  }
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
const el = (parent, className, tag = "div") => {
  const e = parent.appendChild(new El(tag));
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

// A boss fought from 0 to 80 s with an add from 30 to 40 s, then the next
// pull from 100 to 110 s. Each answer counts its hit times from its own
// target's first hit and holds the healing of its own span; the context lists
// the fight's healing once.
const T0 = 1_700_000_000_000;
const skill = (actorId, code, name, dmg, hitTimestamps) => ({ actorId, code, name, dmg, time: hitTimestamps.length, hitTimestamps, job: "" });
const heal = (dmg) => [{ actorId: 1, code: 17010000, name: "Healing Light", dmg, time: 1, isDot: false }];
const answers = {
  800: { battleTime: 80_000, startTime: T0, skills: [skill(1, 16010000, "Strike", 8000, [0, 80_000])], healSkills: heal(1200) },
  801: { battleTime: 10_000, startTime: T0 + 30_000, skills: [skill(1, 16020000, "Cleave", 1000, [0, 10_000])], healSkills: heal(300) },
  802: { battleTime: 10_000, startTime: T0 + 100_000, skills: [skill(2, 16030000, "Slash", 900, [0, 10_000])], healSkills: [] },
};
const target = (targetId, actorDamage) => ({
  targetId, targetName: `Mob ${targetId}`, mobCode: 0, totalDamage: Object.values(actorDamage).reduce((a, b) => a + b, 0),
  battleTime: answers[targetId].battleTime, lastDamageTime: answers[targetId].startTime + answers[targetId].battleTime, actorDamage,
});
const fightContext = (healSkills = heal(1400)) => ({
  currentTargetId: 0,
  targets: [target(800, { 1: 8000 }), target(801, { 1: 1000 }), target(802, { 2: 900 })],
  actors: [{ actorId: 1, nickname: "One", job: "" }, { actorId: 2, nickname: "Two", job: "" }],
  numbers: {},
  takenSkills: [],
  healSkills,
});

async function openEveryTarget(ctx, given = answers) {
  const root = new El();
  root.className = "detailsPanel";
  const title = el(root, "detailsFightTitle");
  for (const mode of ["dmg", "heal"]) el(root, "detailsModeBtn", "button").dataset.mode = mode;
  const party = el(root, "detailsPartyList");
  const stats = el(root, "detailsStats");
  const chartSection = el(root, "detailsSection dpsChartSection");
  const wrap = el(el(chartSection, "detailsSectionContent"), "dpsChartWrap");
  wrap.clientWidth = 1040;
  el(wrap, "dpsChartLegend");
  el(wrap, "dpsChartCanvas", "canvas");
  const chartAxis = el(wrap, "dpsChartXAxis");
  const skills = el(root, "detailsSkills");
  headerFrom(el(skills, "skillHeader"), html.slice(html.indexOf('<div class="skillHeader">'), html.indexOf('<div class="skills">')));
  const list = el(skills, "skills");
  const timelineChart = el(el(root, "detailsSection timelineSection"), "timelineChart");
  el(timelineChart, "timelineLegend");
  const viewport = el(timelineChart, "timelineViewport");
  viewport.clientWidth = 1124;
  const timeline = el(viewport, "timelineCanvas", "canvas");
  el(timelineChart, "timelineXAxis");

  const window = {
    addEventListener() {},
    i18n: { t: (key, fallback) => lookup(key) ?? fallback },
    dpsData: { getTargetDetails: async (id) => JSON.stringify(given[id] ?? {}) },
    javaBridge: { logToDebug() {} },
  };
  const page = loadScripts(["shared/format.js", "shared/jobs.js", "shared/players.js", "shared/targetModes.js", "details.js", "core.js"], {
    window, console,
    document: { readyState: "loading", addEventListener() {}, createElement: (tag) => new El(tag), documentElement: new El("html") },
    getComputedStyle: () => ({ getPropertyValue: () => "36", fontSize: "14px", fontFamily: "sans-serif" }),
    requestAnimationFrame: () => 0,
    setInterval: () => 0,
    clearInterval() {},
  });
  const app = vm.runInContext("Object.create(DpsApp.prototype)", page);
  Object.assign(app, { dpsFormatter: new Intl.NumberFormat("en-US"), i18n: window.i18n });
  const ui = vm.runInContext("createDetailsUI", page)({
    detailsPanel: root, detailsFightTitleEl: title, detailsPartyListEl: party, detailsStatsEl: stats, skillsListEl: list,
    dpsFormatter: app.dpsFormatter,
    getDetails: (row, options) => app.getDetails(row, options),
    getDetailsContext: () => ctx,
  });
  await ui.open(null, { pin: true, force: true, defaultTargetAll: true });
  const stat = (key) => stats.children.find((s) => s.style.display !== "none" && s.children[0].textContent === lookup(key))?.children[1].textContent;
  const bars = () => party.children.slice(1).map((bar) => bar.children[1].children.map((c) => c.textContent));
  // Each bar's class icon.
  const icons = () => party.children.slice(1).map((bar) => bar.children[1].children.find((c) => c.tagName === "IMG")?.alt ?? "");
  const tab = (mode) => root.children.find((c) => c.dataset.mode === mode).click();
  // The timeline's casts in seconds from its left edge, as drawn.
  const casts = () => {
    const width = Number(timeline.style.width.replace("px", ""));
    const seconds = Number(timeline.style.width && chartAxis.children.at(-1)?.textContent.split(":").reduce((m, s) => m * 60 + Number(s), 0));
    return timeline.drawn.arcs.map((x) => Math.round((x / width) * seconds));
  };
  return { stat, bars, icons, tab, casts, chartAxis, title: () => title.textContent, ui };
}

test("every target: the combat time is the fight's, a boss and its adds once, the gap between pulls left out", async () => {
  const view = await openEveryTarget(fightContext());
  // 80 s of the boss (its add inside it) and 10 s of the next pull; the
  // targets' spans added up were 100 s.
  assert.equal(view.stat("details.stats.combatTime"), "01:30");
  // The party bars' damage per second over the same time.
  assert.deepEqual(view.bars()[0].slice(1), ["100/s", "9.00k", "90.9%"]);
});

test("every target: the healing is the fight's, each tick once, whatever order the targets come in", async () => {
  for (const ctx of [fightContext(), { ...fightContext(), targets: fightContext().targets.reverse() }]) {
    const view = await openEveryTarget(ctx);
    view.tab("heal");
    // The boss's answer held 1,200 and the add's 300 (a tick in both); the
    // fight healed 1,400.
    assert.equal(view.stat("details.stats.totalHealing"), "1.40k");
    assert.equal(view.stat("details.stats.hps"), "16");
    assert.deepEqual(view.bars().map((b) => b.slice(1)), [["16/s", "1.40k", "100.0%"]]);
  }
});

test("every target: hit times count from the fight's first hit", async () => {
  const view = await openEveryTarget(fightContext());
  // The charts run from the fight's first hit to its last: 110 s.
  assert.equal(view.chartAxis.children.at(-1).textContent, "1:50");
  // Strike at 0 and 80 s, Cleave on the add at 30 and 40 s, Slash at 100
  // and 110 s: each target's own first hit is not the fight's.
  assert.deepEqual(view.casts().sort((a, b) => a - b), [0, 30, 40, 80, 100, 110]);
});

test("HEAL in a fight with no healing shows no bars, not the damage bars", async () => {
  const none = Object.fromEntries(Object.entries(answers).map(([id, a]) => [id, { ...a, healSkills: [] }]));
  const view = await openEveryTarget(fightContext([]), none);
  assert.equal(view.bars().length, 2, "DMG has its bars");
  view.tab("heal");
  assert.deepEqual(view.bars(), []);
  assert.equal(view.stat("details.stats.totalHealing"), "0");
});

// The date part of a title, as details.js writes it in English.
const titleDate = (ms) => {
  const d = new Date(ms);
  const h = d.getHours();
  const two = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())} @ ${h % 12 || 12}:${two(d.getMinutes())} ${h < 12 ? "AM" : "PM"}`;
};

test("every target: the title names the target with the most damage and the fight's first hit, whatever order the targets come in", async () => {
  const ctx = fightContext();
  for (const targets of [ctx.targets, [...ctx.targets].reverse(), [ctx.targets[2], ctx.targets[0], ctx.targets[1]]]) {
    const view = await openEveryTarget({ ...ctx, targets });
    // The boss took 8,000 of 9,900; the next pull began 100 s after it.
    assert.equal(view.title(), `Fight vs Mob 800 - ${titleDate(T0)}`);
  }
});

test("every target: the title names the target the meter follows, as its header does in BOSS", async () => {
  const view = await openEveryTarget({ ...fightContext(), currentTargetId: 801 });
  assert.equal(view.title(), `Fight vs Mob 801 - ${titleDate(T0)}`);
});

test("every target: the combat time is the context's, the time the meter counts", async () => {
  // TRAIN: your own 60 s on the dummies, not their 90 s.
  const view = await openEveryTarget({ ...fightContext(), battleTime: 60_000 });
  assert.equal(view.stat("details.stats.combatTime"), "01:00");
  assert.deepEqual(view.bars()[0].slice(1), ["150/s", "9.00k", "90.9%"]);
});

test("the party bars take each player's class from the context, not from whichever skill came last", async () => {
  // A Templar's skill rows: one Gladiator-coded skill among them.
  const mixed = {
    ...answers,
    800: { ...answers[800], skills: [{ ...skill(1, 12010000, "Shield Bash", 7000, [0, 80_000]), job: "수호성" },
      { ...skill(1, 11340000, "Lifestealing Blade", 1000, [40_000]), job: "검성" }] },
  };
  const ctx = { ...fightContext(), actors: [{ actorId: 1, nickname: "One", job: "수호성" }, { actorId: 2, nickname: "Two", job: "" }] };
  for (const order of [mixed[800].skills, [...mixed[800].skills].reverse()]) {
    const view = await openEveryTarget(ctx, { ...mixed, 800: { ...mixed[800], skills: order } });
    assert.equal(view.icons()[0], "수호성");
  }
});
