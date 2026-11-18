import { For, Show, createResource, onCleanup, onMount } from "solid-js";
import { run } from "../api";
import { onStoreChanged } from "../list/api";
import { dateWords, type Calendar, type Draft } from "./proposals";

export function ProposalSchedule(props: { draft: Draft; calendar: Calendar }) {
  const [range, { refetch }] = createResource(
    () => props.draft.startDate && props.draft.zone ? {
      from: props.draft.startDate, tz: props.draft.zone, account: props.calendar.account, days: 1,
    } : false,
    query => run("cal.list", query),
  );
  onMount(() => {
    const stop = onStoreChanged(() => void refetch());
    onCleanup(() => void stop.then(f => f()));
  });
  const events = () => range.error || range.loading || !props.draft.startDate || !props.draft.zone ? [] :
    range()?.events.filter(e => !e.cancelled && e.calendar_id === props.calendar.id) ?? [];
  const covered = () => props.calendar.synced_from && props.calendar.synced_to &&
    props.draft.startDate >= props.calendar.synced_from && props.draft.startDate < props.calendar.synced_to;
  return <aside class="proposal-schedule" aria-label="Destination schedule">
    <h3>{dateWords(props.draft.startDate) || "Choose a date"}</h3>
    <p>{props.calendar.account} · {props.calendar.name}</p>
    <p class="muted">{props.draft.zone}</p>
    <Show when={props.draft.startDate && props.draft.zone}>
      <p class="proposed-slot"><strong>{props.draft.allDay ? "All day" : props.draft.startTime + " – " + props.draft.endTime}</strong>
        {props.draft.subject || "Proposed event"}</p>
      <Show when={range.loading}><p role="status">Loading schedule…</p></Show>
      <Show when={range.error}><p role="alert">Couldn't load this day's schedule. Check the date and time zone.</p></Show>
      <Show when={!covered() || props.calendar.error}><p class="warn">{props.calendar.error || "This day hasn't synced."}</p></Show>
      <Show when={!range.error && !range.loading && covered() && !props.calendar.error && !events().length}><p>No other events.</p></Show>
      <For each={events()}>{e => {
        const overlaps = () => !props.draft.allDay && !e.all_day && e.show_as !== "free" && e.response !== "declined" &&
          e.start.slice(0, 16) < props.draft.endDate + "T" + props.draft.endTime &&
          e.end.slice(0, 16) > props.draft.startDate + "T" + props.draft.startTime;
        return <p class="schedule-entry" classList={{ conflict: overlaps() }}>
          <strong>{e.all_day ? "All day" : e.start.slice(11, 16) + " – " + e.end.slice(11, 16)}</strong>
          {e.subject}<Show when={overlaps()}><small>Overlaps proposed time</small></Show>
        </p>;
      }}</For>
    </Show>
  </aside>;
}
