// SPDX-License-Identifier: MIT OR Apache-2.0
// Fails when a desktop app bench report breaks a limit in perf-budget.json.
// Usage: node scripts/check-bench.mjs <report.json> [crates/gui/perf-budget.json]
import { readFileSync } from "node:fs";

const [reportFile, budgetFile = "crates/gui/perf-budget.json"] = process.argv.slice(2);
if (!reportFile) {
  console.error("usage: check-bench.mjs <report.json> [budget.json]");
  process.exit(2);
}
const report = JSON.parse(readFileSync(reportFile, "utf8"));
const { checks } = JSON.parse(readFileSync(budgetFile, "utf8"));

// Values at a dotted path; `*` matches every key at that level.
function values(obj, parts, at = []) {
  if (parts.length === 0) return [[at.join("."), obj]];
  const [p, ...rest] = parts;
  if (obj === null || typeof obj !== "object") return [[[...at, p].join("."), undefined]];
  const keys = p === "*" ? Object.keys(obj) : [p];
  return keys.flatMap((k) => values(obj[k], rest, [...at, k]));
}

let failed = 0;
for (const c of checks) {
  for (const [path, v] of values(report, c.path.split("."))) {
    const ok = typeof v === "number" && v <= c.limit;
    const note = typeof v === "number" && v > c.target ? ` (target ${c.target})` : "";
    console.log(`${ok ? "ok  " : "FAIL"} ${path} = ${typeof v === "number" ? +v.toFixed(1) : "missing"}, limit ${c.limit}${note}`);
    if (!ok) failed++;
  }
}
for (const v of report.a11y_violations ?? []) console.log(`  a11y ${v.impact}: ${v.id}: ${v.help} at ${v.nodes.join(", ")}`);
if (failed) {
  console.log(`${failed} check(s) over budget`);
  process.exit(1);
}
