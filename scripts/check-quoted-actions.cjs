// SPDX-License-Identifier: MIT OR Apache-2.0
// Run against the UI's Vite dev server, with playwright installed.
const assert = require("node:assert/strict");
const { chromium } = require("playwright");

const origin = process.argv[2] || "http://localhost:5173";
const quoted = (sender, address, subject) =>
  `<p><b><span>From:</span></b> ${sender} &lt;${address}&gt;<br><b>Sent:</b> Wednesday<br>` +
  `<b>To:</b> Secretary &lt;sec@example.org&gt;; Me &lt;me@example.org&gt;<br>` +
  `<b>Cc:</b> President, Club &lt;pres@example.org&gt;; Secretary &lt;SEC@example.org&gt;<br>` +
  `<b>Subject:</b> ${subject}</p>`;
const html = "<html><head></head><body>Outer cover note" + quoted("Shop", "shop@example.org", "RE: Nets") +
  "<p>Newer delivery note</p>" + quoted("Shop, Orders", "shop@example.org", "RE: Nets") +
  "<p>Older order details</p><div>----- Forwarded message -----</div>" +
  "<div><b>From:</b> &quot;Teacher&quot; &lt;teacher@example.org&gt;</div>" +
  "<div><b>To:</b> &quot;Parent&quot; &lt;parent@example.org&gt;</div>" +
  "<div><b>Cc:</b> </div><div><b>Sent:</b> Thursday</div>" +
  "<div><b>Subject:</b> Option form</div><p>Form details</p></body></html>";

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
      await import("/src/styles/tokens.css");
      const { setSelectedMessage } = await import("/src/state.ts");
      document.body.append(ReadingPane());
      setSelectedMessage(7);
    }, html);
    const rows = page.frameLocator(".message-frame").locator(".fam-quoted-from");
    const yahoo = rows.nth(2);
    await yahoo.waitFor();
    assert.equal(await rows.count(), 3);
    assert.match(await yahoo.innerText(), /Teacher/);
    for (const [label, action] of [["Reply", "reply"], ["Reply all", "replyAll"], ["Forward", "forward"]]) {
      await yahoo.hover();
      await yahoo.getByRole("button", { name: label, exact: true }).click();
      assert.deepEqual(await page.evaluate(() => window.opened), { kind: "quoted", index: 2, message: 7, action });
    }
    const row = rows.nth(1);
    assert.ok(await row.getByRole("button", { name: "Reply all" }).isEnabled());
    const bar = row.locator(".fam-quoted-actions");
    const opacity = () => bar.evaluate(el => getComputedStyle(el).opacity);
    assert.equal(await opacity(), "0");
    await row.hover();
    assert.equal(await opacity(), "1");
    // Header and quoted buttons live in different documents. Check what
    // Chromium actually draws in each, including the hover colours.
    const appearance = button => button.evaluate(async el => {
      await Promise.all(el.getAnimations().map(animation => animation.finished));
      const svg = el.querySelector("svg");
      const buttonStyle = getComputedStyle(el);
      const iconStyle = getComputedStyle(svg);
      return [el.textContent, svg.outerHTML, buttonStyle.width, buttonStyle.height,
        buttonStyle.color, buttonStyle.backgroundColor, buttonStyle.border, buttonStyle.borderRadius,
        iconStyle.width, iconStyle.height, iconStyle.fill, iconStyle.stroke, iconStyle.strokeWidth];
    });
    for (const theme of ["light", "dark"]) {
      await page.evaluate(async theme => (await import("/src/theme.ts")).setTheme(theme), theme);
      await page.frameLocator(".message-frame").locator(`html[data-theme="${theme}"]`).waitFor();
      for (const name of ["Reply", "Reply all", "Forward"]) {
        const header = page.getByRole("toolbar").getByRole("button", { name, exact: true });
        const quote = row.getByRole("button", { name, exact: true });
        await row.hover();
        assert.deepEqual(await appearance(quote), await appearance(header));
        await header.hover();
        const hovered = await appearance(header);
        await row.hover();
        await quote.hover();
        assert.deepEqual(await appearance(quote), hovered);
      }
    }
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
    assert.deepEqual(draft.to, ['"Shop, Orders" <shop@example.org>', "Secretary <sec@example.org>"]);
    assert.deepEqual(draft.cc, ['"President, Club" <pres@example.org>']);
    assert.equal(draft.subject, "RE: Nets");
    assert.match(draft.body, /My signature[\s\S]*Older order details/);
    assert.doesNotMatch(draft.body, /Outer cover note|Newer delivery note|fam-quoted-actions/);
    console.log("Quoted From-row actions and saved Reply all draft passed");
  } finally {
    await browser.close();
  }
})().catch(e => { console.error(e); process.exitCode = 1; });
