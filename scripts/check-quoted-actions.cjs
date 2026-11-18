// SPDX-License-Identifier: MIT OR Apache-2.0
// Run against the UI's Vite dev server, with playwright installed.
const assert = require("node:assert/strict");
const { chromium } = require("playwright");

const origin = process.argv[2] || "http://localhost:5173";
const quoted = (sender, address, subject) =>
  `<p><b><span>From:</span></b> ${sender} &lt;${address}&gt;<br><b>Sent:</b> Wednesday<br>` +
  `<b>To:</b> Secretary &lt;sec@example.org&gt;; Me &lt;me@example.org&gt;<br>` +
  `<b>Cc:</b> President &lt;pres@example.org&gt;; Secretary &lt;SEC@example.org&gt;<br>` +
  `<b>Subject:</b> ${subject}</p>`;
const html = "<html><body>Outer cover note" + quoted("Shop", "shop@example.org", "RE: Nets") +
  "<p>Newer delivery note</p>" + quoted("Shop", "shop@example.org", "RE: Nets") +
  "<p>Older order details</p></body></html>";

(async () => {
  const browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH });
  try {
    const page = await browser.newPage();
    await page.route(origin + "/test", route => route.fulfill({ contentType: "text/html", body: '<div id="root"></div>' }));
    await page.goto(origin + "/test");
    await page.evaluate(async html => {
      const { mockWindows, mockIPC } = await import("/node_modules/@tauri-apps/api/mocks.js");
      mockWindows("main");
      mockIPC((cmd, args) => {
        if (cmd === "reading_open") return { html, message: { id: 7, subject: "FW: Nets", account: "me",
          from: { name: "Cover", addr: "outer@example.org" }, to: [], cc: [], is_read: true } };
        if (cmd === "open_compose") window.opened = args.spec;
        if (cmd === "ui_info") return { platform: "windows", theme: "light" };
        if (cmd === "compose_info") return { identities: [{ key: "me", email: "me@example.org", aliases: [],
          signature_reply: "My signature" }] };
        if (cmd === "run" && args.name === "draft.create") {
          window.saved = args.input;
          return { ops: [{ kind: "create_draft", item: 99 }] };
        }
      }, { shouldMockEvents: true });
      const { ReadingPane } = await import("/src/reading/ReadingPane.tsx");
      const { setSelectedMessage } = await import("/src/state.ts");
      document.body.append(ReadingPane());
      setSelectedMessage(7);
    }, html);
    const rows = page.frameLocator(".message-frame").locator(".fam-quoted-from");
    await rows.nth(1).waitFor();
    assert.equal(await rows.count(), 2);
    const row = rows.nth(1);
    const bar = row.locator(".fam-quoted-actions");
    const opacity = () => bar.evaluate(el => getComputedStyle(el).opacity);
    assert.equal(await opacity(), "0");
    await row.hover();
    assert.equal(await opacity(), "1");
    for (const [label, action] of [["Reply", "reply"], ["Forward", "forward"], ["Reply all", "replyAll"]]) {
      await row.getByRole("button", { name: label, exact: true }).click();
      assert.deepEqual(await page.evaluate(() => window.opened), { kind: "quoted", index: 1, message: 7, action });
    }
    await page.evaluate(async () => {
      location.hash = "compose=" + encodeURIComponent(JSON.stringify(window.opened));
      await import("/src/main.tsx");
    });
    await page.waitForFunction(() => window.saved);
    const draft = await page.evaluate(() => window.saved);
    assert.equal(draft.account, "me");
    assert.deepEqual(draft.to, ["Shop <shop@example.org>", "Secretary <sec@example.org>"]);
    assert.deepEqual(draft.cc, ["President <pres@example.org>"]);
    assert.equal(draft.subject, "RE: Nets");
    assert.match(draft.body, /My signature[\s\S]*Older order details/);
    assert.doesNotMatch(draft.body, /Outer cover note|Newer delivery note|fam-quoted-actions/);
    console.log("Quoted From-row actions and saved Reply all draft passed");
  } finally {
    await browser.close();
  }
})().catch(e => { console.error(e); process.exitCode = 1; });
