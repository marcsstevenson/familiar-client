import { For, Show, createMemo, createResource, createSignal, onCleanup, onMount } from "solid-js";
import type { CalEvent } from "../../../../../bindings/commands";
import { isCommandError, run } from "../api";
import { onStoreChanged } from "../list/api";
import { notify } from "../state";
import { EventDetails } from "./EventDetails";
import { NewEvent } from "./NewEvent";
import { localDate, nextDate, timedBlocks } from "./timeline";
import "./day-panel.css";

const HALF_HOUR = 30 * 60_000;
const time = (at: number) => new Date(at).toTimeString().slice(0, 5);
function errorText(error: unknown) {
  // Solid resources wrap rejected command objects in an Error.
  const cause = error instanceof Error ? error.cause : error;
  return isCommandError(cause) ? cause.message : String(error);
}

export function DayPanel(props: { onClose: () => void }) {
  const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  const [now, setNow] = createSignal(Date.now());
  const [date, setDate] = createSignal(localDate(new Date()));
  const [catalogue, { refetch: refreshCalendars }] = createResource(() => run("cal.calendars", {}));
  const [range, { refetch: refreshDay }] = createResource(date, from => run("cal.list", { from, days: 1, tz: zone }));
  const calendars = () => catalogue.error ? [] : catalogue()?.calendars ?? [];
  const result = () => range.error ? undefined : range();
  const [hidden, setHidden] = createSignal<number[]>([]);
  const [slot, setSlot] = createSignal<string>();
  const [selected, select] = createSignal<number>();
  const writable = createMemo(() => calendars().filter(c => c.can_edit));
  const visible = createMemo(() => (result()?.from.slice(0, 10) === date() ? result()?.events ?? [] : [])
    .filter(e => !e.cancelled && !hidden().includes(e.calendar_id)));
  const [revision, setRevision] = createSignal(0);
  const start = () => new Date(date() + "T00:00:00").getTime();
  const end = () => new Date(nextDate(date(), 1) + "T00:00:00").getTime();
  const slots = createMemo(() => Array.from({ length: Math.ceil((end() - start()) / HALF_HOUR) }, (_, n) => start() + n * HALF_HOUR));
  const blocks = createMemo(() => timedBlocks(visible(), start(), end()));
  const percent = (at: number) => 100 * (at - start()) / (end() - start());
  const color = (id: number) => {
    const value = calendars().find(c => c.id === id)?.color ?? "";
    const fallback = ["blue", "green", "purple", "orange", "teal", "pink"];
    return CSS.supports("color", value) ? value : "var(--folder-" + fallback[id % fallback.length] + ")";
  };
  let heading!: HTMLHeadingElement;
  let scroll!: HTMLDivElement;
  onMount(() => {
    heading.focus();
    scroll.scrollTop = (date() === localDate(new Date()) ? Math.max(0, new Date().getHours() - 1) : 8) * 64;
    const timer = setInterval(() => setNow(Date.now()), 60_000);
    const stop = onStoreChanged(() => { void refreshDay(); void refreshCalendars(); if (selected() !== undefined) setRevision(n => n + 1); });
    onCleanup(() => { clearInterval(timer); void stop.then(f => f()); });
  });

  function navigate(day: string) {
    if (!day) return;
    select(undefined);
    setDate(day);
  }
  function card(e: CalEvent) {
    return <button class="day-event" style={{ "border-left-color": color(e.calendar_id) }}
      aria-expanded={selected() === e.id} title={e.subject + " · " + e.account + " · " + e.calendar}
      onClick={() => select(selected() === e.id ? undefined : e.id)}>
      <strong>{e.subject || "(No subject)"}</strong><small>{e.account} · {e.calendar}</small>
    </button>;
  }
  return (
    <aside class="day-panel" aria-label="Day calendar" onKeyDown={e => {
      if (e.key === "Escape" && !slot() && selected() === undefined) { e.preventDefault(); props.onClose(); }
    }}>
      <header><h2 ref={heading} tabindex="-1">Calendar</h2>
        <button aria-label="Close calendar" onClick={props.onClose}>×</button>
      </header>
      <div class="day-nav">
        <button onClick={() => navigate(localDate(new Date()))}>Today</button>
        <button aria-label="Previous day" onClick={() => navigate(nextDate(date(), -1))}>‹</button>
        <input type="date" aria-label="Calendar date" value={date()} onChange={e => navigate(e.currentTarget.value)} />
        <button aria-label="Next day" onClick={() => navigate(nextDate(date(), 1))}>›</button>
      </div>
      <div class="day-tools">
        <details><summary>Calendars</summary>
          <For each={calendars()}>{c => <label><input type="checkbox" checked={!hidden().includes(c.id)}
            onChange={e => setHidden(ids => e.currentTarget.checked ? ids.filter(id => id !== c.id) : [...ids, c.id])} />
            <i style={{ background: color(c.id) }} />{c.account} · {c.name}{c.can_edit ? "" : " (read-only)"}</label>}</For>
        </details>
        <button class="day-create" disabled={!writable().length} onClick={() => setSlot("09:00")}>+ New event</button>
      </div>
      <Show when={catalogue.error || range.error}>{e => <p class="day-warning" role="alert">{errorText(e())}</p>}</Show>
      <Show when={!catalogue.loading && !catalogue.error && !calendars().length}><p>No calendars have synced yet.</p></Show>
      <For each={calendars().filter(c => !hidden().includes(c.id) && c.error)}>{c =>
        <p class="day-warning">{c.account} · {c.name} — {c.error}</p>}</For>
      <For each={result()?.from.slice(0, 10) === date() ? result()?.warnings : []}>{w => <p class="day-warning">{w}</p>}</For>
      <Show when={range.loading && result()?.from.slice(0, 10) !== date()}><p role="status">Loading schedule…</p></Show>
      <div class="day-all"><span>All day</span><div><For each={visible().filter(e => e.all_day)}>{card}</For></div></div>
      <div class="day-scroll" ref={scroll}>
        <div class="day-grid" style={{ height: slots().length * 32 + "px" }}>
          <For each={slots()}>{at => <button class="day-slot" disabled={!writable().length}
            aria-label={"New event at " + time(at)} onClick={() => setSlot(time(at))}>
            {new Date(at).getMinutes() === 0 ? time(at) : ""}
          </button>}</For>
          <div class="day-blocks">
            <For each={blocks()}>{b => <div class="day-block" style={{
              top: percent(b.start) + "%", height: (percent(b.end) - percent(b.start)) + "%",
              left: (100 * b.column / b.columns) + "%", width: (100 / b.columns) + "%",
            }}>{card(b.event)}</div>}</For>
          </div>
          <Show when={now() >= start() && now() < end()}><div class="day-now" style={{ top: percent(now()) + "%" }} /></Show>
        </div>
      </div>
      <footer>{zone.replaceAll("_", " ")}</footer>
      <Show when={selected()}>{id => <EventDetails id={id()} revision={revision()}
        onClose={() => { select(undefined); heading.focus(); }} onChanged={() => void refreshDay()} />}</Show>
      <Show when={slot()}>{t => <NewEvent date={date()} time={t()} zone={zone} calendars={writable()} onClose={() => setSlot(undefined)}
        onSaved={(c, day) => {
          navigate(day);
          setHidden(ids => ids.filter(id => id !== c.id));
          setSlot(undefined);
          void refreshDay();
          notify("Added to " + c.account + " · " + c.name);
        }} />}</Show>
    </aside>
  );
}
