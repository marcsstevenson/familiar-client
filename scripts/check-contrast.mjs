// SPDX-License-Identifier: MIT OR Apache-2.0
// Checks the desktop app's colour tokens against WCAG 2.1 AA: 4.5:1 for text,
// 3:1 for focus rings and other non-text marks, in the light and dark sets.
// Usage: node scripts/check-contrast.mjs [path/to/tokens.css]
import { readFileSync } from "node:fs";

const file = process.argv[2] ?? "crates/gui/ui/src/styles/tokens.css";
const css = readFileSync(file, "utf8");

function block(selector) {
  const i = css.indexOf(selector + " {");
  if (i < 0) throw new Error(`${file}: no block for ${selector}`);
  const body = css.slice(i, css.indexOf("}", i));
  return Object.fromEntries([...body.matchAll(/--([\w-]+):\s*(#[0-9a-fA-F]{6})/g)].map((m) => [m[1], m[2]]));
}

function luminance(hex) {
  const c = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);
  const [r, g, b] = c.map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function ratio(a, b) {
  const [x, y] = [luminance(a), luminance(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
}

const surfaces = ["bg", "bg-sidebar", "bg-chrome", "bg-hover", "bg-selected", "bg-selected-inactive"];
const pairs = [
  ...["text", "text-muted"].flatMap((t) => surfaces.map((s) => [t, s, 4.5])),
  ...["accent", "warn", "danger", "success"].flatMap((t) => ["bg", "bg-sidebar", "bg-chrome"].map((s) => [t, s, 4.5])),
  ["text-on-accent", "accent", 4.5],
  ...["focus"].flatMap((t) => surfaces.map((s) => [t, s, 3])),
];

let failed = 0;
for (const [name, selector] of [
  ["light", ':root,\n:root[data-theme="light"]'],
  ["dark", ':root[data-theme="dark"]'],
]) {
  const t = block(selector);
  for (const [fg, bg, min] of pairs) {
    if (!t[fg] || !t[bg]) {
      console.log(`${name}: --${fg} or --${bg} missing`);
      failed++;
      continue;
    }
    const r = ratio(t[fg], t[bg]);
    if (r < min) {
      console.log(`${name}: --${fg} on --${bg} is ${r.toFixed(2)}:1, needs ${min}:1`);
      failed++;
    }
  }
}
if (failed) {
  console.log(`${failed} colour pair(s) fail WCAG AA`);
  process.exit(1);
}
console.log(`colour contrast: ${pairs.length * 2} pairs pass WCAG AA`);
