// SPDX-License-Identifier: MIT OR Apache-2.0
// Run against the UI's Vite dev server with Playwright installed.
const assert = require("node:assert/strict");
const { chromium } = require("playwright");
const origin = process.argv[2] || "http://localhost:5173";

async function mount(page, main) {
  await page.route(origin + "/test", route => route.fulfill({
    contentType: "text/html", body: '<html lang="en"><title>Familiar</title><main class="reading"></main></html>',
  }));
  await page.goto(origin + "/test");
  await page.evaluate(async main => {
    const { mockWindows, mockIPC } = await import("/node_modules/@tauri-apps/api/mocks.js");
    mockWindows("main");
    mockIPC((cmd, args) => {
      if (cmd === "ui_info") {
        const { theme = "light", ...desktop } = JSON.parse(localStorage.getItem("mock.config") ?? "{}");
        return { theme, desktop };
      }
      if (cmd === "save_preferences") {
        const config = JSON.parse(localStorage.getItem("mock.config") ?? "{}");
        for (const [key, value] of Object.entries(args.changes)) {
          if (args.import && key in config) continue;
          if (value === null) delete config[key];
          else config[key] = value;
        }
        localStorage.setItem("mock.config", JSON.stringify(config));
      }
      if (cmd === "set_accent_icon") window.iconPixels = Array.from(new Uint8Array(args));
      if (cmd === "reading_attachments") return [{ kind: "file", id: "1", name: "Notes.txt", size: 120 }];
      if (cmd === "reading_preview") return { kind: "text", text: "Attachment preview" };
      if (cmd === "reading_save") {
        if (window.holdSave) return new Promise((resolve, reject) => {
          window.finishSave = resolve;
          window.failSave = reject;
        });
        return "Downloads/Notes.txt";
      }
    });
    const { TitleBar } = await import("/src/TitleBar.tsx");
    const { SearchBar } = await import("/src/list/SearchBar.tsx");
    const { Attachments } = await import("/src/reading/Attachments.tsx");
    await import("/src/reading/reading.css");
    await import("/src/compose/compose.css");
    await import("/src/styles/tokens.css");
    await import("/src/styles/app.css");
    await (await import("/src/preferences.ts")).initPreferences();
    (await import("/src/theme.ts")).initTheme();
    (await import("/src/icon.ts")).followWindowIcon();
    document.body.prepend(TitleBar({ platform: "windows" }), SearchBar());
    const { render } = await import("/node_modules/solid-js/web/dist/web.js");
    render(() => Attachments({ message: 1, onStatus: s => window.status = s }), document.querySelector("main"));
  }, main);
}

(async () => {
  const browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH });
  try {
    const context = await browser.newContext();
    const main = await context.newPage();
    await mount(main, true);
    const secondary = await context.newPage();
    await mount(secondary, false);
    const search = main.getByRole("searchbox");
    assert.equal(await search.evaluate(el => getComputedStyle(el).paddingInlineStart), "12px");
    const file = main.getByTitle("View Notes.txt");
    const save = main.getByRole("button", { name: "Save Notes.txt" });
    const border = el => getComputedStyle(el).borderTopWidth;
    assert.equal(await file.locator("..").evaluate(border), "1px");
    assert.equal(await file.evaluate(border), "0px");
    assert.equal(await save.evaluate(border), "0px");
    await file.click();
    await main.getByText("Attachment preview", { exact: true }).waitFor();
    const viewer = main.getByRole("dialog", { name: "Viewing Notes.txt" });
    const viewerSave = viewer.getByRole("button", { name: "Save", exact: true });
    await main.evaluate(() => { window.holdSave = true; });
    await viewerSave.click();
    assert.ok(await viewerSave.isDisabled(), "Save should be disabled during the write");
    await main.evaluate(() => window.failSave("Disk full"));
    await viewer.getByRole("status").filter({ hasText: "Disk full" }).waitFor();
    await viewerSave.click();
    await main.evaluate(() => window.finishSave("Downloads/Notes (2).txt"));
    await viewer.getByRole("status").filter({ hasText: "Downloads/Notes (2).txt" }).waitFor();
    await main.evaluate(() => { window.holdSave = false; });
    await main.getByRole("button", { name: "Close viewer" }).click();
    await save.click();
    await main.waitForFunction(() => window.status.includes("Downloads/Notes.txt"));

    const migrated = await main.evaluate(async () => {
      localStorage.setItem("familiar.accent", "#007fff");
      localStorage.setItem("familiar.panes", JSON.stringify({ folders: 210, list: 390 }));
      localStorage.setItem("mock.config", JSON.stringify({ theme: "light", accent: "#b69cff" }));
      await (await import("/src/importPreferences.ts")).importPreferences();
      return [JSON.parse(localStorage.getItem("mock.config")), localStorage.getItem("familiar.accent")];
    });
    assert.deepEqual(migrated, [{ theme: "light", accent: "#b69cff", panes: { folders: 210, list: 390 } }, null]);

    const appearance = main.getByRole("button", { name: "Appearance" });
    await appearance.click();
    const dialog = main.getByRole("dialog", { name: "Appearance" });
    const picker = dialog.getByLabel("Accent colour");
    const hex = dialog.getByRole("textbox", { name: "Hex colour" });
    const sky = dialog.getByRole("button", { name: "Sky", exact: true });
    const presets = dialog.getByRole("group", { name: "Accent presets" });
    assert.equal(await presets.getByRole("button").count(), 24);
    assert.ok((await presets.boundingBox()).y < (await picker.boundingBox()).y);
    assert.equal(await dialog.getByRole("button", { name: "Reset accent" }).count(), 0);
    const done = dialog.getByRole("button", { name: "Done" });
    const neon = dialog.getByRole("switch", { name: "Neon" });
    const kea = main.locator(".titlebar-icon");
    const filter = el => getComputedStyle(el).filter;
    assert.equal(await neon.isChecked(), false);
    assert.equal(await kea.evaluate(filter), "none");
    await presets.getByRole("button", { name: "Hot pink", exact: true }).click();
    assert.equal(await hex.inputValue(), "#FF69B4");
    await neon.focus();
    await main.keyboard.press("Space");
    assert.ok(await neon.isChecked());
    assert.ok((await kea.evaluate(filter)).includes("drop-shadow"));
    const pinkShadow = await done.evaluate(el => getComputedStyle(el).boxShadow);
    assert.notEqual(pinkShadow, "none");
    await secondary.waitForFunction(() => document.documentElement.hasAttribute("data-neon"));
    await main.emulateMedia({ forcedColors: "active" });
    assert.equal(await kea.evaluate(filter), "none");
    await main.emulateMedia({ forcedColors: "none" });
    await hex.fill("#99D9");
    assert.ok(await done.isDisabled());
    await hex.fill("99D9EA");
    assert.equal(await picker.inputValue(), "#99d9ea");
    assert.notEqual(await done.evaluate(el => getComputedStyle(el).boxShadow), pinkShadow);
    assert.equal(await sky.getAttribute("aria-pressed"), "true");
    assert.ok(await done.isEnabled());
    await Promise.all([main, secondary].map(page => page.waitForFunction(() => {
      const rgba = window.iconPixels;
      return rgba?.length === 64 * 64 * 4 && rgba.some((r, i) =>
        i % 4 === 0 && r === 0x99 && rgba[i + 1] === 0xd9 && rgba[i + 2] === 0xea && rgba[i + 3] === 255);
    })));
    assert.ok(decodeURIComponent(await main.locator(".titlebar-icon").getAttribute("src")).includes("#99d9ea"));
    await dialog.getByRole("button", { name: "Violet", exact: true }).click();
    assert.equal(await hex.inputValue(), "#8A5CF6");
    assert.equal(await sky.getAttribute("aria-pressed"), "false");
    for (const theme of ["light", "dark"]) {
      await dialog.getByLabel("Theme", { exact: true }).selectOption(theme);
      await secondary.waitForFunction(t => document.documentElement.dataset.theme === t, theme);
      await picker.fill("#99d9ea");
      for (const page of [main, secondary]) {
        const mark = page.locator(".titlebar-icon");
        await page.waitForFunction(() => decodeURIComponent(document.querySelector(".titlebar-icon").src).includes("#99d9ea"));
        const feather = await mark.evaluate(async el => {
          await el.decode();
          const canvas = document.createElement("canvas");
          canvas.width = canvas.height = 100;
          const ctx = canvas.getContext("2d");
          ctx.drawImage(el, 0, 0, 100, 100);
          return [...ctx.getImageData(15, 32, 1, 1).data];
        });
        assert.deepEqual(feather, theme === "light" ? [0, 0, 0, 0] : [34, 34, 34, 255]);
      }
      for (const colour of ["#ffff00", "#000000", "#ffffff", "#7000ca"]) {
        await picker.fill(colour);
        await secondary.waitForFunction(c => document.documentElement.style.getPropertyValue("--accent-source") === c, colour);
        const rendered = await main.evaluate(() => document.documentElement.style.getPropertyValue("--accent"));
        await secondary.waitForFunction(c => document.documentElement.style.getPropertyValue("--accent") === c, rendered);
        const passes = await main.evaluate(async () => {
          const relativeLuminance = channels => channels.reduce((sum, n, i) => {
            const c = n / 255;
            return sum + [0.2126, 0.7152, 0.0722][i] * (c <= .04045 ? c / 12.92 : ((c + .055) / 1.055) ** 2.4);
          }, 0);
          const rgb = css => css.match(/\d+/g).map(Number);
          const probe = document.createElement("span");
          document.body.append(probe);
          const luminance = token => {
            probe.style.color = `var(--${token})`;
            return relativeLuminance(rgb(getComputedStyle(probe).color));
          };
          const accent = luminance("accent");
          const results = ["bg", "bg-hover", "bg-selected", "bg-chrome", "bg-sidebar", "text-on-accent"].map(token => {
            const surface = luminance(token);
            return (Math.max(accent, surface) + .05) / (Math.min(accent, surface) + .05);
          });
          probe.remove();
          return results;
        });
        assert.ok(passes.every(r => r >= 4.5), `${theme} ${colour}: ${passes}`);
      }
    }
    await main.keyboard.press("Escape");
    assert.equal(await dialog.isVisible(), false);
    assert.ok(await appearance.evaluate(el => el === document.activeElement));
    await mount(main, true);
    await appearance.click();
    assert.ok(await neon.isChecked());
    await neon.uncheck();
    await secondary.waitForFunction(() => !document.documentElement.hasAttribute("data-neon"));
    assert.equal(await kea.evaluate(filter), "none");
    assert.equal(await picker.inputValue(), "#7000ca");
    assert.equal(await hex.inputValue(), "#7000CA");
    await presets.getByRole("button", { name: "Familiar blue" }).click();
    await secondary.waitForFunction(() => !document.documentElement.style.getPropertyValue("--accent"));
    await dialog.getByRole("button", { name: "Done" }).click();
    console.log("Appearance, field padding, attachment actions and cross-window accent checks passed");
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
