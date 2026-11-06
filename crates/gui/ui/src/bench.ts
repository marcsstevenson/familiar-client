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
  report.cold_start_ms = await invoke<number>("bench_elapsed_ms");

  // Opening the unified Inbox view (all inboxes), sorted newest first.
  const opens: number[] = [];
  for (let i = 0; i < 5; i++) {
    const t = performance.now();
    await listOpen({ folders: [], unread_only: false, sort: "newest", query: null });
    opens.push(performance.now() - t);
  }
  report.list_open_ms = stats(opens);

  // Searches, as the search box runs them (all matches up to the cap, then
  // the first page of rows).
  const searches: Record<string, unknown> = {};
  for (const q of ["from:ana", "subject:invoice", "\"team meeting\"", "is:unread", "has:attachment after:2025-01-01"]) {
    const ts: number[] = [];
    for (let i = 0; i < 5; i++) {
      const t = performance.now();
      await listOpen({ folders: [], unread_only: false, sort: "newest", query: q });
      await invoke("list_rows", { start: 0, count: 100 });
      ts.push(performance.now() - t);
    }
    searches[q] = stats(ts);
  }
  report.search_first_page_ms = searches;
  await listOpen({ folders: [], unread_only: false, sort: "newest", query: null });

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

  // Keypresses: select the top row, then press j 30 times. Each is timed to
  // the frame showing the new selection, then to the frame showing its header
  // in the reading pane.
  el.scrollTop = 0;
  await frame();
  await waitFor(() => el.querySelector("#msg-0 .row-from") !== null, 2000);
  el.querySelector("#msg-0")!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
  const subjectOf = (id: string | null) =>
    (id && document.getElementById(id)?.querySelector(".row-subject")?.textContent) || "(no subject)";
  const shown = () => document.querySelector(".reading-subject")?.textContent;
  await waitFor(() => shown() === subjectOf("msg-0"), 2000);
  const keys: number[] = [];
  const headers: number[] = [];
  for (let i = 1; i <= 30; i++) {
    const t = performance.now();
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "j", bubbles: true }));
    await waitFor(() => el.getAttribute("aria-activedescendant") === `msg-${i}`, 2000);
    await frame();
    keys.push(performance.now() - t);
    await waitFor(() => shown() === subjectOf(`msg-${i}`), 2000);
    await frame();
    headers.push(performance.now() - t);
  }
  report.key_to_selection_ms = stats(keys);
  report.key_to_header_ms = stats(headers);

  // Accessibility: axe's WCAG 2.1 A and AA rules over the app's own page (not
  // the message frame, which runs no scripts).
  const axe = (await import("axe-core")).default;
  const result = await axe.run(
    { exclude: [[".message-frame"]] },
    { runOnly: ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"], iframes: false },
  );
  report.a11y_violations = result.violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => ({ id: v.id, impact: v.impact, help: v.help, nodes: v.nodes.slice(0, 5).map((n) => n.target.join(" ")) }));
  await invoke("bench_report", { report });
}
