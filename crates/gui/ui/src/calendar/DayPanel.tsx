import { For, Match, Show, Switch, batch, createEffect, createMemo, createResource, createSignal, onCleanup, onMount } from "solid-js";
import { readSession, writeSession } from "../session";
import type { CalEvent } from "../../../../../bindings/commands";
import { isCommandError, run } from "../api";
import { onStoreChanged } from "../list/api";
import { notify } from "../state";
import { EventDetails } from "./EventDetails";
import { NewEvent } from "./NewEvent";
import { AgendaView, MonthView, TimeView } from "./CalendarViews";
import { MiniMonth } from "./MiniMonth";
import { dateWords, type Calendar } from "./proposals";
import { calendarRange, datesFrom, localDate, nextDate, shiftMonth, views, type CalendarView } from "./timeline";
import "./full-calendar.css";
import "./day-panel.css";

function preferences() {
  try {
    const saved = JSON.parse(localStorage.getItem("familiar.calendar-view") ?? "{}");
    return { view: views.includes(saved.view) ? saved.view as CalendarView : "Week" as const,
      hidden: Array.isArray(saved.hidden) ? saved.hidden.filter(Number.isSafeInteger) as number[] : [] };
  } catch { return { view: "Week" as const, hidden: [] }; }
}
function errorText(error: unknown) {
  // Solid resources wrap rejected command objects in an Error.
  const cause = error instanceof Error ? error.cause : error;
  return isCommandError(cause) ? cause.message : String(error);
}

export function DayPanel(props: { full: boolean; expand: () => void; onClose: () => void }) {
  const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  const [now, setNow] = createSignal(Date.now());
  const location = readSession<{ date: string; event?: number }>("calendar");
  const [date, setDate] = createSignal(location?.date ?? localDate(new Date()));
  const [catalogue, { refetch: refreshCalendars }] = createResource(() => run("cal.calendars", {}));
  const saved = preferences();
  const [fullView, setFullView] = createSignal<CalendarView>(saved.view);
  const view = () => props.full ? fullView() : "Day";
  const period = createMemo(() => calendarRange(view(), date()), undefined, {
    equals: (a, b) => a?.from === b.from && a.days === b.days,
  });
  const dates = createMemo(() => datesFrom(period().from, period().days));
  const [range, { refetch: refreshDay }] = createResource(period, async query => ({
    query, ...await run("cal.list", { ...query, tz: zone }),
  }));
  const calendars = () => catalogue.error ? [] : catalogue()?.calendars ?? [];
  const result = () => range.error || range()?.query !== period() ? undefined : range();
  const [hidden, setHidden] = createSignal<number[]>(saved.hidden);
  function remember() {
    try { localStorage.setItem("familiar.calendar-view", JSON.stringify({ view: fullView(), hidden: hidden() })); }
    catch { /* Keep the current choices in memory. */ }
  }
  function chooseView(v: CalendarView) { setFullView(v); remember(); }
  const [slot, setSlot] = createSignal<{ date: string; endDate?: string; time: string; zone: string }>();
  const [selected, select] = createSignal<number | undefined>(location?.event);
  createEffect(() => writeSession("calendar", { date: date(), event: selected() }));
  const writable = createMemo(() => calendars().filter(c => c.can_edit));
  const visible = createMemo(() => (result()?.events ?? [])
    .filter(e => !e.cancelled && !hidden().includes(e.calendar_id)));
  const [revision, setRevision] = createSignal(0);
  const color = (id: number) => {
    const value = calendars().find(c => c.id === id)?.color ?? "";
    const fallback = ["blue", "green", "purple", "orange", "teal", "pink"];
    return CSS.supports("color", value) ? value : "var(--folder-" + fallback[id % fallback.length] + ")";
  };
  let heading!: HTMLHeadingElement;
  onMount(() => {
    heading.focus();
    const timer = setInterval(() => setNow(Date.now()), 60_000);
    const stop = onStoreChanged(() => { void refreshDay(); void refreshCalendars(); if (selected() !== undefined) setRevision(n => n + 1); });
    onCleanup(() => { clearInterval(timer); void stop.then(f => f()); });
  });

  function navigate(day: string) {
    if (!day) return;
    select(undefined);
    setDate(day);
  }
  function step(n: number) {
    navigate(view() === "Month" ? shiftMonth(date(), n) : nextDate(date(), n * period().days));
  }
  function openDay(d: string) {
    batch(() => { navigate(d); chooseView("Day"); });
  }
  function newAt(at: number) {
    const local = new Date(at);
    const date = localDate(local);
    const time = local.toTimeString().slice(0, 5);
    // A wall time alone cannot select the later copy of a repeated hour.
    // UTC also keeps the default one-hour duration across a clock change.
    if (new Date(date + "T" + time).getTime() !== at ||
        local.getTimezoneOffset() !== new Date(at + 3600_000).getTimezoneOffset()) {
      const stamp = local.toISOString();
      setSlot({ date: stamp.slice(0, 10), time: stamp.slice(11, 16), zone: "UTC" });
    } else setSlot({ date, time, zone });
  }
  function card(e: CalEvent, prefix?: string) {
    return <button class="day-event" style={{ "border-left-color": color(e.calendar_id) }}
      aria-expanded={selected() === e.id} title={e.subject + " · " + e.account + " · " + e.calendar}
      onClick={() => select(e.id)}>
      <strong>{prefix ? prefix + " · " : ""}{e.subject || "(No subject)"}</strong><small>{e.account} · {e.calendar}</small>
    </button>;
  }
  function checkbox(c: Calendar) {
    return <label><input type="checkbox" checked={!hidden().includes(c.id)}
      onChange={e => {
        const checked = e.currentTarget.checked;
        setHidden(ids => checked ? ids.filter(id => id !== c.id) : [...ids, c.id]);
        remember();
      }} /><i style={{ background: color(c.id) }} />{!props.full && c.account + " · "}{c.name}{c.can_edit ? "" : " (read-only)"}</label>;
  }
  const rangeTitle = () => view() === "Month" ? new Date(date() + "T12:00").toLocaleDateString(undefined, { month: "long", year: "numeric" })
    : view() === "Day" ? dateWords(date()) : dateWords(period().from) + " – " + dateWords(nextDate(period().from, period().days - 1));
  const viewProps = { get dates() { return dates(); }, get events() { return visible(); }, get date() { return date(); }, card, openDay };
  return (
    <aside class="day-panel" classList={{ "full-calendar": props.full }} aria-label={props.full ? "Full calendar" : "Day calendar"} onKeyDown={e => {
      if (e.key === "Escape" && !slot() && selected() === undefined) { e.preventDefault(); props.onClose(); }
    }}>
      <header><h2 ref={heading} tabindex="-1">Calendar</h2>
        <Show when={!props.full}><button onClick={() => { props.expand(); heading.focus(); }}>Open full calendar</button></Show>
        <button aria-label={props.full ? "Back to Mail" : "Close calendar"} onClick={props.onClose}>{props.full ? "Back to Mail" : "×"}</button>
      </header>
      <Show when={props.full}><div class="calendar-sidebar">
        <MiniMonth date={date()} navigate={navigate} />
        <h3>Calendars</h3><For each={[...new Set(calendars().map(c => c.account))]}>{account =>
          <fieldset><legend>{account}</legend><For each={calendars().filter(c => c.account === account)}>{checkbox}</For></fieldset>
        }</For>
      </div></Show>
      <div class="calendar-content">
        <div class="day-nav">
          <button onClick={() => navigate(localDate(new Date()))}>Today</button>
          <button aria-label={"Previous " + view().toLowerCase()} onClick={() => step(-1)}>‹</button>
          <input type="date" aria-label="Calendar date" value={date()} onChange={e => navigate(e.currentTarget.value)} />
          <button aria-label={"Next " + view().toLowerCase()} onClick={() => step(1)}>›</button>
          <Show when={props.full}><strong class="calendar-range">{rangeTitle()}</strong>
            <select aria-label="Calendar view" value={view()} onChange={e => chooseView(e.currentTarget.value as CalendarView)}>
              <For each={views}>{v => <option value={v} selected={view() === v}>{v}</option>}</For>
            </select></Show>
        </div>
        <div class="day-tools">
          <Show when={!props.full}><details><summary>Calendars</summary>
            <For each={calendars()}>{checkbox}</For>
          </details></Show>
          <button class="day-create" disabled={!writable().length} onClick={() => setSlot({ date: date(), time: "09:00", zone })}>+ New event</button>
        </div>
        <Show when={catalogue.error || range.error}>{e => <p class="day-warning" role="alert">{errorText(e())}</p>}</Show>
        <Show when={!catalogue.loading && !catalogue.error && !calendars().length}><p>No calendars have synced yet.</p></Show>
        <For each={calendars().filter(c => !hidden().includes(c.id) && c.error)}>{c =>
          <p class="day-warning">{c.account} · {c.name} — {c.error}</p>}</For>
        <For each={result()?.warnings}>{w => <p class="day-warning">{w}</p>}</For>
        <Show when={range.loading && !result()}><p role="status">Loading schedule…</p></Show>
        <Switch>
          <Match when={view() === "Month"}><MonthView {...viewProps} writable={!!writable().length}
            create={(date, endDate, time) => setSlot({ date, endDate, time, zone })} /></Match>
          <Match when={view() === "Agenda"}><Show when={result()}><AgendaView {...viewProps} /></Show></Match>
          <Match when={true}><TimeView {...viewProps} now={now()} writable={!!writable().length} create={newAt} /></Match>
        </Switch>
        <footer>{zone.replaceAll("_", " ")}</footer>
      </div>
      <Show when={selected()}>{id => <EventDetails id={id()} revision={revision()} expand={props.full ? undefined : props.expand}
        onClose={() => { select(undefined); heading.focus(); }} onChanged={() => void refreshDay()} />}</Show>
      <Show when={slot()}>{t => <NewEvent date={t().date} endDate={t().endDate} time={t().time} zone={t().zone} calendars={writable()} onClose={() => setSlot(undefined)}
        onSaved={(c, day, time, savedZone) => {
          navigate(savedZone === "UTC" && time ? localDate(new Date(day + "T" + time + "Z")) : day);
          setHidden(ids => ids.filter(id => id !== c.id));
          remember();
          setSlot(undefined);
          void refreshDay();
          notify("Added to " + c.account + " · " + c.name);
        }} />}</Show>
    </aside>
  );
}
