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
