import { For, Show } from "solid-js";
import { DAYS, type Draft, type Every, repeatWords } from "./proposals";

type SetField = <K extends keyof Draft>(key: K, value: Draft[K]) => void;

export function EventFields(props: {
  draft: Draft;
  set: SetField;
  editing: boolean;
  toggleRepeat: () => void;
  showNotes?: boolean;
}) {
  return (
    <>
      <input
        class="event-subject"
        aria-label="Subject"
        placeholder="Subject"
        value={props.draft.subject}
        onInput={(e) => props.set("subject", e.currentTarget.value)}
      />
      <div class="event-when">
        <input
          type="date"
          aria-label="Start date"
          value={props.draft.startDate}
          onInput={(e) => {
            const v = e.currentTarget.value;
            // Moving the start moves a same-day end with it.
            if (props.draft.endDate === props.draft.startDate) props.set("endDate", v);
            props.set("startDate", v);
          }}
        />
        <Show when={!props.draft.allDay}>
          <input
            type="time"
            aria-label="Start time"
            value={props.draft.startTime}
            onInput={(e) => props.set("startTime", e.currentTarget.value)}
          />
        </Show>
        <span>to</span>
        <input
          type="date"
          aria-label="End date"
          value={props.draft.endDate}
          onInput={(e) => props.set("endDate", e.currentTarget.value)}
        />
        <Show when={!props.draft.allDay}>
          <input
            type="time"
            aria-label="End time"
            value={props.draft.endTime}
            onInput={(e) => props.set("endTime", e.currentTarget.value)}
          />
        </Show>
        <label class="event-check">
          <input
            type="checkbox"
            checked={props.draft.allDay}
            onChange={(e) => props.set("allDay", e.currentTarget.checked)}
          />
          All day
        </label>
        <input
          class="event-zone"
          aria-label="Time zone"
          value={props.draft.zone}
          onInput={(e) => props.set("zone", e.currentTarget.value)}
        />
      </div>
      <input
        aria-label="Location"
        placeholder="Location"
        value={props.draft.location}
        onInput={(e) => props.set("location", e.currentTarget.value)}
      />
      <div class="event-repeat">
        <span>Repeats: {repeatWords(props.draft)}</span>
        <button
          class="event-link"
          aria-expanded={props.editing}
          onClick={() => props.toggleRepeat()}
        >
          {props.editing ? "done" : "change"}
        </button>
      </div>
      <Show when={props.editing}>
        <RepeatEditor draft={props.draft} set={props.set} />
      </Show>
      <Show when={props.showNotes || props.draft.notes}>
        <textarea
          aria-label="Notes"
          rows={2}
          value={props.draft.notes}
          onInput={(e) => props.set("notes", e.currentTarget.value)}
        />
      </Show>
    </>
  );
}

const EVERY: { value: Every; label: string }[] = [
  { value: "none", label: "Doesn't repeat" },
  { value: "daily", label: "Daily" },
  { value: "weekly", label: "Weekly" },
  { value: "monthly", label: "Monthly" },
  { value: "yearly", label: "Yearly" },
];

function RepeatEditor(props: { draft: Draft; set: <K extends keyof Draft>(k: K, v: Draft[K]) => void }) {
  const d = () => props.draft;
  return (
    <div class="event-repeat-editor">
      <select aria-label="Repeat" value={d().every} onChange={(e) => props.set("every", e.currentTarget.value as Every)}>
        <For each={EVERY}>{(o) => <option value={o.value}>{o.label}</option>}</For>
      </select>
      <Show when={d().every !== "none"}>
        <label>
          every
          <input
            type="number"
            min="1"
            aria-label="Interval"
            value={d().interval}
            onInput={(e) => props.set("interval", Math.max(1, Number(e.currentTarget.value) || 1))}
          />
        </label>
        <Show when={d().every === "weekly"}>
          <span class="event-days">
            <For each={DAYS}>
              {(w) => (
                <label>
                  <input
                    type="checkbox"
                    checked={d().on.includes(w)}
                    onChange={(e) =>
                      props.set(
                        "on",
                        e.currentTarget.checked
                          ? DAYS.filter((x) => x === w || d().on.includes(x))
                          : d().on.filter((x) => x !== w),
                      )
                    }
                  />
                  {w.charAt(0).toUpperCase() + w.slice(1)}
                </label>
              )}
            </For>
          </span>
        </Show>
        <select aria-label="Ends" value={d().ends} onChange={(e) => props.set("ends", e.currentTarget.value as Draft["ends"])}>
          <option value="until">until</option>
          <option value="count">for</option>
          <option value="never">with no end</option>
        </select>
        <Show when={d().ends === "until"}>
          <input type="date" aria-label="Until" value={d().until} onInput={(e) => props.set("until", e.currentTarget.value)} />
        </Show>
        <Show when={d().ends === "count"}>
          <label>
            <input
              type="number"
              min="1"
              aria-label="Times"
              value={d().count}
              onInput={(e) => props.set("count", Math.max(1, Number(e.currentTarget.value) || 1))}
            />
            times
          </label>
        </Show>
      </Show>
    </div>
  );
}
