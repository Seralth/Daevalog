// Times of day follow Settings' Display Time: 12-hour or 24-hour, else the
// system's clock, else 12-hour. Fight lengths are durations and never change.
import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { loadScripts } from "./scripts.mjs";

// Local times, so the checks hold in any time zone.
const at = (h, m, s = 0) => new Date(2026, 9, 7, h, m, s).getTime();
const MIDNIGHT = at(0, 0, 7);
const NOON = at(12, 0, 7);

const page = (window = {}) => loadScripts(["shared/format.js"], { window, console });

test("24-hour times: midnight is 00:00, noon is 12:00", () => {
  const formatClock = vm.runInContext("formatClock", page());
  const h24 = (ms, options = {}) => formatClock(ms, { hour24: true, ...options });
  assert.equal(h24(MIDNIGHT), "00:00");
  assert.equal(h24(NOON), "12:00");
  assert.equal(h24(at(0, 30)), "00:30");
  assert.equal(h24(at(9, 5)), "09:05");
  assert.equal(h24(at(23, 59)), "23:59");
  assert.equal(h24(MIDNIGHT, { seconds: true }), "00:00:07");
  assert.equal(h24(NOON, { seconds: true, lang: "ko" }), "12:00:07", "the same in every language");
});

test("12-hour times: midnight is 12:00 AM, noon is 12:00 PM", () => {
  const formatClock = vm.runInContext("formatClock", page());
  assert.equal(formatClock(MIDNIGHT), "12:00 AM");
  assert.equal(formatClock(NOON), "12:00 PM");
  assert.equal(formatClock(at(0, 30)), "12:30 AM");
  assert.equal(formatClock(at(11, 59)), "11:59 AM");
  assert.equal(formatClock(at(14, 59)), "2:59 PM");
  assert.equal(formatClock(at(23, 59)), "11:59 PM");
  assert.equal(formatClock(NOON, { seconds: true }), "12:00:07 PM");
  // Korean and Chinese put the morning or afternoon word first.
  assert.equal(formatClock(MIDNIGHT, { lang: "ko" }), "오전 12:00");
  assert.equal(formatClock(NOON, { lang: "ko" }), "오후 12:00");
  assert.equal(formatClock(MIDNIGHT, { lang: "zh-Hans" }), "上午 12:00");
  assert.equal(formatClock(NOON, { lang: "zh-Hant" }), "下午 12:00");
  assert.equal(formatClock(NOON, { lang: "de" }), "12:00 PM");
  assert.equal(formatClock(Number.NaN), "");
});

test("the setting wins, then the system's clock, then 12-hour", () => {
  const cases = [
    [{ "dpsMeter.timeFormat": "24h" }, "12h", "00:00"],
    [{ "dpsMeter.timeFormat": "12h" }, "24h", "12:00 AM"],
    [{}, "24h", "00:00"],
    [{}, "12h", "12:00 AM"],
    [{}, null, "12:00 AM"],
    [{ "dpsMeter.timeFormat": "something else" }, "24h", "00:00"],
  ];
  for (const [settings, system, expected] of cases) {
    const window = {
      javaBridge: { getSetting: (key) => settings[key] ?? null, systemTimeFormat: () => system },
      i18n: { getLanguage: () => "en" },
    };
    assert.equal(vm.runInContext("clockText", page(window))(MIDNIGHT), expected, JSON.stringify([settings, system]));
  }
  // No bridge at all (a page without the backend): 12-hour.
  assert.equal(vm.runInContext("clockText", page())(NOON), "12:00 PM");
});

// Just enough DOM for the History list: elements found by one class name.
class El {
  constructor(tag = "div") {
    this.tagName = tag.toUpperCase();
    this.children = [];
    this.classes = new Set();
    this.text = "";
    this.style = {};
    this.dataset = {};
    this.attrs = {};
    this.listeners = {};
    this.classList = {
      add: (...c) => c.forEach((x) => this.classes.add(x)),
      remove: (...c) => c.forEach((x) => this.classes.delete(x)),
      contains: (c) => this.classes.has(c),
      toggle: (c, force = !this.classes.has(c)) => (force ? this.classes.add(c) : this.classes.delete(c), force),
    };
  }
  set className(v) { this.classes = new Set(String(v).split(/\s+/).filter(Boolean)); }
  get className() { return [...this.classes].join(" "); }
  set textContent(v) { this.children = []; this.text = String(v); }
  get textContent() { return this.text + this.children.map((c) => c.textContent).join(""); }
  set innerHTML(v) { this.children = []; this.text = ""; }
  appendChild(c) {
    if (c.fragment) { c.children.forEach((x) => this.appendChild(x)); return c; }
    this.children.push(c);
    return c;
  }
  setAttribute(k, v) { this.attrs[k] = String(v); }
  getAttribute(k) { return this.attrs[k] ?? null; }
  addEventListener(type, fn) { (this.listeners[type] ||= []).push(fn); }
  contains() { return false; }
  all() { return [this, ...this.children.flatMap((c) => c.all())]; }
  querySelectorAll(selector) {
    const name = /^\.([\w-]+)$/.exec(selector)?.[1];
    return name ? this.all().filter((e) => e !== this && e.classes.has(name)) : [];
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
}

// History in list view, its fights at midnight and noon.
function openHistory(settings, system = null) {
  const panel = new El();
  panel.className = "historyPanel";
  for (const name of ["historyList", "historyEmpty"]) {
    const el = new El();
    el.className = name;
    panel.appendChild(el);
  }
  const fight = (id, startTimeMs) => ({ id, bossName: `Boss ${id}`, targetId: id, startTimeMs, durationMs: 89_000, totalDamage: 1000, jobs: [] });
  const listeners = {};
  const window = {
    addEventListener: (type, fn) => (listeners[type] ||= []).push(fn),
    javaBridge: {
      getSetting: (key) => (key === "historyViewMode" ? "list" : settings[key] ?? null),
      systemTimeFormat: () => system,
      getFightHistory: () => JSON.stringify([fight(2, NOON), fight(1, MIDNIGHT)]),
    },
    i18n: { getLanguage: () => "en" },
  };
  const document = {
    querySelector: (selector) => (selector === ".historyPanel" ? panel : null),
    createElement: (tag) => new El(tag),
    createDocumentFragment: () => Object.assign(new El(), { fragment: true }),
    createTextNode: (text) => Object.assign(new El(), { text }),
    addEventListener() {},
  };
  const context = loadScripts(["shared/format.js", "shared/jobs.js", "history.js"], { window, document, console });
  const ui = vm.runInContext("createHistoryUI", context)({});
  ui.open();
  const texts = (name) => panel.querySelectorAll(`.${name}`).map((e) => e.textContent);
  const emit = (type) => (listeners[type] || []).forEach((fn) => fn());
  return { times: () => texts("historyRowTime"), lengths: () => texts("historyRowDuration"), emit };
}

test("History rows: the day and the time of day in the chosen format, the fight's length as it was", () => {
  const h24 = openHistory({ "dpsMeter.timeFormat": "24h" });
  assert.deepEqual(h24.times(), ["2026-10-07 12:00", "2026-10-07 00:00"]);
  const h12 = openHistory({ "dpsMeter.timeFormat": "12h" });
  assert.deepEqual(h12.times(), ["2026-10-07 12:00 PM", "2026-10-07 12:00 AM"]);
  assert.deepEqual(h12.lengths(), ["01:29", "01:29"], "a duration, not a time of day");
  // Not picked yet: the system's clock.
  assert.deepEqual(openHistory({}, "24h").times(), ["2026-10-07 12:00", "2026-10-07 00:00"]);
});

test("History redraws its times as soon as Display Time changes", () => {
  const settings = { "dpsMeter.timeFormat": "12h" };
  const history = openHistory(settings);
  assert.deepEqual(history.times(), ["2026-10-07 12:00 PM", "2026-10-07 12:00 AM"]);
  settings["dpsMeter.timeFormat"] = "24h";
  history.emit("clock-format-changed");
  assert.deepEqual(history.times(), ["2026-10-07 12:00", "2026-10-07 00:00"]);
});
