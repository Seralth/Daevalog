// The footer's mode button opens a list of the modes; picking one sets it.
import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { loadScripts } from "./scripts.mjs";

// Just enough DOM for the button, the list and their events.
class El {
  constructor(doc, tag) {
    this.doc = doc;
    this.tagName = tag.toUpperCase();
    this.nodeType = 1;
    this.children = [];
    this.parentElement = null;
    this.attrs = new Map();
    this.classes = new Set();
    this.listeners = new Map();
    this.style = {};
    this.text = "";
    this.rect = { left: 0, top: 0, right: 0, bottom: 0 };
    this.offsetParent = null;
    this.clientLeft = 0;
    this.clientTop = 0;
    this.classList = {
      add: (...c) => c.forEach((x) => this.classes.add(x)),
      remove: (...c) => c.forEach((x) => this.classes.delete(x)),
      contains: (c) => this.classes.has(c),
      toggle: (c, force = !this.classes.has(c)) => (force ? this.classes.add(c) : this.classes.delete(c), force),
    };
    this.dataset = new Proxy({}, {
      get: (_, k) => this.attrs.get(`data-${String(k)}`),
      set: (_, k, v) => (this.attrs.set(`data-${String(k)}`, String(v)), true),
    });
  }
  // A column of five modes, or one row of them.
  get offsetWidth() { return this.classes.has("isRow") ? 280 : 70; }
  get offsetHeight() { return this.classes.has("isRow") ? 24 : 110; }
  set className(v) { this.classes = new Set(String(v).split(/\s+/).filter(Boolean)); }
  set textContent(v) { this.children = []; this.text = String(v); }
  get textContent() { return this.text + this.children.map((c) => c.textContent).join(""); }
  setAttribute(k, v) { this.attrs.set(k, String(v)); }
  getAttribute(k) { return this.attrs.get(k) ?? null; }
  appendChild(c) { c.parentElement = this; this.children.push(c); return c; }
  replaceChildren(...cs) { this.children = []; cs.forEach((c) => this.appendChild(c)); }
  addEventListener(type, fn) { this.listeners.set(type, [...(this.listeners.get(type) || []), fn]); }
  dispatch(type, init = {}) {
    const event = { target: this, ...init, preventDefault() { event.defaultPrevented = true; } };
    (this.listeners.get(type) || []).forEach((fn) => fn(event));
    return event;
  }
  click(detail = 1) { return this.dispatch("click", { detail }); }
  focus() { this.doc.activeElement = this; }
  getBoundingClientRect() { return this.rect; }
  matches(compound) {
    const classes = [...compound.matchAll(/\.([\w-]+)/g)].map((m) => m[1]);
    const attrs = [...compound.matchAll(/\[([\w-]+)(?:="([^"]*)")?\]/g)];
    return classes.every((c) => this.classes.has(c)) &&
      attrs.every(([, a, v]) => this.attrs.has(a) && (v === undefined || this.attrs.get(a) === v));
  }
  closest(list) {
    for (let el = this; el; el = el.parentElement) {
      if (list.split(",").some((s) => el.matches(s.trim()))) return el;
    }
    return null;
  }
  descendants() { return this.children.flatMap((c) => [c, ...c.descendants()]); }
  querySelectorAll(selector) { return this.descendants().filter((el) => el.matches(selector)); }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
}

function setup({ mode = "bossTargets", buttonTop = 300, place = null, screenY = 0, screenHeight = 1080 } = {}) {
  const docListeners = new Map();
  const document = {
    activeElement: null,
    readyState: "loading",
    addEventListener: (type, fn) => docListeners.set(type, [...(docListeners.get(type) || []), fn]),
  };
  document.createElement = (tag) => new El(document, tag);
  document.body = new El(document, "body");
  document.documentElement = new El(document, "html");
  const root = new El(document, "div");
  const meter = root.appendChild(new El(document, "div"));
  meter.className = "meter";
  meter.rect = { left: 8, top: 8, right: 388, bottom: buttonTop + 20 };
  const button = meter.appendChild(new El(document, "button"));
  button.className = "footerBtn targetModeBtn";
  button.rect = { left: 300, top: buttonTop, right: 344, bottom: buttonTop + 16 };
  const menu = meter.appendChild(new El(document, "div"));
  menu.className = "targetModeMenu";
  menu.offsetParent = meter;
  const other = meter.appendChild(new El(document, "div"));
  other.className = "list";
  document.querySelector = (selector) => root.querySelector(selector);

  const calls = [];
  const timers = [];
  const window = {
    screen: { availTop: 0, availHeight: screenHeight },
    screenY,
    innerHeight: buttonTop + 40,
    addEventListener() {},
    javaBridge: {
      setTargetSelection: (value) => calls.push(`backend ${value}`),
      updateOverlaySize: () => calls.push("size"),
      getOverlayPlace: () => Promise.resolve(place),
    },
  };
  const context = loadScripts(
    ["shared/targetModes.js", "core.js", "settings/panel.js", "app/windowChrome.js", "app/modeMenu.js"],
    {
      window, console, document,
      setTimeout: (fn, ms) => timers.push({ fn, ms }),
      clearTimeout: (id) => { if (id) timers[id - 1] = null; },
    },
  );
  const app = vm.runInContext("Object.create(DpsApp.prototype)", context);
  Object.assign(app, {
    targetModeBtn: button,
    targetSelection: mode,
    isCollapse: false,
    storageKeys: { targetSelection: "dpsMeter.targetSelection" },
    safeSetSetting: (key, value) => calls.push(`${key}=${value}`),
    logDebug() {},
    hideHoverTooltip: () => calls.push("tooltip hidden"),
    fetchDps: () => calls.push("fetch"),
  });
  app.bindTargetModeMenu();
  const press = (target) => (docListeners.get("pointerdown") || []).forEach((fn) => fn({ target }));
  // Runs the timers still set; returns their delays.
  const runTimers = () => timers.splice(0).filter(Boolean).map(({ fn, ms }) => (fn(), ms));
  const activity = (type) => (docListeners.get(type) || []).forEach((fn) => fn({}));
  return { app, button, menu, other, document, calls, press, runTimers, activity };
}

// Opening waits for the backend to say where a layer overlay is.
const settle = () => new Promise((resolve) => setImmediate(resolve));
const open = async (button, detail = 1) => { button.click(detail); await settle(); };
const labels = (menu) => menu.querySelectorAll(".targetModeItem").map((el) => el.textContent);
const checked = (menu) => menu.querySelectorAll('.targetModeItem[aria-checked="true"]').map((el) => el.textContent);

test("the mode button opens a list of the five modes with the current one marked", async () => {
  const { button, menu, calls } = setup({ mode: "bossTargets" });
  assert.ok(!menu.classList.contains("isOpen"));
  await open(button);
  assert.ok(menu.classList.contains("isOpen"));
  assert.equal(button.getAttribute("aria-expanded"), "true");
  assert.deepEqual(labels(menu), ["TARGET", "BOSS", "ALL", "TRAIN", "ENC"]);
  assert.deepEqual(checked(menu), ["BOSS"]);
  assert.ok(calls.includes("tooltip hidden"));
  button.click();
  assert.ok(!menu.classList.contains("isOpen"), "a second click closes it");
  assert.equal(button.getAttribute("aria-expanded"), "false");
});

test("picking a mode sets it, saves it, tells the backend and closes the list", async () => {
  const { app, button, menu, calls } = setup({ mode: "bossTargets" });
  await open(button);
  calls.length = 0;
  menu.querySelectorAll(".targetModeItem")[3].click();
  assert.equal(app.targetSelection, "trainTargets");
  assert.ok(calls.includes("dpsMeter.targetSelection=trainTargets"));
  assert.ok(calls.includes("backend trainTargets"));
  assert.ok(calls.includes("fetch"));
  assert.ok(!menu.classList.contains("isOpen"));
  assert.equal(button.textContent, "TRAIN");
  assert.deepEqual(checked(menu), ["TRAIN"]);
});

test("picking the current mode only closes the list", async () => {
  const { button, menu, calls } = setup({ mode: "encounter" });
  await open(button);
  calls.length = 0;
  menu.querySelector('.targetModeItem[data-mode="encounter"]').click();
  assert.ok(!menu.classList.contains("isOpen"));
  assert.deepEqual(calls.filter((c) => c !== "size"), []);
});

test("keyboard: arrows open the list and move through it, Escape gives the focus back", async () => {
  const { app, button, menu, document } = setup({ mode: "allTargets" });
  button.dispatch("keydown", { key: "ArrowDown" });
  await settle();
  assert.ok(menu.classList.contains("isOpen"));
  assert.equal(document.activeElement.textContent, "ALL", "the current mode has the focus");
  menu.dispatch("keydown", { key: "ArrowDown" });
  assert.equal(document.activeElement.textContent, "TRAIN");
  menu.dispatch("keydown", { key: "End" });
  menu.dispatch("keydown", { key: "ArrowDown" });
  assert.equal(document.activeElement.textContent, "TARGET", "the list wraps around");
  menu.dispatch("keydown", { key: "Escape" });
  assert.ok(!menu.classList.contains("isOpen"));
  assert.equal(document.activeElement, button);
  // Enter on the button and Enter on an item arrive as clicks with detail 0.
  await open(button, 0);
  assert.equal(document.activeElement.textContent, "ALL");
  menu.querySelectorAll(".targetModeItem")[0].click(0);
  assert.equal(app.targetSelection, "lastHitByMe");
  assert.equal(document.activeElement, button);
  // Opened with the mouse, the focus stays on the button until an arrow key.
  await open(button);
  button.focus();
  button.dispatch("keydown", { key: "ArrowDown" });
  assert.equal(document.activeElement.textContent, "TARGET");
  button.focus();
  button.dispatch("keydown", { key: "Escape" });
  assert.ok(!menu.classList.contains("isOpen"));
});

test("a press outside the list closes it, a press on it does not", async () => {
  const { menu, other, button, press } = setup();
  await open(button);
  press(menu.querySelectorAll(".targetModeItem")[1]);
  assert.ok(menu.classList.contains("isOpen"));
  press(other);
  assert.ok(!menu.classList.contains("isOpen"));
});

test("the pointer leaving the window closes the list soon after", async () => {
  const { menu, button, document, runTimers } = setup();
  await open(button);
  document.documentElement.dispatch("mouseleave");
  assert.deepEqual(runTimers(), [400]);
  assert.ok(!menu.classList.contains("isOpen"));
});

test("the list closes after five seconds without pointer or key activity", async () => {
  const { menu, button, runTimers, activity } = setup();
  await open(button);
  activity("pointermove");
  activity("keydown");
  assert.deepEqual(runTimers(), [5000], "activity restarts the one timer");
  assert.ok(!menu.classList.contains("isOpen"));
});

test("locking the overlay closes the list", async () => {
  const { app, button, menu } = setup();
  Object.assign(app, { safeSetSetting() {}, _applyLockBtnVisibility() {} });
  await open(button);
  app._onOverlayLockChanged(true);
  assert.ok(!menu.classList.contains("isOpen"));
  await open(button);
  assert.ok(!menu.classList.contains("isOpen"), "a locked overlay does not open it");
});

test("the list opens above the button when the meter has room, else below it, inside the window", async () => {
  const high = setup({ buttonTop: 300 });
  await open(high.button);
  // 110 high, 4 px over the button, right edges lined up; the meter's corner is the origin.
  assert.equal(high.menu.style.top, `${300 - 4 - 110 - 8}px`);
  assert.equal(high.menu.style.left, `${344 - 70 - 8}px`);
  assert.ok(high.calls.includes("size"), "the window is told to fit the list");

  const low = setup({ buttonTop: 60 });
  await open(low.button);
  assert.equal(low.menu.style.top, `${60 + 16 + 4 - 8}px`);
  assert.ok(!low.menu.classList.contains("isRow"));
});

test("a short meter at the bottom of the screen gets the modes in one row above the button", async () => {
  // A layer overlay: the page reads screenY 0, the backend knows the place.
  const layer = setup({ buttonTop: 60, place: [0, 700], screenHeight: 800 });
  await open(layer.button);
  assert.ok(layer.menu.classList.contains("isRow"));
  assert.equal(layer.menu.style.top, `${60 - 4 - 24 - 8}px`);
  assert.equal(layer.menu.style.left, `${344 - 280 - 8}px`);
  // A normal window knows its own place.
  const x11 = setup({ buttonTop: 60, screenY: 700, screenHeight: 800 });
  await open(x11.button);
  assert.ok(x11.menu.classList.contains("isRow"));
  // The same layer overlay at the top of the screen opens a column below.
  const top = setup({ buttonTop: 60, place: [0, 0], screenHeight: 800 });
  await open(top.button);
  assert.ok(!top.menu.classList.contains("isRow"));
  assert.equal(top.menu.style.top, `${60 + 16 + 4 - 8}px`);
  layer.button.click();
  assert.ok(!layer.menu.classList.contains("isRow"), "closing drops the row layout");
});
