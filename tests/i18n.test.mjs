import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../public/src/js/i18n.js", import.meta.url), "utf8");

// i18n.js over the built resources in dist/.
const builtI18n = () => {
  const window = {};
  const document = {
    baseURI: "http://localhost/",
    documentElement: { setAttribute() {} },
    querySelectorAll: () => [],
  };
  class MissingResource {
    open() {}
    send() { this.onerror(); }
  }
  const fetch = async (url) => {
    try {
      const file = readFileSync(new URL(`../dist${new URL(url).pathname}`, import.meta.url));
      return { ok: true, arrayBuffer: async () => Uint8Array.from(file).buffer };
    } catch {
      return { ok: false, status: 404 };
    }
  };
  vm.runInNewContext(source, { window, document, fetch, URL, TextDecoder, Uint8Array, XMLHttpRequest: MissingResource });
  return window;
};

test("built UI resources support language switching, dungeons included", async () => {
  const window = builtI18n();
  const ru = JSON.parse(readFileSync(new URL("../src/data/i18n/ui/ru.json", import.meta.url)));
  const en = JSON.parse(readFileSync(new URL("../src/data/i18n/ui/en.json", import.meta.url)));
  await window.i18n.setLanguage("ru", { persist: false });
  assert.equal(window.i18n.t("target.all"), ru.target.all);
  assert.match(window.i18n.getDungeonLabel(600001), /^Пещера крао/);
  // An id the game no longer has keeps its English name.
  assert.equal(window.i18n.getDungeonLabel(600161), "Consumed Deus Research Base");
  await window.i18n.setLanguage("en", { persist: false });
  assert.equal(window.i18n.t("target.all"), en.target.all);
  assert.match(window.i18n.getDungeonLabel(600001), /^Krao Cave/);
});

test("a dungeon's difficulty is the game's own, and only where the game gives one", async () => {
  const window = builtI18n();
  await window.i18n.setLanguage("en", { persist: false });
  const { getDungeonLabel, getDungeonDifficulty } = window.i18n;
  // The Expedition menu's two tabs: Exploration, and Conquest with its stars.
  assert.equal(getDungeonLabel(600002), "Krao Cave (Exploration)");
  assert.equal(getDungeonLabel(600004), "Krao Cave (Conquest Tier 1)");
  assert.equal(getDungeonDifficulty(600004).key, "tier");
  assert.equal(getDungeonLabel(600022), "Fire Temple (Conquest Tier 3)");
  assert.equal(getDungeonLabel(600123), "Cradle of Nihility (Conquest Tier 4 · Hard)");
  assert.equal(getDungeonDifficulty(600123).key, "advanced");
  // A Transcendence run, not "Level 3" from the id's last digit.
  assert.equal(getDungeonLabel(600053), "Deus Research Base (Transcendence)");
  assert.equal(getDungeonLabel(620021), "Chalice of Muspel (Hard)");
  assert.equal(getDungeonLabel(690035), "Orcus's Grave (Insane)");
  // Sealed and quest dungeons, the Abyss, maps without a dungeon row and
  // names that hold their variant: a name only.
  for (const [id, name] of [[310051, "Altar of Hope"], [210009, "Zumion Relic Storage"],
    [142007, "Corrupted Forester Ruins Treasure Storage"], [21, "Chaotic Lower Reshanta"],
    [600144, "Citadel of the Fallen Daeva"], [910011, "Nightmare Altar (Easy)"]]) {
    assert.equal(getDungeonLabel(id), name, String(id));
    assert.equal(getDungeonDifficulty(id), null, String(id));
  }
  await window.i18n.setLanguage("de", { persist: false });
  assert.equal(getDungeonLabel(600002), "Kraohöhle (Erkundung)");
  await window.i18n.setLanguage("fr", { persist: false });
  assert.equal(getDungeonLabel(600004), "Grotte de Krao (Conquête de rang 1)");
});

test("no dungeon label repeats a word, or a variant its name holds", () => {
  const dir = new URL("../src/data/i18n/dungeons/", import.meta.url);
  for (const file of readdirSync(dir)) {
    const table = JSON.parse(readFileSync(new URL(file, dir), "utf8"));
    for (const [id, entry] of Object.entries(table)) {
      if (!entry.label) continue;
      const words = entry.label.toLowerCase().split(/[\s·()[\]]+/).filter(Boolean);
      assert.equal(new Set(words).size, words.length, `${file} ${id}: ${entry.label}`);
      assert.doesNotMatch(entry.name, /[(\[（【].*[)\]）】]/, `${file} ${id}: ${entry.name}`);
      // Short Latin words ("de") say nothing twice.
      const meaningful = (text) => (text.toLowerCase().match(/[\p{L}\p{N}_]+/gu) || [])
        .filter((w) => w.length > 2 || /[^\x00-\x7f]/.test(w));
      const named = new Set(meaningful(entry.name));
      assert.deepEqual(meaningful(entry.label).filter((w) => named.has(w)), [], `${file} ${id}: ${entry.name} / ${entry.label}`);
    }
  }
});

const uiDir = new URL("../src/data/i18n/ui/", import.meta.url);
const flatten = (obj, prefix = "") =>
  Object.entries(obj).flatMap(([key, value]) =>
    value && typeof value === "object" ? flatten(value, `${prefix}${key}.`) : [[`${prefix}${key}`, value]]);
const placeholders = (text) => [...String(text).matchAll(/\{\w+\}/g)].map((m) => m[0]).sort();

test("every language file has every English string, with the same placeholders", () => {
  const en = new Map(flatten(JSON.parse(readFileSync(new URL("en.json", uiDir)))));
  for (const lang of ["de", "es", "fr", "ja", "ko", "pt", "ru", "zh-Hans", "zh-Hant"]) {
    const strings = new Map(flatten(JSON.parse(readFileSync(new URL(`${lang}.json`, uiDir)))));
    const missing = [...en.keys()].filter((key) => !strings.has(key));
    assert.deepEqual(missing, [], `${lang}.json is missing strings`);
    for (const [key, text] of en) {
      assert.deepEqual(placeholders(strings.get(key)), placeholders(text), `${lang}.json ${key}`);
    }
  }
});

// Ids that look like keys but are never shown as text (stat slots in details.js).
const NOT_TEXT = new Set(["details.stats.empty", "details.stats.partyHeal"]);
const pageFiles = (dir) =>
  readdirSync(dir, { withFileTypes: true }).flatMap((entry) =>
    entry.isDirectory() ? pageFiles(new URL(`${entry.name}/`, dir))
      : entry.name.endsWith(".js") ? [new URL(entry.name, dir)] : []);

test("every key the page uses is in en.json", () => {
  const en = JSON.parse(readFileSync(new URL("en.json", uiDir)));
  const keys = new Set(flatten(en).map(([key]) => key));
  const files = [new URL("../index.html", import.meta.url), ...pageFiles(new URL("../public/src/js/", import.meta.url))];
  const missing = new Set();
  for (const file of files) {
    const text = readFileSync(file, "utf8");
    for (const [, key] of text.matchAll(/["'`]([a-z][A-Za-z0-9]*(?:\.[A-Za-z0-9_]+)+)["'`]/g)) {
      if (!(key.split(".")[0] in en) || /\.(json|js|css|html|png|svg|txt|dat)$/.test(key)) continue;
      const section = key.split(".").reduce((node, part) => node?.[part], en);
      if (!keys.has(key) && !(section && typeof section === "object") && !NOT_TEXT.has(key)) missing.add(key);
    }
  }
  assert.deepEqual([...missing].sort(), [], "keys used by the page but missing from en.json");
});

test("a language picked in one window reaches the other windows", async () => {
  const handlers = [];
  const bus = { listen: (name, fn) => { handlers.push([name, fn]); }, emit: (name, payload) => handlers.filter(([n]) => n === name).forEach(([, fn]) => fn({ payload })) };
  const fetch = async (url) => {
    try {
      const file = readFileSync(new URL(`../dist${new URL(url).pathname}`, import.meta.url));
      return { ok: true, arrayBuffer: async () => Uint8Array.from(file).buffer };
    } catch {
      return { ok: false, status: 404 };
    }
  };
  const open = () => {
    const window = { __TAURI__: { event: bus } };
    const document = { baseURI: "http://localhost/", documentElement: { setAttribute() {} }, querySelectorAll: () => [] };
    vm.runInNewContext(source, { window, document, fetch, URL, TextDecoder, Uint8Array });
    return window.i18n;
  };
  const settings = open();
  const history = open();
  await settings.init();
  await history.init();
  const changed = new Promise((resolve) => history.onChange(resolve));
  await settings.setLanguage("ko");
  assert.equal(await changed, "ko");
  const ko = JSON.parse(readFileSync(new URL("../src/data/i18n/ui/ko.json", import.meta.url)));
  assert.equal(history.t("target.all"), ko.target.all);
});

test("a slow language load does not undo a newer choice", async () => {
  const window = {};
  const document = { baseURI: "http://localhost/", documentElement: { setAttribute() {} }, querySelectorAll: () => [] };
  let releaseSlow;
  const slow = new Promise((resolve) => { releaseSlow = resolve; });
  const fetch = async (url) => {
    const path = new URL(url).pathname;
    if (path.endsWith("/de.json")) await slow;
    try {
      const file = readFileSync(new URL(`../dist${path}`, import.meta.url));
      return { ok: true, arrayBuffer: async () => Uint8Array.from(file).buffer };
    } catch {
      return { ok: false, status: 404 };
    }
  };
  vm.runInNewContext(source, { window, document, fetch, URL, TextDecoder, Uint8Array });
  const first = window.i18n.setLanguage("de", { persist: false });
  await window.i18n.setLanguage("ko", { persist: false });
  releaseSlow();
  await first;
  const ko = JSON.parse(readFileSync(new URL("../src/data/i18n/ui/ko.json", import.meta.url)));
  assert.equal(window.i18n.t("target.all"), ko.target.all);
});
