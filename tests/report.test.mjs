import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { loadScripts } from "./scripts.mjs";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");

function report() {
  return loadScripts(["settings/report.js"], { DpsApp: class {}, window: {} });
}

test("each issue form opens with the report info in its report-info field", () => {
  const context = report();
  const forms = vm.runInContext("REPORT_FORMS", context);
  const url = (kind, info) => vm.runInContext(`reportFormUrl(${JSON.stringify(kind)}, ${JSON.stringify(info)})`, context);
  const info = "Daevalog 1.0 · r400\nSystem: Fedora Linux 40";
  for (const [kind, file] of Object.entries(forms)) {
    const link = new URL(url(kind, info));
    assert.equal(link.origin + link.pathname, "https://github.com/Seralth/Daevalog/issues/new");
    assert.equal(link.searchParams.get("template"), file);
    assert.equal(link.searchParams.get("report-info"), info);
    const form = read(`../.github/ISSUE_TEMPLATE/${file}`);
    assert.match(form, /^\s+id: report-info$/m, `${file} has a report-info field`);
  }
  assert.equal(url("security", info), "https://github.com/Seralth/Daevalog/security/advisories/new");
  assert.equal(url("nope", info), null);
});

test("the security contact link and the chooser go to the same private form", () => {
  const config = read("../.github/ISSUE_TEMPLATE/config.yml");
  assert.match(config, /^blank_issues_enabled: false$/m);
  assert.match(config, /url: https:\/\/github\.com\/Seralth\/Daevalog\/security\/advisories\/new$/m);
});
