#!/usr/bin/env node
// Checks the window's translations (ui/locales/*.js) against the source, pt-BR:
//  - every language has every key, and no key the source lacks;
//  - every translation keeps the source's {placeholders};
//  - every key the page (data-i18n, data-i18n-attr) and app.js (t, say, I18N.text) use exists;
//  - every language listed in i18n.js has a file, loaded by index.html.
// Run: node scripts/check-locales.js (no dependencies). Exits 1 on any problem.
"use strict";

const fs = require("fs");
const path = require("path");

const ui = path.join(__dirname, "..", "ui");
const read = (f) => fs.readFileSync(path.join(ui, f), "utf8");
const problems = [];

const dicts = {};
globalThis.I18N = { add: (code, dict) => (dicts[code] = dict) };
for (const f of fs.readdirSync(path.join(ui, "locales")).filter((f) => f.endsWith(".js"))) {
  new Function(read(path.join("locales", f)))();
  const code = f.slice(0, -3);
  if (!dicts[code]) problems.push(`locales/${f}: does not call I18N.add("${code}", …)`);
}

const source = dicts["pt-BR"];
if (!source) {
  console.error("locales/pt-BR.js is missing");
  process.exit(1);
}
const holes = (s) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort().join(",");

for (const [code, dict] of Object.entries(dicts)) {
  for (const key of Object.keys(source)) {
    if (!(key in dict)) problems.push(`${code}: missing "${key}"`);
    else if (holes(dict[key]) !== holes(source[key])) problems.push(`${code}: "${key}" has {${holes(dict[key])}}, the source has {${holes(source[key])}}`);
    else if (!dict[key].trim()) problems.push(`${code}: "${key}" is empty`);
  }
  for (const key of Object.keys(dict)) if (!(key in source)) problems.push(`${code}: "${key}" is not in pt-BR`);
}

const html = read("index.html");
const app = read("app.js");
const used = new Set();
for (const m of html.matchAll(/data-i18n="([^"]+)"/g)) used.add(m[1]);
for (const m of html.matchAll(/data-i18n-attr="([^"]+)"/g)) for (const pair of m[1].split(";")) used.add(pair.split(":")[1].trim());
for (const m of app.matchAll(/\b(?:t|say|I18N\.text)\((?:[^,()]+, )?"([a-zA-Z0-9.]+)"/g)) used.add(m[1]);
for (const m of app.matchAll(/"([a-z]+\.[a-zA-Z0-9.]+)"/g)) if (m[1].split(".")[0] in { check: 1, run: 1, runtimes: 1, vt: 1 }) used.add(m[1]);
for (const key of used) if (!(key in source)) problems.push(`"${key}" is used but not in pt-BR`);

const listed = [...read("i18n.js").matchAll(/\{ code: "([^"]+)"/g)].map((m) => m[1]);
for (const code of listed) {
  if (!dicts[code]) problems.push(`i18n.js lists "${code}" but locales/${code}.js does not exist`);
  if (!html.includes(`src="locales/${code}.js"`)) problems.push(`index.html does not load locales/${code}.js`);
}
for (const code of Object.keys(dicts)) if (!listed.includes(code)) problems.push(`locales/${code}.js is not listed in i18n.js`);

if (problems.length) {
  for (const p of problems) console.error(p);
  process.exit(1);
}
console.log(`${Object.keys(dicts).length} languages, ${Object.keys(source).length} keys each, ${used.size} used by the window: ok`);
