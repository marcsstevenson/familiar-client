// SPDX-License-Identifier: MIT OR Apache-2.0
// Run against the UI's Vite dev server, with playwright installed.
const assert = require("node:assert/strict");
const { chromium } = require("playwright");
const origin = process.argv[2] || "http://localhost:5173";

(async () => {
  const browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH });
  try {
    const context = await browser.newContext();
    await context.route(origin + "/test", route => route.fulfill({ contentType: "text/html", body: '<div id="root"></div>' }));
    const main = await context.newPage();
    const editor = await context.newPage();
    for (const page of [main, editor]) {
      await page.goto(origin + "/test");
      await page.evaluate(async () => {
        window.session = await import("/src/session.ts");
        session.initSession("test store", false);
        await session.connectEditors();
      });
    }

    // Hold the writer busy so that the close request in the other window
    // definitely arrives before the final working copy can commit.
    await editor.evaluate(async () => {
      const { rememberEditor } = await import("/src/compose/recovery.ts");
      const store = session.editorStore("readwrite");
      const until = performance.now() + 300;
      function hold() {
        if (performance.now() < until) store.count().onsuccess = hold;
      }
      hold();
      void rememberEditor("draft", { subject: "Latest edit" });
    });
    await main.evaluate(() => session.flushEditorWrites());
    await editor.close();
    assert.equal(await main.evaluate(async () =>
      (await (await import("/src/compose/recovery.ts")).readEditor("draft")).subject), "Latest edit");

    // Restore into a shorter provisional document, then into the full one.
    await main.evaluate(async () => {
      const { mockIPC, mockWindows } = await import("/node_modules/@tauri-apps/api/mocks.js");
      mockWindows("reader-1");
      const images = new Promise(resolve => { window.finishDownload = resolve; });
      mockIPC(async (cmd, args) => {
        if (cmd === "ui_info") return { session_store: "test store", theme: "light", platform: "windows", spam_accounts: [] };
        if (cmd === "reading_open") {
          if (!args.local) await images;
          return {
            message: { id: 1, subject: "Reading position", account: "home", from: { addr: "test@example.org" }, to: [], cc: [], is_read: true },
            html: '<!doctype html><html><head></head><body style="height:' + (args.local ? 1200 : 3000) + 'px">' +
              (args.local ? "Provisional" : "Complete") + '</body></html>',
            inline_pending: args.local,
          };
        }
      }, { shouldMockEvents: true });
      session.writeSession("readerScroll/1", { id: 1, top: 1500 });
      location.hash = "#reader=1";
      await import("/src/main.tsx");
    });
    await main.frameLocator("iframe").getByText("Provisional").waitFor();
    const offset = () => main.frameLocator("iframe").locator("body").evaluate(el => el.ownerDocument.scrollingElement.scrollTop);
    const clamped = await offset();
    assert.ok(clamped > 0 && clamped < 1500);
    await main.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    assert.equal(await main.evaluate(() => session.readSession("readerScroll/1").top), 1500);
    await main.evaluate(() => finishDownload());
    await main.frameLocator("iframe").getByText("Complete").waitFor();
    assert.equal(await offset(), 1500);
    console.log("Editor flush and two-stage reading position restore passed.");
  } finally {
    await browser.close();
  }
})().catch(e => { console.error(e); process.exitCode = 1; });
