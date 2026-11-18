// Builds the Linux PDF frame (src/pdfframe) into one self-contained page,
// dist/pdf-frame.html, with pdf.js inlined as a classic script. Runs after the
// main build.
//
// Inlined because the frame is sandboxed without allow-same-origin: its
// opaque origin can't load scripts from the app's own URL in WebKitGTK.
// Tauri adds the inline script's hash to the page's CSP when it embeds it.
import { createHash } from "node:crypto";
import { readFileSync, rmSync, writeFileSync } from "node:fs";
import { defineConfig } from "vite";

export default defineConfig({
  build: {
    target: "es2022",
    outDir: "dist",
    emptyOutDir: false,
    copyPublicDir: false,
    lib: {
      entry: "src/pdfframe/main.ts",
      formats: ["iife"],
      name: "pdfFrame",
      fileName: () => "pdf-frame.js",
    },
  },
  plugins: [
    {
      name: "inline-pdf-frame",
      closeBundle() {
        const js = readFileSync("dist/pdf-frame.js", "utf8");
        if (/<\/script/i.test(js)) throw new Error("pdf-frame.js contains </script>; can't inline it");
        const html = readFileSync("src/pdfframe/frame.html", "utf8");
        // The page's own policy narrows the app's: only this exact script runs,
        // and the frame can't fetch or connect anywhere, Familiar's IPC included.
        const hash = createHash("sha256").update(js).digest("base64");
        const csp =
          `default-src 'none'; script-src 'sha256-${hash}'; style-src 'unsafe-inline'; ` +
          "img-src data: blob:; font-src data: blob:";
        const page = html
          .replace("<!--PDF_FRAME_CSP-->", `<meta http-equiv="Content-Security-Policy" content="${csp}" />`)
          // A function replacement, so `$` in the script isn't read as a pattern.
          .replace("/*PDF_FRAME_SCRIPT*/", () => js);
        writeFileSync("dist/pdf-frame.html", page);
        rmSync("dist/pdf-frame.js");
      },
    },
  ],
});
