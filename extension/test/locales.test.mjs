// Every message the extension shows exists in both locales.
//   node --test "extension/test/*.test.mjs"   (Node 22+; Node 20: node --test extension/test/)
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";

const src = new URL("../src/", import.meta.url);
const locale = (name) => JSON.parse(readFileSync(new URL(`_locales/${name}/messages.json`, src), "utf8"));

test("both locales have the same keys", () => {
  assert.deepEqual(Object.keys(locale("pt_BR")).sort(), Object.keys(locale("en")).sort());
});

test("every key the code asks for is in the locales", () => {
  const known = locale("en");
  const files = [
    ...readdirSync(src).filter((name) => /\.(js|html)$/.test(name)).map((name) => new URL(name, src)),
    new URL("../manifest.chrome.json", src),
    new URL("../manifest.firefox.json", src),
  ];
  const used = new Set();
  for (const file of files) {
    const text = readFileSync(file, "utf8");
    for (const m of text.matchAll(/\b(?:message|t|getMessage)\(\s*"(\w+)"/g)) used.add(m[1]);
    for (const m of text.matchAll(/data-i18n(?:-placeholder)?="(\w+)"/g)) used.add(m[1]);
    for (const m of text.matchAll(/__MSG_(\w+)__/g)) used.add(m[1]);
  }
  assert.ok(used.size > 40, "the keys were found");
  assert.deepEqual([...used].filter((key) => !(key in known)), []);
});
