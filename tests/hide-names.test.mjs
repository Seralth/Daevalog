// "Hide other players' names": with the setting on, no other player's name
// reaches the page in any view these tests can draw.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { loadScripts } from "./scripts.mjs";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const en = JSON.parse(read("../src/data/i18n/ui/en.json"));
const lookup = (key) => key.split(".").reduce((o, k) => o?.[k], en);
const i18n = {
  t: (key, fallback) => lookup(key) ?? fallback,
  format: (key, vars, fallback) =>
    String(lookup(key) ?? fallback).replace(/\{(\w+)\}/g, (_, k) => String(vars?.[k] ?? "")),
  getLanguage: () => "en",
};

// Made-up names. A saved fight keeps other players masked, so the masked
// form must not show either.
const ME = "Selfname";
const OTHER = "Othername";
const THIRD = "Thirdname";
const MASKED = "Ot****e";
const SECRETS = [OTHER, THIRD, MASKED];

// Just enough DOM to draw into, keeping every text, markup and attribute.
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
    this.html = "";
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
    this.offsetWidth = 100;
    this.offsetHeight = 20;
    this.clientWidth = 400;
  }
  set className(v) { this.classes = new Set(String(v).split(/\s+/).filter(Boolean)); }
  get className() { return [...this.classes].join(" "); }
  set textContent(v) { this.children = []; this.html = ""; this.text = String(v); }
  get textContent() { return this.text + this.children.map((c) => c.textContent).join(""); }
  set innerHTML(v) { this.children = []; this.text = ""; this.html = String(v); }
  get innerHTML() { return this.html; }
  set src(v) { this.attrs.set("src", String(v)); }
  set alt(v) { this.attrs.set("alt", String(v)); }
  set title(v) { this.attrs.set("title", String(v)); }
  setAttribute(k, v) { if (k === "class") this.className = v; else this.attrs.set(k, String(v)); }
  getAttribute(k) { return this.attrs.get(k) ?? null; }
  removeAttribute(k) { this.attrs.delete(k); }
  appendChild(c) {
    if (c.parentNode) c.parentNode.children = c.parentNode.children.filter((x) => x !== c);
    c.parentNode = this;
    this.children.push(c);
    return c;
  }
  append(...cs) { cs.forEach((c) => this.appendChild(c)); }
  prepend(c) { this.appendChild(c); this.children.unshift(this.children.pop()); }
  replaceChildren(...cs) { this.children = []; this.text = ""; this.html = ""; cs.forEach((c) => this.appendChild(c)); }
  remove() { if (this.parentNode) this.parentNode.children = this.parentNode.children.filter((x) => x !== this); }
  get firstChild() { return this.children[0] ?? null; }
  addEventListener(type, fn) { this.listeners.set(type, [...(this.listeners.get(type) || []), fn]); }
  click() { (this.listeners.get("click") || []).forEach((fn) => fn({ target: this })); }
  getContext() { return { measureText: (t) => ({ width: String(t).length * 6 }) }; }
  getBoundingClientRect() { return { left: 0, top: 0, right: 100, bottom: 20, width: 100, height: 20 }; }
  matches(compound) {
    const classes = [...compound.matchAll(/\.([\w-]+)/g)].map((m) => m[1]);
    const attrs = [...compound.matchAll(/\[([\w-]+)(?:="([^"]*)")?\]/g)];
    return classes.every((c) => this.classes.has(c)) &&
      attrs.every(([, a, v]) => this.attrs.has(a) && (v === undefined || this.attrs.get(a) === v));
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
}

// Everything the element and its children put on screen or in an attribute.
const pageText = (el) =>
  [el.text, el.html, ...el.attrs.values(), ...el.children.map(pageText)].join("\n");

const assertNoNames = (el, what) => {
  const text = pageText(el);
  for (const name of SECRETS) assert.ok(!text.includes(name), `${what} shows "${name}"`);
};

const scripts = (files, extra = {}) => {
  const window = { i18n, addEventListener() {}, ...extra.window };
  return loadScripts(["shared/format.js", "shared/jobs.js", "shared/players.js", ...files], {
    window,
    console,
    document: {
      createElement: (tag) => new El(tag),
      documentElement: new El("html"),
      readyState: "loading",
      addEventListener() {},
      querySelector: () => null,
    },
    getComputedStyle: () => ({ getPropertyValue: () => "36", fontSize: "14px", fontFamily: "sans-serif" }),
    requestAnimationFrame: (fn) => { fn(); return 0; },
    cancelAnimationFrame() {},
    setInterval: () => 0,
    clearInterval() {},
    ...extra.globals,
  });
};

const hide = (context, on) => vm.runInContext(`playerNames.hideOthers = ${on};`, context);

const meterRows = [
  { id: "100", name: ME, job: "치유성", dps: 900, totalDamage: 9000, isUser: true, number: 0 },
  { id: "200", name: OTHER, job: "검성", dps: 800, totalDamage: 8000, number: 2 },
  { id: "300", name: "300", job: "", dps: 700, totalDamage: 7000, number: 3, isIdentifying: true },
  { id: "400", name: THIRD, job: "마도성", dps: 600, totalDamage: 6000, number: 1 },
  { id: "80000000", name: "Unattributed summons and effects", job: "Unknown", dps: 10, totalDamage: 100 },
];

const drawMeter = (on) => {
  const context = scripts(["meter.js"]);
  hide(context, on);
  const elList = new El("div");
  const meter = vm.runInContext("createMeterUI", context)({
    elList,
    dpsFormatter: new Intl.NumberFormat("en-US"),
    getMetric: (row) => ({ value: row.dps, text: `${row.dps}/s` }),
    getSortDirection: () => "desc",
    getPlayerLimit: () => 8,
  });
  meter.updateFromRows(meterRows);
  return elList;
};

test("meter rows show other players by class and number, and you by name", () => {
  const names = drawMeter(true).querySelectorAll(".name").map((el) => el.textContent);
  assert.deepEqual(names, [ME, "Gladiator 2", "Player 3", "Sorcerer 1", "Unattributed summons and effects"]);
  assertNoNames(drawMeter(true), "the meter");
});

test("the check catches a name: with the setting off the meter shows names", () => {
  const text = pageText(drawMeter(false));
  assert.ok(text.includes(OTHER) && text.includes(THIRD));
});

test("the hover tooltip names another player by class and number", () => {
  const context = scripts(["shared/targetModes.js", "core.js"]);
  const app = vm.runInContext("Object.create(DpsApp.prototype)", context);
  app.dpsFormatter = new Intl.NumberFormat("en-US");
  app.i18n = i18n;
  app.positionHoverTooltip = () => {};
  app.hoverTooltipEl = new El("div");
  const row = { id: "200", name: OTHER, job: "검성", number: 2, dps: 800, totalDamage: 8000 };
  hide(context, true);
  app.renderHoverTooltip({ skills: [], state: "empty" }, row, new El("div"));
  assert.ok(app.hoverTooltipEl.innerHTML.includes("Gladiator 2"));
  assertNoNames(app.hoverTooltipEl, "the hover tooltip");
  hide(context, false);
  app.renderHoverTooltip({ skills: [], state: "empty" }, row, new El("div"));
  assert.ok(app.hoverTooltipEl.innerHTML.includes(OTHER), "the check would catch it");
});

// The Details panel, wired as core.js wires it: you by the backend's id.
const detailsSetup = ({ context: detailsContext = null, details, localId = 100 }) => {
  const context = scripts(["details.js"]);
  const panel = new El("div");
  panel.className = "detailsPanel";
  const skills = panel.appendChild(new El("div"));
  skills.className = "detailsSkills";
  const title = new El("div");
  const party = new El("div");
  const ui = vm.runInContext("createDetailsUI", context)({
    detailsPanel: panel,
    detailsFightTitleEl: title,
    detailsPartyListEl: party,
    detailsStatsEl: new El("div"),
    skillsListEl: skills.appendChild(new El("div")),
    dpsFormatter: new Intl.NumberFormat("en-US"),
    getDetails: async () => details,
    getDetailsContext: () => detailsContext,
    getDungeonId: () => 0,
    isUser: (id, name, { saved = false } = {}) => (!saved && Number(id) === localId) || name === ME,
  });
  return { context, ui, panel, title, party };
};

const liveDetails = {
  skills: [
    { actorId: 100, code: 17010000, name: "Smite", time: 3, dmg: 3000, job: "치유성" },
    { actorId: 200, code: 11010000, name: "Slash", time: 3, dmg: 2000, job: "검성" },
    { actorId: 400, code: 15010000, name: "Bolt", time: 3, dmg: 1000, job: "마도성" },
  ],
  healSkills: [],
  perActorStats: [],
  totalDmg: 6000,
  battleTimeMs: 10000,
};

test("Details names other players by the meter's numbers", async () => {
  const { context, ui, party, title } = detailsSetup({
    details: liveDetails,
    context: {
      currentTargetId: 900,
      targets: [{ targetId: 900, targetName: "Boss", battleTime: 10000, lastDamageTime: 20000, totalDamage: 6000,
        actorDamage: { 100: 3000, 200: 2000, 400: 1000 } }],
      actors: [
        { actorId: 100, nickname: ME, job: "치유성" },
        { actorId: 200, nickname: OTHER, job: "검성" },
        { actorId: 400, nickname: THIRD, job: "마도성" },
      ],
      numbers: { 200: 2, 400: 1 },
    },
  });
  hide(context, true);
  await ui.open({ id: "200", name: OTHER, job: "검성", number: 2 }, { force: true, defaultTargetAll: true });
  const bars = party.querySelectorAll(".detailsPartyBarName").map((el) => el.textContent);
  assert.deepEqual(bars, [ME, "Gladiator 2", "Sorcerer 1"]);
  assertNoNames(party, "the Details party list");
  assertNoNames(title, "the Details title");
  hide(context, false);
  await ui.refresh();
  assert.ok(pageText(party).includes(OTHER), "the check would catch it");
});

test("a Details view titled after a player names them by class and number", async () => {
  const { context, ui, title } = detailsSetup({ details: { ...liveDetails, skills: [] } });
  hide(context, true);
  await ui.open({ id: "200", name: OTHER, job: "검성", number: 2 }, { force: true });
  assert.ok(title.innerHTML.includes("Gladiator 2"));
  assertNoNames(title, "the Details title");
});

test("a saved fight numbers other players by first hit and keeps you by name", async () => {
  const record = {
    id: "auto_900_1",
    bossName: "Boss",
    targetId: 900,
    mobCode: 0,
    totalDamage: 6000,
    startTimeMs: 1000,
    durationMs: 10000,
    actors: [
      { actorId: 7, nickname: ME, job: "치유성" },
      { actorId: 8, nickname: MASKED, job: "검성" },
      { actorId: 9, nickname: "Th****e", job: "마도성" },
    ],
    details: {
      skills: [
        { actorId: 7, code: 17010000, dmg: 3000, hitTimestamps: [1000] },
        { actorId: 8, code: 11010000, dmg: 2000, hitTimestamps: [3000, 2500] },
        { actorId: 9, code: 15010000, dmg: 1000, hitTimestamps: [1500] },
      ],
    },
  };
  const processed = {
    ...liveDetails,
    skills: [
      { actorId: 7, code: 17010000, name: "Smite", time: 1, dmg: 3000, job: "치유성" },
      { actorId: 8, code: 11010000, name: "Slash", time: 2, dmg: 2000, job: "검성" },
      { actorId: 9, code: 15010000, name: "Bolt", time: 1, dmg: 1000, job: "마도성" },
    ],
  };
  // Id 8 is this session's own id: a saved fight's ids are from its session.
  const { context, ui, party, title } = detailsSetup({ details: processed, localId: 8 });
  hide(context, true);
  await ui.openHistoryFight(record);
  const bars = party.querySelectorAll(".detailsPartyBarName").map((el) => el.textContent);
  assert.deepEqual(bars, [ME, "Gladiator 2", "Sorcerer 1"]);
  assertNoNames(party, "the saved fight's party list");
  assertNoNames(title, "the saved fight's title");
});

test("the setting is read at start and follows a change from the Settings window", () => {
  const context = scripts(["shared/targetModes.js", "core.js", "settings/panel.js"]);
  const app = vm.runInContext("Object.create(DpsApp.prototype)", context);
  const calls = [];
  app.storageKeys = { hideOtherNames: "dpsMeter.hideOtherNames" };
  app.renderCurrentRows = () => calls.push("meter");
  app.hideHoverTooltip = () => calls.push("tooltip");
  app.detailsUI = { refresh: () => calls.push("details"), relabelHistoryFight: () => calls.push("saved fight") };
  app.safeSetSetting = (key, value) => calls.push(`${key}=${value}`);
  app.setHideOtherNames(true, { persist: true });
  assert.equal(vm.runInContext("playerNames.hideOthers", context), true);
  assert.deepEqual(calls, ["dpsMeter.hideOtherNames=true", "meter", "tooltip", "details", "saved fight"]);
  assert.equal(vm.runInContext("REMOTE_APPLIED_SETTING_CONTROLS", context)["dpsMeter.hideOtherNames"], ".hideOtherNamesCheckbox");
  const markup = read("../index.html");
  assert.ok(markup.includes('class="hideOtherNamesCheckbox"'));
  assert.ok(!/class="hideOtherNamesCheckbox"[^>]*checked/.test(markup), "off by default");
});
