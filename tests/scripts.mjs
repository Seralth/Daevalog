// Loads front-end scripts into one vm context, in the order index.html loads them.
import { readFileSync } from "node:fs";
import vm from "node:vm";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const order = [...read("../index.html").matchAll(/<script src="\/src\/js\/([^"?]+)/g)].map((m) => m[1]);
const sources = new Map();

export function loadScripts(files, globals) {
  for (const file of files) {
    if (!order.includes(file)) throw new Error(`${file} is not loaded by index.html`);
  }
  const context = vm.createContext(globals);
  for (const file of order.filter((f) => files.includes(f))) {
    if (!sources.has(file)) sources.set(file, read(`../public/src/js/${file}`));
    vm.runInContext(sources.get(file), context, { filename: file });
  }
  return context;
}
