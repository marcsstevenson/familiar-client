import { For, Match, Show, Switch, batch, createEffect, createMemo, createResource, createSignal, onCleanup, onMount } from "solid-js";
import type { CalEvent } from "../../../../../bindings/commands";
import { isCommandError, run } from "../api";
import { onStoreChanged } from "../list/api";
import { notify } from "../state";
import { EventDetails } from "./EventDetails";
import type { MenuPoint } from "./EventMenu";
import { NewEvent } from "./NewEvent";
import { AgendaView, MonthView, TimeView } from "./CalendarViews";
import { MiniMonth } from "./MiniMonth";
import { dateWords, type Calendar } from "./proposals";
import { calendarView, calendarVisible, chooseView, setCalendarVisible, writableCalendars } from "./preferences";
import { calendarRange, datesFrom, localDate, nextDate, shiftMonth, views, type CalendarView } from "./timeline";
import "./full-calendar.css";
import "./day-panel.css";

function errorText(error: unknown) {
  // Solid resources wrap rejected command objects in an Error.
  const cause = error instanceof Error ? error.cause : error;
  return isCommandError(cause) ? cause.message : String(error);
}

export type CalendarLocation = { date: string; event?: number };

export function DayPanel(props: { full: boolean; location?: CalendarLocation; remember: (location: CalendarLocation) => void;
  expand: () => void; onClose: () => void;
  target?: { calendar: number; event: number; date: string }; onTargetOpened: () => void }) {
  const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  const [now, setNow] = createSignal(Date.now());
  const location = props.location;
  const [date, setDate] = createSignal(location?.date ?? localDate(new Date()));
  const [catalogue, { refetch: refreshCalendars }] = createResource(() => run("cal.calendars", {}));
  const [configuring, setConfiguring] = createSignal(false);
  const view = () => props.full ? calendarView() : "Day";
  const period = createMemo(() => calendarRange(view(), date()), undefined, {
    equals: (a, b) => a?.from === b.from && a.days === b.days,
  });
  const dates = createMemo(() => datesFrom(period().from, period().days));
  const [range, { refetch: refreshDay }] = createResource(period, async query => ({
    query, ...await run("cal.list", { ...query, tz: zone }),
  }));
  const calendars = () => catalogue.error ? [] : catalogue()?.calendars ?? [];
  const result = () => range.error || range()?.query !== period() ? undefined : range();
  const [slot, setSlot] = createSignal<{ date: string; endDate?: string; time: string; zone: string }>();
  const [selected, select] = createSignal<number | undefined>(location?.event);
  const [menu, setMenu] = createSignal<MenuPoint>();
  let trigger: HTMLButtonElement | undefined;
  createEffect(() => {
    const target = props.target;
    if (!target) return;
    setSlot(undefined);
    setMenu(undefined);
    trigger = undefined;
    setDate(target.date);
    select(target.event);
    props.onTargetOpened();
  });
  createEffect(() => props.remember({ date: date(), event: menu() ? undefined : selected() }));
  const enabled = createMemo(() => calendars().filter(c => calendarVisible(c)));
  const sidebarCalendars = () => configuring() ? calendars() : enabled();
  const writable = createMemo(() => writableCalendars(calendars()));
  const hiddenIds = createMemo(() => new Set(calendars().filter(c => !calendarVisible(c)).map(c => c.id)));
  const visible = createMemo(() => (result()?.events ?? [])
    .filter(e => !e.cancelled && !hiddenIds().has(e.calendar_id)));
  const [revision, setRevision] = createSignal(0);
  const color = (id: number) => {
    const value = calendars().find(c => c.id === id)?.color ?? "";
    const fallback = ["blue", "green", "purple", "orange", "teal", "pink"];
    return CSS.supports("color", value) ? value : "var(--folder-" + fallback[id % fallback.length] + ")";
  };
  let heading!: HTMLHeadingElement;
  onMount(() => {
    if (!document.activeElement?.closest('[role="tablist"]')) heading.focus();
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
      aria-haspopup="menu" onContextMenu={ev => {
        ev.preventDefault();
        trigger = ev.currentTarget;
        const rect = trigger.getBoundingClientRect();
        const point = ev.clientX || ev.clientY ? { x: ev.clientX, y: ev.clientY } : { x: rect.left, y: rect.bottom };
        batch(() => { setMenu(point); select(e.id); });
      }} onClick={ev => { trigger = ev.currentTarget; select(e.id); }}>
      <strong>{prefix ? prefix + " · " : ""}{e.subject || "(No subject)"}</strong><small>{e.account} · {e.calendar}</small>
    </button>;
  }
  function checkbox(c: Calendar) {
    return <label><input type="checkbox" checked={calendarVisible(c)}
      onChange={e => setCalendarVisible(c, e.currentTarget.checked)} />
      <i style={{ background: color(c.id) }} /><span>{!props.full && c.account + " · "}{c.name}{c.can_edit ? "" : " (read-only)"}</span></label>;
  }
  const rangeTitle = () => view() === "Month" ? new Date(date() + "T12:00").toLocaleDateString(undefined, { month: "long", year: "numeric" })
    : view() === "Day" ? dateWords(date()) : dateWords(period().from) + " – " + dateWords(nextDate(period().from, period().days - 1));
  const viewProps = { get dates() { return dates(); }, get events() { return visible(); }, get date() { return date(); }, card, openDay };
  return (
    <aside class="day-panel" classList={{ "full-calendar": props.full }} tabindex="-1" aria-label={props.full ? "Full calendar" : "Day calendar"} onKeyDown={e => {
      if (slot() || selected() !== undefined || e.defaultPrevented) return;
      if (e.key === "Escape") { e.preventDefault(); props.onClose(); return; }
      if ((e.key !== "ArrowLeft" && e.key !== "ArrowRight") || e.altKey || e.ctrlKey || e.metaKey || e.shiftKey ||
          (e.target instanceof Element && e.target.closest("input, select, textarea, [contenteditable=true]"))) return;
      e.preventDefault();
      // Event cards and date cells can disappear when the range changes.
      heading.focus({ preventScroll: true });
      step(e.key === "ArrowLeft" ? -1 : 1);
    }}>
      <header><h2 ref={heading} tabindex="-1">{props.full ? "Calendar" : "Day panel"}</h2>
        <Show when={!props.full}><button onClick={props.expand}>Open full calendar</button>
          <button aria-label="Close day panel" onClick={props.onClose}>×</button></Show>
      </header>
      <Show when={props.full}><div class="calendar-sidebar">
        <MiniMonth date={date()} navigate={navigate} />
        <div class="calendar-key-heading"><h3>Calendars</h3>
          <button type="button" aria-label={configuring() ? "Done configuring calendars" : "Configure calendars"}
            onClick={() => setConfiguring(!configuring())}>{configuring() ? "Done" : "Configure"}</button>
        </div>
        <Show when={!configuring() && calendars().length > 0 && !enabled().length}><p>No calendars selected.</p></Show>
        <For each={[...new Set(sidebarCalendars().map(c => c.account))]}>{account =>
          <fieldset><legend>{account}</legend><div class="calendar-key-items">
            <For each={sidebarCalendars().filter(c => c.account === account)}>{c =>
              <Show when={configuring()} fallback={
                <span class="calendar-pill" style={{ "--calendar-color": color(c.id) }}><i />{c.name}</span>
              }>{checkbox(c)}</Show>
            }</For>
          </div></fieldset>
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
        <For each={enabled().filter(c => c.error)}>{c =>
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
      <Show when={selected()} keyed>{id => <EventDetails id={id} revision={revision()} expand={props.full ? undefined : props.expand}
        menu={menu()} onOpenDetails={() => setMenu(undefined)}
        onClose={() => {
          batch(() => { select(undefined); setMenu(undefined); });
          (trigger?.isConnected ? trigger : heading).focus({ preventScroll: true });
          trigger = undefined;
        }} onChanged={() => void refreshDay()} />}</Show>
      <Show when={slot()}>{t => <NewEvent date={t().date} endDate={t().endDate} time={t().time} zone={t().zone} calendars={writable()} onClose={() => setSlot(undefined)}
        onSaved={(c, day, time, savedZone) => {
          navigate(savedZone === "UTC" && time ? localDate(new Date(day + "T" + time + "Z")) : day);
          setSlot(undefined);
          void refreshDay();
          notify("Added to " + c.account + " · " + c.name);
        }} />}</Show>
    </aside>
  );
}
