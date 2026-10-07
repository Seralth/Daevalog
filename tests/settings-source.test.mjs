// settings.json is the one store for settings. The page's own storage is only
// read once, to hand old values to the backend, and never for a setting.
import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { loadScripts } from "./scripts.mjs";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const bridgeSource = read("../public/src/js/tauriBridge.js");
const noop = () => {};

// The page's WebKit storage. `writes` records every change the page makes.
function pageStorage(entries = {}) {
  const store = new Map(Object.entries(entries));
  const writes = [];
  return {
    store, writes,
    get length() { return store.size; },
    key: (i) => [...store.keys()][i] ?? null,
    getItem: (key) => (store.has(key) ? store.get(key) : null),
    setItem: (key, value) => { writes.push(`set ${key}`); store.set(key, String(value)); },
    removeItem: (key) => { writes.push(`remove ${key}`); store.delete(key); },
    clear: () => { writes.push("clear"); store.clear(); },
  };
}

// Loads the whole bridge. `settings` is settings.json; `adopt` answers the
// hand-over (default: the backend keeps every value it was given).
function loadBridge({ settings = {}, storage = pageStorage(), adopt, view } = {}) {
  const calls = [];
  const listeners = new Map();
  const invoke = (command, args) => {
    calls.push({ command, args });
    if (command === "get_settings") return Promise.resolve({ ...settings });
    if (command === "adopt_page_settings") {
      return adopt ? adopt(args.values) : Promise.resolve({ ...args.values, ...settings });
    }
    return Promise.resolve(null);
  };
  const classList = { add: noop, remove: noop, toggle: noop };
  const window = {
    A2_VIEW: "main", devicePixelRatio: 1, location: { search: "" }, localStorage: storage, addEventListener: noop,
    __A2_VIEW__: view,
    __TAURI__: {
      core: { invoke },
      event: { listen: (name, handler) => { listeners.set(name, handler); return Promise.resolve(noop); }, emit: noop },
      window: { getCurrentWindow: () => ({ label: "main" }) },
    },
  };
  const document = {
    readyState: "complete", addEventListener: noop, querySelector: () => null, querySelectorAll: () => [],
    createElement: () => ({ style: {}, classList, setAttribute: noop, appendChild: noop }),
    documentElement: { classList }, body: { classList, appendChild: noop }, head: { appendChild: noop },
  };
  vm.runInContext(bridgeSource, vm.createContext({
    window, document, navigator: { userAgent: "Linux" },
    console: { log: noop, warn: noop, error: noop },
    setInterval: () => 0, clearInterval: noop, setTimeout: () => 0, clearTimeout: noop,
    requestAnimationFrame: noop, MutationObserver: class { observe() {} }, URLSearchParams,
  }), { filename: "tauriBridge.js" });
  const commands = (name) => calls.filter((call) => call.command === name);
  return { bridge: window.javaBridge, storage, calls, commands, listeners };
}

test("the layer switch case: a value only the page's storage holds is never read", async () => {
  // COSMIC, 2026-10-07: settings.json had no layer key, the page's storage
  // still had "true" from a test the day before. The backend keeps the value
  // out (only it acts on the layer), so the page must show the default.
  const storage = pageStorage({ "dpsMeter.waylandLayer": "true", unrelated: "kept" });
  const { bridge } = loadBridge({ storage, adopt: () => Promise.resolve({}) });
  await bridge.settingsReady;
  assert.equal(bridge.getSetting("dpsMeter.waylandLayer"), null);
  assert.deepEqual([...storage.store.keys()], ["unrelated"], "the old copy is gone, other keys stay");
});

test("the page's old values are handed to the backend once, and its answer is what the page reads", async () => {
  const storage = pageStorage({
    "dpsMeter.theme": "ember",
    "dpsMeter.showPing": "false",
    "window.x": "5",
    "backend.screenshotFolder": "/tmp",
    historyViewMode: "list",
    historyShowTraining: "0",
    unrelated: "kept",
  });
  // settings.json wins over the page's copy; the backend says what moved.
  const settings = { "dpsMeter.theme": "frost" };
  const first = loadBridge({
    settings, storage,
    adopt: () => Promise.resolve({ ...settings, "dpsMeter.showPing": "false", historyViewMode: "list" }),
  });
  await first.bridge.settingsReady;
  const sent = first.commands("adopt_page_settings");
  assert.equal(sent.length, 1);
  assert.deepEqual(Object.keys(sent[0].args.values).sort(), [
    "backend.screenshotFolder", "dpsMeter.showPing", "dpsMeter.theme", "historyShowTraining", "historyViewMode", "window.x",
  ]);
  assert.equal(first.bridge.getSetting("dpsMeter.theme"), "frost");
  assert.equal(first.bridge.getSetting("dpsMeter.showPing"), "false");
  assert.equal(first.bridge.getSetting("window.x"), null);
  assert.deepEqual([...storage.store.keys()], ["unrelated"]);

  // The next start finds nothing to hand over.
  const second = loadBridge({ settings, storage });
  await second.bridge.settingsReady;
  assert.equal(second.commands("adopt_page_settings").length, 0);
});

test("only the overlay hands the old values over", async () => {
  const storage = pageStorage({ "dpsMeter.showPing": "false" });
  const settingsWindow = loadBridge({ storage, view: "settings" });
  await settingsWindow.bridge.settingsReady;
  assert.equal(settingsWindow.commands("adopt_page_settings").length, 0);
  assert.equal(settingsWindow.bridge.getSetting("dpsMeter.showPing"), null);
  assert.deepEqual(storage.writes, []);
});

test("a failed hand-over keeps the page's copy for the next start and still does not read it", async () => {
  const storage = pageStorage({ "dpsMeter.showPing": "false" });
  const { bridge } = loadBridge({
    settings: { "dpsMeter.theme": "frost" }, storage,
    adopt: () => Promise.reject(new Error("backend busy")),
  });
  await bridge.settingsReady;
  assert.equal(bridge.getSetting("dpsMeter.theme"), "frost");
  assert.equal(bridge.getSetting("dpsMeter.showPing"), null);
  assert.equal(storage.getItem("dpsMeter.showPing"), "false");
  assert.deepEqual(storage.writes, []);
});

test("changes go to settings.json and the window's copy, never to the page's storage", async () => {
  const storage = pageStorage();
  const { bridge, commands, listeners } = loadBridge({ storage });
  await bridge.settingsReady;
  bridge.setSetting("dpsMeter.waylandLayer", true);
  assert.deepEqual(commands("update_settings").map(({ args }) => `${args.key}=${args.value}`), ["dpsMeter.waylandLayer=true"]);
  assert.equal(bridge.getSetting("dpsMeter.waylandLayer"), "true");
  // A change made in another window.
  listeners.get("setting-changed")({ payload: { key: "dpsMeter.roundDps", value: "false" } });
  assert.equal(bridge.getSetting("dpsMeter.roundDps"), "false");
  await bridge.clearAllSettings();
  assert.equal(bridge.getSetting("dpsMeter.roundDps"), null);
  assert.equal(commands("clear_settings").length, 1);
  assert.deepEqual(storage.writes, []);
});

test("the app starts only once the settings are loaded", async () => {
  const steps = [];
  let loaded;
  const window = {
    A2_VIEW: "main", addEventListener: noop, dpsData: {},
    javaBridge: { settingsReady: new Promise((resolve) => { loaded = resolve; }), notifyUiReady: () => steps.push("ready") },
    i18n: { init: async () => steps.push("language") },
  };
  loadScripts(["main.js"], {
    window, console, setTimeout,
    document: { readyState: "complete", addEventListener: noop },
    DpsApp: { createInstance: () => ({ start: () => steps.push("start") }) },
  });
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(steps, [], "nothing reads a setting before settings.json is loaded");
  loaded();
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(steps, ["language", "start", "ready"]);
});

test("the app's setting helpers use the bridge alone", () => {
  const writes = [];
  const window = {
    javaBridge: {
      getSetting: (key) => ({ "dpsMeter.theme": "frost" })[key] ?? null,
      setSetting: (key, value) => writes.push(`${key}=${value}`),
    },
  };
  const localStorage = new Proxy({}, { get() { throw new Error("the page's storage was read"); } });
  const context = loadScripts(["core.js"], { window, console, localStorage });
  const app = vm.runInContext("Object.create(DpsApp.prototype)", context);
  assert.equal(app.safeGetSetting("dpsMeter.theme"), "frost");
  assert.equal(app.safeGetSetting("dpsMeter.waylandLayer"), null);
  app.safeSetSetting("dpsMeter.showPing", "false");
  assert.deepEqual(writes, ["dpsMeter.showPing=false"]);
});

test("no page script uses the page's storage except the one-time hand-over", () => {
  const root = new URL("../public/src/js/", import.meta.url);
  const files = [];
  const walk = (dir, prefix) => {
    for (const name of readdirSync(dir)) {
      const path = new URL(name, dir);
      if (statSync(path).isDirectory()) walk(new URL(`${name}/`, dir), `${prefix}${name}/`);
      else if (name.endsWith(".js")) files.push(`${prefix}${name}`);
    }
  };
  walk(root, "");
  const users = files.filter((file) => /localStorage/.test(readFileSync(new URL(file, root), "utf8")));
  assert.deepEqual(users, ["tauriBridge.js"]);
  const uses = bridgeSource.split("\n").filter((line) => line.includes("localStorage"));
  assert.deepEqual(uses.map((line) => line.trim()), ["storage = window.localStorage;"]);
});
