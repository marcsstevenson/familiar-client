import { Show, createSignal } from "solid-js";
import { createStore } from "solid-js/store";
import type { CalEditInput, CalShowOutput } from "../../../../../bindings/commands";
import { EventFields } from "./EventFields";
import { draftOf, type Draft } from "./proposals";
import { nextDate } from "./timeline";

function fields(detail: CalShowOutput) {
  const e = detail.event;
  return draftOf({
    subject: e.subject, location: e.location, all_day: e.all_day, time_zone: detail.display_zone,
    start: e.all_day ? e.start : e.start.slice(0, 16).replace("T", " "),
    end: e.all_day ? nextDate(e.end, -1) : e.end.slice(0, 16).replace("T", " "),
  });
}

function when(d: Draft, end: boolean) {
  const date = end ? d.endDate : d.startDate;
  return d.allDay ? date : date + " " + (end ? d.endTime : d.startTime);
}

export function EditEvent(props: {
  detail: CalShowOutput; series: boolean; busy: boolean;
  onCancel: () => void; onSave: (patch: CalEditInput) => void;
}) {
  // Take the baseline once. Background sync may refresh the details while typing.
  const initial = fields(props.detail);
  const originalReminder = props.detail.reminder_minutes?.toString() ?? "";
  const [reminder, setReminder] = createSignal(originalReminder);
  const [draft, setDraft] = createStore({ ...initial });
  const [seriesTimes, setSeriesTimes] = createSignal(false);
  const editTimes = () => !props.series || seriesTimes();
  const retimed = () => editTimes() && (seriesTimes() || when(initial, false) !== when(draft, false)
    || when(initial, true) !== when(draft, true) || draft.zone !== initial.zone);
  const organizer = () => props.detail.is_organizer;
  const changes = (): Omit<CalEditInput, "event" | "series"> => {
    const patch: Omit<CalEditInput, "event" | "series"> = {};
    if (organizer()) {
      if (draft.subject !== initial.subject) patch.subject = draft.subject.trim();
      if (draft.location !== initial.location) patch.location = draft.location;
      if (retimed()) {
        patch.tz = draft.zone;
        if (seriesTimes() || draft.zone !== initial.zone || when(draft, false) !== when(initial, false)) patch.start = when(draft, false);
        patch.end = when(draft, true);
      }
    }
    if (reminder() !== originalReminder) {
      if (reminder() === "") patch.no_reminder = true;
      else patch.reminder = Number(reminder());
    }
    return patch;
  };
  const invalid = () => {
    if (organizer() && draft.subject !== initial.subject && !draft.subject.trim()) return "Enter an event title.";
    if (organizer() && retimed()) {
      if (!draft.startDate || !draft.endDate || (!draft.allDay && (!draft.startTime || !draft.endTime)) || !draft.zone.trim())
        return "Enter a start, end and time zone.";
      if (draft.allDay ? draft.endDate < draft.startDate : when(draft, true) <= when(draft, false))
        return "The end must be after the start.";
    }
    if (reminder() && (!Number.isSafeInteger(Number(reminder())) || Number(reminder()) < 0)) return "Enter a whole number of minutes.";
    return "";
  };

  return <div class="event-fields">
    <Show when={organizer()}>
      <Show when={props.series}>
        <label><input type="checkbox" checked={seriesTimes()} onChange={e => {
          setSeriesTimes(e.currentTarget.checked);
          // A series PATCH targets its master. A later occurrence's date would move the first event.
          setDraft({ startDate: e.currentTarget.checked ? "" : initial.startDate, endDate: e.currentTarget.checked ? "" : initial.endDate });
        }} /> Change the series start and end</label>
        <Show when={seriesTimes()}><p>Enter the dates and times for the first event of the entire series.</p></Show>
      </Show>
      <EventFields draft={draft} set={<K extends keyof Draft>(key: K, value: Draft[K]) => setDraft(key, value)} showWhen={editTimes()} />
    </Show>
    <label>Reminder in minutes before the event
      <input type="number" min="0" step="1" placeholder="Off" value={reminder()} onInput={e => setReminder(e.currentTarget.value)} />
    </label>
    <Show when={invalid()}>{msg => <p class="warn">{msg()}</p>}</Show>
    <div class="event-actions">
      <button disabled={props.busy} onClick={props.onCancel}>Back</button>
      <span class="spacer" />
      <button class="primary" disabled={props.busy || !!invalid() || Object.keys(changes()).length === 0}
        onClick={() => props.onSave({ event: props.detail.event.id, series: props.series, ...changes() })}>{props.busy ? "Saving…" : "Save changes"}</button>
    </div>
  </div>;
}
