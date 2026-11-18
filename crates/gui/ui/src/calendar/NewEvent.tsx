import { For, Show, createSignal, onMount } from "solid-js";
import { createStore } from "solid-js/store";
import { isCommandError, run } from "../api";
import { EventFields } from "./EventFields";
import { type Calendar, type Draft, addInput, draftOf, problem } from "./proposals";

export function NewEvent(props: {
  date: string; time: string; zone: string; calendars: Calendar[];
  onClose: () => void; onSaved: (calendar: Calendar, date: string) => void;
}) {
  const [draft, setDraft] = createStore(draftOf({
    subject: "", start: props.date + " " + props.time, all_day: false, time_zone: props.zone,
  }));
  const [id, setId] = createSignal((props.calendars.find(c => c.is_default) ?? props.calendars[0])?.id);
  const [editing, setEditing] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const calendar = () => props.calendars.find(c => c.id === id());
  let dialog!: HTMLDialogElement;
  onMount(() => dialog.showModal());

  async function save() {
    const c = calendar();
    if (!c || busy() || problem(draft)) return;
    setBusy(true);
    try {
      await run("cal.add", addInput(draft, undefined, c));
      props.onSaved(c, draft.startDate);
    } catch (e) {
      setError(isCommandError(e) ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <dialog ref={dialog} class="new-event event-dialog" role="dialog" aria-label="New event"
      onCancel={e => { e.preventDefault(); if (!busy()) props.onClose(); }}>
      <div class="event-body">
        <h2>New event</h2>
        <div class="event-fields">
          <EventFields draft={draft} set={<K extends keyof Draft>(key: K, value: Draft[K]) => setDraft(key, value)}
            editing={editing()} toggleRepeat={() => setEditing(!editing())} showNotes />
        </div>
        <label class="event-calendar">Calendar
          <select aria-label="Calendar" onChange={e => setId(Number(e.currentTarget.value))}>
            <For each={props.calendars}>{c => <option value={c.id} selected={id() === c.id}>{c.account} · {c.name}</option>}</For>
          </select>
        </label>
        <Show when={draft.subject && problem(draft)}>{p => <span class="warn">{p()}</span>}</Show>
        <Show when={error()}>{message => <span class="warn" role="alert">{message()}</span>}</Show>
        <div class="event-actions"><span class="spacer" />
          <button onClick={props.onClose} disabled={busy()}>Cancel</button>
          <button class="primary" disabled={busy() || !!problem(draft) || !calendar()} onClick={() => void save()}>
            {busy() ? "Saving…" : "Save"}
          </button>
        </div>
      </div>
    </dialog>
  );
}
