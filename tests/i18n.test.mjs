import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../public/src/js/i18n.js", import.meta.url), "utf8");

test("built UI resources support language switching and English dungeon fallback", async () => {
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
  const ru = JSON.parse(readFileSync(new URL("../src/data/i18n/ui/ru.json", import.meta.url)));
  const en = JSON.parse(readFileSync(new URL("../src/data/i18n/ui/en.json", import.meta.url)));
  await window.i18n.setLanguage("ru", { persist: false });
  assert.equal(window.i18n.t("target.all"), ru.target.all);
  assert.match(window.i18n.getDungeonLabel(600001), /^Krao Cave/);
  await window.i18n.setLanguage("en", { persist: false });
  assert.equal(window.i18n.t("target.all"), en.target.all);
  assert.match(window.i18n.getDungeonLabel(600001), /^Krao Cave/);
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
const NOT_TEXT = new Set(["details.stats.empty", "details.stats.partyHeal", "details.stats.damageReceived"]);
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
