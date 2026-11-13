// SPDX-License-Identifier: MIT OR Apache-2.0
// Renders the reading pane's HTML test set in Chromium (the engine behind
// WebView2 on Windows) inside the same sandboxed frame and CSP the app uses,
// and fails if a script runs or a request leaves the page.
//
//   FAMILIAR_HTML_OUT=target/reading-html cargo test -p familiar-gui html_test_set
//   node scripts/check-reading-html.cjs target/reading-html
const fs = require("fs");
const path = require("path");
const { chromium } = require("playwright");

const APP_CSP =
  "default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: famimg: http://famimg.localhost; frame-src 'self' blob:";

(async () => {
  const dir = process.argv[2];
  if (!dir) throw new Error("usage: check-reading-html.cjs <dir of sanitised documents>");
  const files = fs.readdirSync(dir).filter((f) => f.endsWith(".html")).sort();
  if (files.length === 0) throw new Error(`no documents in ${dir}`);
  const browser = await chromium.launch();
  const page = await browser.newPage();
  const requests = [];
  page.on("request", (r) => {
    const u = r.url();
    if (!u.startsWith("data:") && !u.startsWith("about:")) requests.push(u);
  });
  // A request the CSP refused never left the page: that is the backstop working.
  const refused = new Set();
  page.on("requestfailed", (r) => {
    if ((r.failure() || {}).errorText === "csp") refused.add(r.url());
  });
  page.on("dialog", (d) => {
    requests.push(`dialog: ${d.message()}`);
    d.dismiss();
  });
  await page.setContent(
    `<!doctype html><meta http-equiv="Content-Security-Policy" content="${APP_CSP}"><body></body>`,
  );
  let failed = 0;
  for (const f of files) {
    requests.length = 0;
    refused.clear();
    const doc = fs.readFileSync(path.join(dir, f), "utf8");
    const result = await page.evaluate(async (doc) => {
      window.pwned = undefined;
      document.body.innerHTML = "";
      const frame = document.createElement("iframe");
      frame.setAttribute("sandbox", "allow-same-origin");
      frame.setAttribute("referrerpolicy", "no-referrer");
      const loaded = new Promise((r) => frame.addEventListener("load", r, { once: true }));
      frame.srcdoc = doc;
      document.body.appendChild(frame);
      await loaded;
      // Poke everything a person might touch.
      for (const el of frame.contentDocument.querySelectorAll("*")) {
        for (const t of ["mouseover", "click", "focus", "toggle"]) {
          el.dispatchEvent(new Event(t, { bubbles: true }));
        }
      }
      await new Promise((r) => setTimeout(r, 150));
      return {
        pwned: window.pwned,
        text: frame.contentDocument.body ? frame.contentDocument.body.innerText.length : 0,
      };
    }, doc);
    const shown = f.endsWith(".shown.html");
    const leaks = requests.filter(
      (u) => !refused.has(u) && !(shown && (u.startsWith("famimg:") || u.startsWith("http://famimg.localhost/"))),
    );
    const problems = [];
    if (result.pwned !== undefined) problems.push(`script ran (pwned=${result.pwned})`);
    if (leaks.length) problems.push(`requests: ${leaks.join(", ")}`);
    if (result.text === 0) problems.push("no text rendered");
    console.log(`${problems.length ? "FAIL" : "ok  "} ${f}${problems.length ? ": " + problems.join("; ") : ""}`);
    failed += problems.length ? 1 : 0;
  }
  await browser.close();
  if (failed) {
    console.error(`${failed} of ${files.length} documents failed`);
    process.exit(1);
  }
  console.log(`all ${files.length} documents rendered safely`);
})().catch((e) => {
  console.error(e);
  process.exit(1);
});
