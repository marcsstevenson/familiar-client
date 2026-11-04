// WP 0.4 Tauri prototype: three panes, a virtualised list of 500k synthetic
// rows fetched from Rust a page at a time, and a reading pane showing a
// sanitised HTML newsletter in a sandboxed iframe with a strict CSP.
//
// Bench mode is driven from Rust (FAMILIAR_BENCH_OUT). The page scrolls,
// moves the selection and opens messages by itself and reports timings.
import { invoke } from "@tauri-apps/api/core";
import { For, createSignal, onMount } from "solid-js";
import { render } from "solid-js/web";
import "./style.css";

type Row = [from: string, subject: string, preview: string, date: string, flags: number];

const ROW_H = 56;
const PAGE = 100;
const OVERSCAN = 6;

const pages = new Map<number, Row[]>();
const inflight = new Set<number>();
const [pageRev, setPageRev] = createSignal(0);

async function loadPage(p: number): Promise<void> {
  if (pages.has(p) || inflight.has(p) || p < 0) return;
  inflight.add(p);
  const rows = await invoke<Row[]>("rows", { start: p * PAGE, count: PAGE });
  pages.set(p, rows);
  inflight.delete(p);
  setPageRev((n) => n + 1);
}

function rowAt(i: number): Row | undefined {
  return pages.get(Math.floor(i / PAGE))?.[i % PAGE];
}

/** Resolves once the frame showing the latest DOM change has been painted. */
function afterPaint(): Promise<number> {
  return new Promise((resolve) =>
    requestAnimationFrame(() => {
      const ch = new MessageChannel();
      ch.port1.onmessage = () => resolve(performance.timeOrigin + performance.now());
      ch.port2.postMessage(0);
    }),
  );
}

const ms = (a: number, b: number) => b - a;
const bench = (event: string, extra: Record<string, unknown> = {}) =>
  void invoke("bench_event", { event, extra });

const FOLDERS: string[] = [];

function App() {
  let listEl!: HTMLDivElement;
  let frameEl!: HTMLIFrameElement;
  const [total, setTotal] = createSignal(0);
  const [scrollTop, setScrollTop] = createSignal(0);
  const [viewH, setViewH] = createSignal(800);
  const [selected, setSelected] = createSignal(0);
  const [folders, setFolders] = createSignal<string[]>(FOLDERS);
  let blankFrames = 0;
  let openSeq = 0;

  const range = () => {
    pageRev();
    const first = Math.max(0, Math.floor(scrollTop() / ROW_H) - OVERSCAN);
    const last = Math.min(total(), Math.ceil((scrollTop() + viewH()) / ROW_H) + OVERSCAN);
    const out: number[] = [];
    for (let i = first; i < last; i++) out.push(i);
    return out;
  };

  const prefetch = () => {
    const p0 = Math.floor(scrollTop() / ROW_H / PAGE);
    for (let p = p0 - 1; p <= p0 + 2; p++) void loadPage(p);
  };

  /** Opens message `ix`; resolves with the time its HTML frame has painted. */
  async function open(ix: number): Promise<{ painted: number; sanitizeMs: number }> {
    const seq = ++openSeq;
    const [html, sanitizeMs] = await invoke<[string, number]>("message_html", { ix });
    if (seq !== openSeq) return { painted: 0, sanitizeMs };
    const loaded = new Promise<void>((r) => frameEl.addEventListener("load", () => r(), { once: true }));
    frameEl.srcdoc = html;
    await loaded;
    return { painted: await afterPaint(), sanitizeMs };
  }

  function select(ix: number) {
    ix = Math.max(0, Math.min(total() - 1, ix));
    setSelected(ix);
    const top = ix * ROW_H;
    if (top < listEl.scrollTop) listEl.scrollTop = top;
    else if (top + ROW_H > listEl.scrollTop + listEl.clientHeight)
      listEl.scrollTop = top + ROW_H - listEl.clientHeight;
  }

  async function timedSelect(ix: number, kind: string, start: number) {
    select(ix);
    const highlighted = afterPaint();
    const opened = open(ix);
    if (kind === "open") {
      const o = await opened;
      if (o.painted) bench("open", { latency_ms: ms(start, o.painted), sanitize_ms: o.sanitizeMs });
    } else {
      bench(kind, { latency_ms: ms(start, await highlighted) });
    }
  }

  async function runBench() {
    await new Promise((r) => setTimeout(r, 1500));
    // 1. Scroll: 40 px per frame for 600 frames.
    const frames: number[] = [];
    await new Promise<void>((done) => {
      let n = 0;
      const step = (t: number) => {
        frames.push(t);
        if (!range().every((i) => rowAt(i))) blankFrames++;
        if (n++ >= 600) return done();
        listEl.scrollTop += 40;
        requestAnimationFrame(step);
      };
      requestAnimationFrame(step);
    });
    const d = frames.slice(1).map((t, i) => t - frames[i]!);
    bench("scroll", { frame_ms: d, blank_frames: blankFrames });
    // 2. Self-driven selection moves and message opens, 30 of each.
    for (let i = 0; i < 30; i++) {
      await new Promise((r) => setTimeout(r, 120));
      const kind = i % 2 === 0 ? "self_key" : "open";
      const next = kind === "open" ? 1000 + i * 7919 : selected() + 1;
      await timedSelect(next, kind, performance.timeOrigin + performance.now());
    }
    await new Promise((r) => setTimeout(r, 500));
    bench("bench_done");
  }

  onMount(async () => {
    const info = await invoke<{ total: number; folders: string[]; bench: boolean }>("info");
    setTotal(info.total);
    setFolders(info.folders);
    setViewH(listEl.clientHeight);
    new ResizeObserver(() => setViewH(listEl.clientHeight)).observe(listEl);
    await loadPage(0);
    await open(0);
    await afterPaint();
    bench("first_frame");
    window.addEventListener("keydown", (e) => {
      const start = performance.timeOrigin + e.timeStamp;
      if (e.key === "j" || e.key === "ArrowDown") void timedSelect(selected() + 1, "key", start);
      else if (e.key === "k" || e.key === "ArrowUp") void timedSelect(selected() - 1, "key", start);
      else return;
      e.preventDefault();
    });
    if (info.bench) void runBench();
  });

  return (
    <div class="app">
      <nav class="folders">
        <For each={folders()}>{(f, i) => <div class={i() === 0 ? "folder sel" : "folder"}>{f}</div>}</For>
      </nav>
      <div
        class="list"
        ref={listEl}
        onScroll={() => {
          setScrollTop(listEl.scrollTop);
          prefetch();
        }}
      >
        <div style={{ height: `${total() * ROW_H}px`, position: "relative" }}>
          <For each={range()}>
            {(i) => {
              const r = () => rowAt(i);
              return (
                <div
                  class={i === selected() ? "row sel" : "row"}
                  style={{ transform: `translateY(${i * ROW_H}px)` }}
                  onClick={() => void timedSelect(i, "click", performance.timeOrigin + performance.now())}
                >
                  <div class="top">
                    <span class={(r()?.[4] ?? 0) & 1 ? "from unread" : "from"}>{r()?.[0] ?? ""}</span>
                    <span class="date">{r()?.[3] ?? ""}</span>
                  </div>
                  <div class="subject">{r()?.[1] ?? ""}</div>
                  <div class="preview">{r()?.[2] ?? ""}</div>
                </div>
              );
            }}
          </For>
        </div>
      </div>
      <main class="reading">
        {/* No allow-scripts, no allow-same-origin: the message cannot run code or reach the app. */}
        <iframe ref={frameEl} sandbox="" referrerpolicy="no-referrer" title="Message" />
      </main>
    </div>
  );
}

render(() => <App />, document.getElementById("root")!);
