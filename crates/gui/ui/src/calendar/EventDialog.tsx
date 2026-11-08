// Add to calendar (WP AI.3): shows the events `cal propose` finds in an email,
// each editable (repeat too), with a calendar picker across accounts and the
// chosen calendar's other events on those days. Add runs `cal add` once per
// ticked event. A meeting invitation is answered with `cal rsvp` instead.
import { For, Match, Show, Switch, createEffect, createResource, createSignal, onCleanup, onMount } from "solid-js";
import { createStore, produce } from "solid-js/store";
import { isCommandError, run } from "../api";
import { aiSettings, notify } from "../state";
import {
  type Calendar,
  DAYS,
  type Draft,
  type Every,
  addInput,
  calendarKey,
  dateWords,
  defaultCalendar,
  draftOf,
  problem,
  rememberCalendar,
  repeatWords,
  sourceWords,
  toolWords,
} from "./proposals";
import "./calendar.css";

const errorText = (e: unknown) => (isCommandError(e) ? e.message : String(e));

type Created = { id: number | null; event: number | null; series: boolean };

export function EventDialog(props: { message: number; from: string; onClose: () => void }) {
  const [proposal] = createResource(() => run("cal.propose", { from_mail: props.message }));
  const [calendars] = createResource(async () =>
    (await run("cal.calendars", {})).calendars.filter((c) => c.can_edit),
  );
  const [drafts, setDrafts] = createStore<Draft[]>([]);
  const [chosen, setChosen] = createSignal<string>("");
  const [editing, setEditing] = createSignal<number | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [created, setCreated] = createSignal<Created[] | null>(null);
  const [tell, setTell] = createSignal(true);

  // Drafts and the calendar are filled once both answers are in.
  let filled = false;
  createEffect(() => {
    const p = proposal();
    const cals = calendars();
    if (!p || !cals || filled) return;
    filled = true;
    setDrafts(p.events.map(draftOf));
    const d = defaultCalendar(cals, p.account, props.from);
    if (d) setChosen(calendarKey(d));
  });
  const ready = () => proposal() !== undefined && calendars() !== undefined;

  const calendar = (): Calendar | undefined => calendars()?.find((c) => calendarKey(c) === chosen());
  const ticked = () => drafts.filter((d) => d.add);
  const blocking = () => ticked().map(problem).find((p) => p !== null) ?? null;
  const accounts = () => [...new Set((calendars() ?? []).map((c) => c.account))];

  // The chosen calendar's events on each ticked event's first day.
  const [clashes] = createResource(
    () => {
      const c = calendar();
      if (!c) return null;
      const days = [...new Set(ticked().map((d) => `${d.startDate}|${d.zone}`))].sort();
      return { c, days };
    },
    async ({ c, days }) =>
      Promise.all(
        days.map(async (k) => {
          const [date = "", tz = ""] = k.split("|");
          try {
            const out = await run("cal.list", { from: date, days: 1, account: c.account, calendar: c.name, tz });
            // Warnings mean the day isn't fully synced here, so "nothing" isn't known.
            return { date, events: out.events.filter((e) => !e.cancelled), known: (out.warnings ?? []).length === 0 };
          } catch {
            return { date, events: [], known: false };
          }
        }),
      ),
  );

  onMount(() => {
    const esc = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      e.preventDefault();
      if (editing() !== null) setEditing(null);
      else props.onClose();
    };
    window.addEventListener("keydown", esc);
    onCleanup(() => window.removeEventListener("keydown", esc));
  });

  const set = <K extends keyof Draft>(i: number, k: K, v: Draft[K]) => setDrafts(i, k, v);

  function addBlank() {
    const today = new Date();
    const d = `${today.getFullYear()}-${String(today.getMonth() + 1).padStart(2, "0")}-${String(today.getDate()).padStart(2, "0")}`;
    setDrafts(drafts.length, {
      ...draftOf({
        subject: "",
        start: `${d} 09:00`,
        end: `${d} 10:00`,
        all_day: false,
        time_zone: Intl.DateTimeFormat().resolvedOptions().timeZone,
      }),
    });
  }

  async function add() {
    const c = calendar();
    if (!c) return;
    setBusy(true);
    setError(null);
    const done: Created[] = [];
    try {
      for (const d of ticked()) {
        const out = await run("cal.add", addInput(d, props.message, c));
        for (const op of out.ops) done.push({ id: op.id ?? null, event: op.item ?? null, series: d.every !== "none" });
      }
      rememberCalendar(props.from, c);
      notify(done.length === 1 ? `Added to ${c.account} · ${c.name}` : `Added ${done.length} events to ${c.account} · ${c.name}`);
      setCreated(done);
    } catch (e) {
      // Events added before the failure stay; undo still covers them.
      setError(`Couldn't add: ${errorText(e)}`);
      if (done.length > 0) setCreated(done);
    } finally {
      setBusy(false);
    }
  }

  async function undo() {
    const list = created() ?? [];
    setBusy(true);
    try {
      for (const c of list) {
        // A create still queued is cancelled; one the server has is deleted.
        const cancelled = c.id !== null && (await run("ops.cancel", { id: c.id })).changed;
        if (!cancelled && c.event !== null) await run("cal.delete", { event: c.event, series: c.series });
      }
      notify(list.length === 1 ? "Event removed" : `${list.length} events removed`);
      props.onClose();
    } catch (e) {
      setError(`Couldn't undo: ${errorText(e)}`);
    } finally {
      setBusy(false);
    }
  }

  async function answer(event: number, response: "accept" | "tentative" | "decline") {
    setBusy(true);
    setError(null);
    try {
      await run("cal.rsvp", { event, response, no_reply: !tell() });
      notify(response === "accept" ? "Accepted" : response === "tentative" ? "Answered tentative" : "Declined");
      props.onClose();
    } catch (e) {
      setError(`Couldn't answer: ${errorText(e)}`);
    } finally {
      setBusy(false);
    }
  }

  const failed = () => (proposal.error ? errorText(proposal.error) : calendars.error ? errorText(calendars.error) : null);

  return (
    <div class="palette-backdrop" onMouseDown={(e) => e.target === e.currentTarget && !busy() && props.onClose()}>
      <div
        class="palette event-dialog"
        role="dialog"
        aria-label={proposal()?.invitation ? "Invitation" : "Add to calendar"}
      >
        <Switch>
          <Match when={failed()}>
            {(f) => (
              <div class="event-body">
                <h2>Add to calendar</h2>
                <p class="warn" role="alert">{f()}</p>
                <div class="event-actions">
                  <button onClick={props.onClose}>Close</button>
                </div>
              </div>
            )}
          </Match>
          <Match when={!ready()}>
            <div class="event-body">
              <h2>Add to calendar</h2>
              <p class="event-wait" role="status">
                <span class="spinner" aria-hidden="true" /> Looking for events in this email…
              </p>
              <Show when={aiSettings.tool}>
                <p class="muted">
                  Without an invitation or calendar file, the email goes to {toolWords(aiSettings.tool)}, which can take
                  10 to 20 seconds.
                </p>
              </Show>
              <div class="event-actions">
                <button onClick={props.onClose}>Cancel</button>
              </div>
            </div>
          </Match>
          <Match when={proposal()?.invitation}>
            {(inv) => (
              <div class="event-body">
                <h2>{inv().kind === "cancelled" ? "Meeting cancelled" : "Invitation"}</h2>
                <Show
                  when={inv().event}
                  fallback={
                    <p>This meeting hasn't reached your calendar on this computer yet. Try again in a few minutes.</p>
                  }
                >
                  {(event) => (
                    <>
                      <div class="event-invite">
                        <strong>{inv().subject}</strong>
                        <span>{inv().when}</span>
                        <span class="muted">
                          {proposal()?.account} · {inv().calendar}
                          <Show when={inv().response}>{(r) => <> ({r()})</>}</Show>
                        </span>
                      </div>
                      <Show when={inv().kind !== "cancelled"}>
                        <label class="event-check">
                          <input type="checkbox" checked={tell()} onChange={(e) => setTell(e.currentTarget.checked)} />
                          Tell the organiser
                        </label>
                        <Show when={error()}>{(e) => <p class="warn" role="alert">{e()}</p>}</Show>
                        <div class="event-actions">
                          <button onClick={props.onClose} disabled={busy()}>Cancel</button>
                          <span class="spacer" />
                          <button onClick={() => void answer(event(), "decline")} disabled={busy()}>Decline</button>
                          <button onClick={() => void answer(event(), "tentative")} disabled={busy()}>Tentative</button>
                          <button class="primary" onClick={() => void answer(event(), "accept")} disabled={busy()}>
                            Accept
                          </button>
                        </div>
                      </Show>
                    </>
                  )}
                </Show>
                <Show when={!inv().event || inv().kind === "cancelled"}>
                  <div class="event-actions">
                    <button onClick={props.onClose}>Close</button>
                  </div>
                </Show>
              </div>
            )}
          </Match>
          <Match when={created()}>
            {(list) => (
              <div class="event-body">
                <h2>Added</h2>
                <p>
                  {list().length === 1 ? "1 event was" : `${list().length} events were`} added to {calendar()?.account} ·{" "}
                  {calendar()?.name}.
                </p>
                <Show when={error()}>{(e) => <p class="warn" role="alert">{e()}</p>}</Show>
                <div class="event-actions">
                  <button onClick={() => void undo()} disabled={busy()}>Undo</button>
                  <span class="spacer" />
                  <button class="primary" ref={(el) => queueMicrotask(() => el.focus())} onClick={props.onClose}>
                    Close
                  </button>
                </div>
              </div>
            )}
          </Match>
          <Match when={true}>
            <div class="event-body">
              <h2>Add to calendar</h2>
              <div class="event-scroll">
              <For each={proposal()?.notes ?? []}>{(n) => <p class="muted event-note">{n}</p>}</For>
              <Show when={drafts.length === 0}>
                <p>No events found in this email.</p>
              </Show>
              <ul class="event-list">
                <For each={drafts}>
                  {(d, i) => (
                    <li class="event-item" classList={{ off: !d.add }}>
                      <input
                        type="checkbox"
                        class="event-tick"
                        aria-label={`Add ${d.subject || "this event"}`}
                        checked={d.add}
                        onChange={(e) => set(i(), "add", e.currentTarget.checked)}
                      />
                      <div class="event-fields">
                        <input
                          class="event-subject"
                          aria-label="Subject"
                          placeholder="Subject"
                          value={d.subject}
                          onInput={(e) => set(i(), "subject", e.currentTarget.value)}
                        />
                        <div class="event-when">
                          <input
                            type="date"
                            aria-label="Start date"
                            value={d.startDate}
                            onInput={(e) => {
                              const v = e.currentTarget.value;
                              // Moving the start moves a same-day end with it.
                              setDrafts(
                                i(),
                                produce((x) => {
                                  if (x.endDate === x.startDate) x.endDate = v;
                                  x.startDate = v;
                                }),
                              );
                            }}
                          />
                          <Show when={!d.allDay}>
                            <input
                              type="time"
                              aria-label="Start time"
                              value={d.startTime}
                              onInput={(e) => set(i(), "startTime", e.currentTarget.value)}
                            />
                          </Show>
                          <span>to</span>
                          <input
                            type="date"
                            aria-label="End date"
                            value={d.endDate}
                            onInput={(e) => set(i(), "endDate", e.currentTarget.value)}
                          />
                          <Show when={!d.allDay}>
                            <input
                              type="time"
                              aria-label="End time"
                              value={d.endTime}
                              onInput={(e) => set(i(), "endTime", e.currentTarget.value)}
                            />
                          </Show>
                          <label class="event-check">
                            <input
                              type="checkbox"
                              checked={d.allDay}
                              onChange={(e) => set(i(), "allDay", e.currentTarget.checked)}
                            />
                            All day
                          </label>
                          <input
                            class="event-zone"
                            aria-label="Time zone"
                            value={d.zone}
                            onInput={(e) => set(i(), "zone", e.currentTarget.value)}
                          />
                        </div>
                        <input
                          aria-label="Location"
                          placeholder="Location"
                          value={d.location}
                          onInput={(e) => set(i(), "location", e.currentTarget.value)}
                        />
                        <div class="event-repeat">
                          <span>Repeats: {repeatWords(d)}</span>
                          <button
                            class="event-link"
                            aria-expanded={editing() === i()}
                            onClick={() => setEditing(editing() === i() ? null : i())}
                          >
                            {editing() === i() ? "done" : "change"}
                          </button>
                        </div>
                        <Show when={editing() === i()}>
                          <RepeatEditor draft={d} set={(k, v) => set(i(), k, v)} />
                        </Show>
                        <Show when={d.notes}>
                          <textarea
                            aria-label="Notes"
                            rows={2}
                            value={d.notes}
                            onInput={(e) => set(i(), "notes", e.currentTarget.value)}
                          />
                        </Show>
                        <div class="event-meta muted">
                          <Show when={d.evidence}>{(q) => <q>{q()}</q>}</Show>
                          <span class="spacer" />
                          <Show when={d.already !== null}>
                            <span>already in your calendar</span>
                          </Show>
                          <span>{sourceWords(proposal()?.source ?? "")}</span>
                        </div>
                        <Show when={d.add && problem(d)}>{(p) => <span class="warn">Can't add: {p()}</span>}</Show>
                      </div>
                    </li>
                  )}
                </For>
              </ul>
              <button class="event-link" onClick={addBlank}>Add an event by hand</button>
              </div>
              <label class="event-calendar">
                Calendar:
                <select value={chosen()} onChange={(e) => setChosen(e.currentTarget.value)}>
                  <For each={accounts()}>
                    {(a) => (
                      <optgroup label={a}>
                        <For each={(calendars() ?? []).filter((c) => c.account === a)}>
                          {(c) => (
                            <option value={calendarKey(c)}>
                              {c.account} · {c.name}
                              {c.purposes && c.purposes.length > 0 ? `  (${c.purposes.join(", ")})` : ""}
                            </option>
                          )}
                        </For>
                      </optgroup>
                    )}
                  </For>
                </select>
              </label>
              <Show when={clashes()}>
                {(list) => (
                  <ul class="event-clashes muted">
                    <For each={list()}>
                      {(c) => (
                        <li>
                          {dateWords(c.date)} in {calendar()?.name}:{" "}
                          {c.events.length === 0
                            ? c.known
                              ? "nothing else"
                              : "not synced on this computer, so not checked"
                            : c.events
                                .map((e) => `${e.all_day ? "all day" : e.start.slice(11, 16)} ${e.subject}`)
                                .join(", ")}
                        </li>
                      )}
                    </For>
                  </ul>
                )}
              </Show>
              <Show when={error()}>{(e) => <p class="warn" role="alert">{e()}</p>}</Show>
              <div class="event-actions">
                <span class="spacer" />
                <button onClick={props.onClose} disabled={busy()}>Cancel</button>
                <button
                  class="primary"
                  disabled={busy() || ticked().length === 0 || blocking() !== null || !calendar()}
                  onClick={() => void add()}
                >
                  {busy() ? "Adding…" : ticked().length === 1 ? "Add 1 event" : `Add ${ticked().length} events`}
                </button>
              </div>
            </div>
          </Match>
        </Switch>
      </div>
    </div>
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
