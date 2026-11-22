import { For, Show, createMemo, createSignal, onMount, type JSX } from "solid-js";
import type { CalEvent } from "../../../../../bindings/commands";
import { dateWords } from "./proposals";
import { localDate, nextDate, onDay, timedBlocks } from "./timeline";
import { readSession, writeSession } from "../session";

function calendarScroller(el: HTMLElement, defaultTop = 0) {
  const view = el.className;
  const key = el.closest(".full-calendar") ? "calendarScroll" : "dayPanelScroll";
  const saved = readSession<{ view: string; top: number }>(key);
  el.scrollTop = saved?.view === view ? saved.top : defaultTop;
  el.addEventListener("scroll", () => writeSession(key, { view, top: el.scrollTop }));
}

type ViewProps = {
  dates: string[]; events: CalEvent[]; date: string;
  card: (event: CalEvent, prefix?: string) => JSX.Element;
  openDay: (date: string) => void;
};

const time = (at: number) => new Date(at).toTimeString().slice(0, 5);
const halfHour = 30 * 60_000;
const weekday = (date: string) => new Date(date + "T12:00").toLocaleDateString(undefined, { weekday: "short" });
function slots(date: string) {
  const start = new Date(date + "T00:00").getTime();
  const end = new Date(nextDate(date, 1) + "T00:00").getTime();
  return Array.from({ length: Math.ceil((end - start) / halfHour) }, (_, i) => start + i * halfHour);
}

export function TimeView(props: ViewProps & {
  now: number; writable: boolean; create: (at: number) => void;
}) {
  let scroll!: HTMLDivElement;
  onMount(() => {
    calendarScroller(scroll, (props.dates.includes(localDate(new Date())) ? Math.max(0, new Date().getHours() - 1) : 8) * 64);
  });
  const week = () => props.dates.length > 1;
  // Use the longest day's labels across columns. This leaves a gap for skipped
  // times in spring and room for both occurrences of a repeated hour in autumn.
  const labels = createMemo(() => {
    const longest = props.dates.map(slots).sort((a, b) => b.length - a.length)[0] ?? [];
    return longest.map(time);
  });
  return <div class="time-view" classList={{ "week-view": week() }}>
    <div class="time-head">
      <For each={props.dates}>{date => <div>
        <Show when={week()}><button class="week-date" onClick={() => props.openDay(date)}>{weekday(date)} {Number(date.slice(8))}</button></Show>
        <div class="day-all"><span>All day</span><div><For each={props.events.filter(e => e.all_day && onDay(e, date))}>{e => props.card(e)}</For></div></div>
      </div>}</For>
    </div>
    <div class="day-scroll" ref={scroll}><div class="time-columns">
      <For each={props.dates}>{date => {
        const ticks = slots(date);
        const end = new Date(nextDate(date, 1) + "T00:00").getTime();
        const start = ticks[0]!;
        const rows = createMemo(() => {
          let last = -1;
          return ticks.map(at => { last = labels().indexOf(time(at), last + 1); return last; });
        });
        const position = (at: number) => {
          if (at >= end) return labels().length;
          const index = (at - start) / halfHour;
          return rows()[Math.floor(index)]! + index % 1;
        };
        const blocks = createMemo(() => timedBlocks(props.events, start, end));
        return <div class="day-grid" aria-label={dateWords(date)} style={{ height: labels().length * 32 + "px" }}>
          <For each={ticks}>{(at, i) => <button class="day-slot" style={{ top: rows()[i()]! * 32 + "px" }}
            disabled={!props.writable} aria-label={"New event at " + time(at)} title={new Date(at).toTimeString()} onClick={() => props.create(at)}>
            {new Date(at).getMinutes() === 0 ? time(at) : ""}</button>}</For>
          <div class="day-blocks"><For each={blocks()}>{b => <div class="day-block" style={{
            top: position(b.start) * 32 + "px", height: (position(b.end) - position(b.start)) * 32 + "px",
            left: 100 * b.column / b.columns + "%", width: 100 / b.columns + "%",
          }}>{props.card(b.event)}</div>}</For></div>
          <Show when={props.now >= start && props.now < end}><div class="day-now" style={{ top: position(props.now) * 32 + "px" }} /></Show>
        </div>;
      }}</For>
    </div></div>
  </div>;
}

function timeLabel(e: CalEvent, date: string) {
  return e.all_day ? "All day" : e.start.slice(0, 10) < date ? "Continues" : e.start.slice(11, 16);
}

export function AgendaView(props: ViewProps) {
  let scroll!: HTMLDivElement;
  onMount(() => calendarScroller(scroll));
  const groups = createMemo(() => props.dates.map(date => ({ date, events: props.events.filter(e => onDay(e, date)) }))
    .filter(g => g.events.length));
  return <div class="calendar-agenda" ref={scroll}><For each={groups()} fallback={<p>No events in this range.</p>}>{g =>
    <section><h3>{dateWords(g.date)}</h3><div><For each={g.events}>{e => props.card(e, timeLabel(e, g.date))}</For></div></section>
  }</For></div>;
}

/** Assign each spanning event one row for the week, then fill the remaining
 * cells with timed summaries. Overflow counts belong to the individual day. */
function monthWeek(dates: string[], events: CalEvent[]) {
  const occupied = Array.from({ length: 3 }, () => new Set<number>());
  const more = dates.map(() => 0);
  const entries: { event: CalEvent; first: number; last: number; row: number }[] = [];
  function place(event: CalEvent, first: number, last: number) {
    const cells = Array.from({ length: last - first + 1 }, (_, i) => first + i);
    const row = occupied.findIndex(used => cells.every(i => !used.has(i)));
    if (row < 0) {
      for (const i of cells) more[i] = more[i]! + 1;
      return;
    }
    for (const i of cells) occupied[row]!.add(i);
    entries.push({ event, first, last, row });
  }
  for (const e of events.filter(e => e.all_day)) {
    const first = dates.findIndex(d => onDay(e, d));
    if (first >= 0) place(e, first, dates.reduce((last, d, i) => onDay(e, d) ? i : last, first));
  }
  for (const [i, date] of dates.entries()) {
    for (const e of events) if (!e.all_day && onDay(e, date)) place(e, i, i);
  }
  return { entries, more };
}

export function MonthView(props: ViewProps & {
  writable: boolean; create: (first: string, last: string, time: string) => void;
}) {
  const weeks = createMemo(() => props.dates.filter((_, i) => i % 7 === 0).map((_, i) => props.dates.slice(7 * i, 7 * i + 7)));
  const [drag, setDrag] = createSignal<{ anchor: string; over: string; time: string; pointer: number }>();
  let month!: HTMLDivElement;
  onMount(() => calendarScroller(month));
  function startTime(cell: HTMLElement, y: number) {
    const rect = cell.getBoundingClientRect();
    const fraction = Math.max(0, Math.min(1, (y - rect.top) / rect.height));
    return String(6 + Math.round(12 * fraction)).padStart(2, "0") + ":00";
  }
  function track(e: PointerEvent, finish: boolean) {
    const d = drag();
    if (!d || e.pointerId !== d.pointer) return;
    // Event cards can cover the destination cell, including across a week boundary.
    const cell = document.elementsFromPoint(e.clientX, e.clientY)
      .find(el => el instanceof HTMLElement && el.dataset.monthDate && month.contains(el)) as HTMLElement | undefined;
    const over = cell?.dataset.monthDate;
    if (finish) {
      setDrag(undefined);
      if (props.writable && over && over !== d.anchor) {
        const [first, last] = d.anchor < over ? [d.anchor, over] : [over, d.anchor];
        props.create(first, last, d.time);
      }
    } else if (over && over !== d.over) setDrag({ ...d, over });
  }
  function inRange(date: string) {
    const d = drag();
    return !!d && ((d.anchor <= date && date <= d.over) || (d.over <= date && date <= d.anchor));
  }
  return <div class="month-view" ref={month} tabindex="-1" onPointerMove={e => track(e, false)}
    onPointerUp={e => track(e, true)} onLostPointerCapture={() => setDrag(undefined)}
    onKeyDown={e => {
      if (e.key === "Escape" && drag()) { e.stopPropagation(); setDrag(undefined); }
    }}>
    <div class="month-weekdays"><For each={props.dates.slice(0, 7)}>{date => <span>{weekday(date)}</span>}</For></div>
    <For each={weeks()}>{week => {
      const layout = createMemo(() => monthWeek(week, props.events));
      return <div class="month-week">
        <div class="month-cells" aria-hidden="true"><For each={week}>{date => <div data-month-date={date}
          classList={{ "in-range": inRange(date) }}
          onPointerDown={e => {
            if (!props.writable || !e.isPrimary || e.button !== 0) return;
            month.focus({ preventScroll: true });
            e.currentTarget.setPointerCapture(e.pointerId);
            setDrag({ anchor: date, over: date, time: startTime(e.currentTarget, e.clientY), pointer: e.pointerId });
          }}
          onDblClick={e => {
            if (props.writable && e.button === 0) props.create(date, date, startTime(e.currentTarget, e.clientY));
          }} />}</For></div>
        <For each={week}>{date => <button class="month-day" classList={{ outside: date.slice(0, 7) !== props.date.slice(0, 7) }}
          aria-label={dateWords(date)} aria-current={date === props.date ? "date" : undefined}
          onClick={() => props.openDay(date)}>{Number(date.slice(8))}</button>}</For>
        <For each={layout().entries}>{x => <div class="month-entry" style={{
          "grid-column": (x.first + 1) + " / " + (x.last + 2), "grid-row": x.row + 2,
        }}>{props.card(x.event, x.event.all_day ? undefined : timeLabel(x.event, week[x.first]!))}</div>}</For>
        <For each={layout().more}>{(n, i) => <Show when={n}><button class="month-more"
          style={{ "grid-column": i() + 1, "grid-row": 5 }} onClick={() => props.openDay(week[i()]!)}>
          +{n} more</button></Show>}</For>
      </div>;
    }}</For>
  </div>;
}
