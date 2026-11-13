// The PDF frame for Linux, where WebKitGTK has no PDF viewer of its own.
//
// It runs pdf.js inside pdf-frame.html, which the viewer loads in an
// <iframe sandbox="allow-scripts">. With no allow-same-origin the frame has an
// opaque origin, so even a PDF that broke out of pdf.js could not reach
// Familiar's commands or the app window. This pdf.js has no eval paths and
// runs without its scripting support; the file arrives by postMessage only.
//
// Built on its own and inlined into dist/pdf-frame.html (vite.pdf.config.ts),
// whose own CSP lets only that script run and blocks every fetch, so the
// frame can't call Familiar's IPC either.
import { GlobalWorkerOptions, getDocument, type PDFDocumentProxy } from "pdfjs-dist/legacy/build/pdf.mjs";
import * as worker from "pdfjs-dist/legacy/build/pdf.worker.mjs";

// No Worker: a sandboxed frame can't start one from our URL, so pdf.js runs
// its worker code on this thread instead ("fake worker").
(globalThis as { pdfjsWorker?: unknown }).pdfjsWorker = worker;
GlobalWorkerOptions.workerSrc = "";

const pages = document.getElementById("pages")!;
const zoomLabel = document.getElementById("zoom")!;
const countLabel = document.getElementById("count")!;
const ZOOMS = [0.5, 0.67, 0.75, 0.9, 1, 1.25, 1.5, 2, 3];

let doc: PDFDocumentProxy | null = null;
let zoom: number | null = null; // null = fit width
let renderId = 0;

function fail(text: string) {
  pages.replaceChildren(Object.assign(document.createElement("p"), { className: "msg", textContent: text }));
}

async function render() {
  if (!doc) return;
  const id = ++renderId;
  const d = doc;
  const width = pages.clientWidth - 32;
  const dpr = window.devicePixelRatio || 1;
  const first = await d.getPage(1);
  const base = first.getViewport({ scale: 1 });
  const scale = zoom ?? Math.max(0.25, width / base.width);
  zoomLabel.textContent = `${Math.round(scale * 100)}%`;
  const frag = document.createDocumentFragment();
  const canvases: [number, HTMLCanvasElement][] = [];
  for (let n = 1; n <= d.numPages; n++) {
    const c = document.createElement("canvas");
    c.className = "page";
    frag.appendChild(c);
    canvases.push([n, c]);
  }
  pages.replaceChildren(frag);
  for (const [n, c] of canvases) {
    if (id !== renderId) return;
    const page = n === 1 ? first : await d.getPage(n);
    const vp = page.getViewport({ scale: scale * dpr });
    c.width = Math.floor(vp.width);
    c.height = Math.floor(vp.height);
    c.style.width = `${Math.floor(vp.width / dpr)}px`;
    c.style.height = `${Math.floor(vp.height / dpr)}px`;
    await page.render({ canvas: c, viewport: vp }).promise;
  }
}

function step(dir: 1 | -1) {
  const cur = zoom ?? Number.parseInt(zoomLabel.textContent ?? "100", 10) / 100;
  const next = dir > 0 ? ZOOMS.find((z) => z > cur + 0.001) : [...ZOOMS].reverse().find((z) => z < cur - 0.001);
  if (next !== undefined) {
    zoom = next;
    void render();
  }
}

document.getElementById("out")!.onclick = () => step(-1);
document.getElementById("in")!.onclick = () => step(1);
document.getElementById("fit")!.onclick = () => {
  zoom = null;
  void render();
};
let resize: number | undefined;
window.addEventListener("resize", () => {
  if (zoom !== null) return;
  clearTimeout(resize);
  resize = window.setTimeout(() => void render(), 150);
});

window.addEventListener("message", async (e: MessageEvent) => {
  if (e.source !== window.parent || !(e.data instanceof ArrayBuffer)) return;
  try {
    doc = await getDocument({
      data: new Uint8Array(e.data),
      enableXfa: false,
      useSystemFonts: true,
      disableFontFace: false,
    }).promise;
    
    await render();
  } catch (err) {
    fail(`Couldn't show this PDF: ${err instanceof Error ? err.message : String(err)}`);
  }
});

window.parent.postMessage("pdf-frame-ready", "*");
