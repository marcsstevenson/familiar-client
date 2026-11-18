import { For, createEffect, createMemo, createSignal } from "solid-js";
import { dateWords } from "./proposals";
import { calendarRange, datesFrom, shiftMonth } from "./timeline";

export function MiniMonth(props: { date: string; navigate: (date: string) => void }) {
  const [month, setMonth] = createSignal(props.date.slice(0, 8) + "01");
  createEffect(() => setMonth(props.date.slice(0, 8) + "01"));
  const days = createMemo(() => {
    const range = calendarRange("Month", month());
    return datesFrom(range.from, range.days);
  });
  return <section class="mini-month" aria-label="Choose a calendar day">
    <header><button aria-label="Previous mini month" onClick={() => setMonth(shiftMonth(month(), -1))}>‹</button>
      <strong>{new Date(month() + "T12:00").toLocaleDateString(undefined, { month: "long", year: "numeric" })}</strong>
      <button aria-label="Next mini month" onClick={() => setMonth(shiftMonth(month(), 1))}>›</button></header>
    <div><For each={days().slice(0, 7)}>{d => <span>{new Date(d + "T12:00").toLocaleDateString(undefined, { weekday: "narrow" })}</span>}</For>
      <For each={days()}>{d => <button classList={{ outside: d.slice(0, 7) !== month().slice(0, 7) }}
        aria-label={dateWords(d)} aria-current={d === props.date ? "date" : undefined} onClick={() => props.navigate(d)}>{Number(d.slice(8))}</button>}</For>
    </div>
  </section>;
}
