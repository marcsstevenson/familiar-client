import type { CalEvent } from "../../../../../bindings/commands";

/** Clip timed events to a local day and share the width between overlapping appointments. */
export function timedBlocks(events: CalEvent[], start: number, end: number) {
  const blocks = events.filter(e => !e.all_day).map(event => ({
    event, start: Math.max(start, Date.parse(event.start)), end: Math.min(end, Date.parse(event.end)),
    column: 0, columns: 1,
  })).filter(b => b.end > b.start).sort((a, b) => a.start - b.start || b.end - a.end);
  let groupStart = 0;
  let groupEnd = start;
  const ends: number[] = [];
  function finishGroup(until: number) {
    for (let i = groupStart; i < until; i++) blocks[i]!.columns = ends.length;
    ends.length = 0;
    groupStart = until;
  }
  for (const [i, block] of blocks.entries()) {
    if (block.start >= groupEnd) finishGroup(i);
    const available = ends.findIndex(t => t <= block.start);
    block.column = available < 0 ? ends.length : available;
    ends[block.column] = block.end;
    groupEnd = Math.max(groupEnd, block.end);
  }
  finishGroup(blocks.length);
  return blocks;
}

export function localDate(at: Date) {
  const pad = (n: number) => String(n).padStart(2, "0");
  return at.getFullYear() + "-" + pad(at.getMonth() + 1) + "-" + pad(at.getDate());
}

export function nextDate(date: string, days: number) {
  const d = new Date(date + "T12:00:00");
  d.setDate(d.getDate() + days);
  return localDate(d);
}

export const views = ["Day", "Week", "Month", "Agenda"] as const;
export type CalendarView = typeof views[number];

export function monday(date: string) {
  return nextDate(date, -((new Date(date + "T12:00").getDay() + 6) % 7));
}

export function shiftMonth(date: string, months: number) {
  const d = new Date(date + "T12:00");
  const last = new Date(d.getFullYear(), d.getMonth() + months + 1, 0, 12);
  last.setDate(Math.min(d.getDate(), last.getDate()));
  return localDate(last);
}

export function datesFrom(first: string, count: number) {
  return Array.from({ length: count }, (_, i) => nextDate(first, i));
}

export function calendarRange(view: CalendarView, date: string) {
  if (view === "Week") return { from: monday(date), days: 7 };
  if (view !== "Month") return { from: date, days: view === "Day" ? 1 : 30 };
  const first = date.slice(0, 8) + "01";
  const from = monday(first);
  const next = shiftMonth(first, 1);
  let days = 28;
  while (nextDate(from, days) < next) days += 7;
  return { from, days };
}

/** All-day end dates are exclusive. Timed endpoints are instants in the display zone. */
export function onDay(e: CalEvent, date: string) {
  if (e.all_day) return e.start <= date && date < e.end;
  return Date.parse(e.start) < new Date(nextDate(date, 1) + "T00:00").getTime() &&
    Date.parse(e.end) > new Date(date + "T00:00").getTime();
}
