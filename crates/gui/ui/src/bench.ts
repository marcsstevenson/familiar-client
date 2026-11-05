// Timing script, run when the app starts with FAMILIAR_BENCH_OUT set. It
// measures what the design doc's targets cover and hands the numbers to Rust,
// which writes them to that file and exits.
import { invoke } from "@tauri-apps/api/core";
import { listOpen } from "./list/api";

const frame = () => new Promise<number>((r) => requestAnimationFrame(r));

function stats(xs: number[]) {
  const s = [...xs].sort((a, b) => a - b);
  const at = (q: number) => s[Math.min(s.length - 1, Math.floor(q * s.length))] ?? 0;
  return { n: s.length, median: at(0.5), p95: at(0.95), max: s[s.length - 1] ?? 0 };
}

async function waitFor(f: () => boolean, ms = 10000) {
  const t0 = performance.now();
  while (!f() && performance.now() - t0 < ms) await frame();
}

export async function runBench(): Promise<void> {
  const report: Record<string, unknown> = { started_ms: performance.now() };
  const scroller = () => document.querySelector<HTMLElement>(".list-scroller");
  await waitFor(() => (scroller()?.querySelectorAll(".row-from").length ?? 0) > 0);
  report.first_rows_ms = performance.now();

  // Opening the unified Inbox view (all inboxes), sorted newest first.
  const opens: number[] = [];
  for (let i = 0; i < 5; i++) {
    const t = performance.now();
    await listOpen({ folders: [], unread_only: false, sort: "newest" });
    opens.push(performance.now() - t);
  }
  report.list_open_ms = stats(opens);

  // Scroll 40 px a frame for 600 frames and record frame intervals.
  const el = scroller()!;
  const gaps: number[] = [];
  let blank = 0;
  let last = await frame();
  for (let i = 0; i < 600; i++) {
    el.scrollTop += 40;
    const now = await frame();
    gaps.push(now - last);
    last = now;
    if (el.querySelectorAll(".row-placeholder").length > 0) blank++;
  }
  report.scroll_frame_ms = stats(gaps);
  report.frames_with_unloaded_rows = blank;

  // Jump far down the list, as dragging the scroll bar does.
  const jumps: number[] = [];
  for (let i = 1; i <= 10; i++) {
    const t = performance.now();
    el.scrollTop = (el.scrollHeight * i) / 11;
    await frame();
    await waitFor(() => el.querySelectorAll(".row-placeholder").length === 0, 2000);
    jumps.push(performance.now() - t);
  }
  report.jump_to_rows_ms = stats(jumps);
  report.total_rows = el.scrollHeight / 64;
  await invoke("bench_report", { report });
}
