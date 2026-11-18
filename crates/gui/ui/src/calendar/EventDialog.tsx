// Add to calendar (WP AI.3): shows the events `cal propose` finds in an email,
// each editable (repeat too), with a calendar picker across accounts and the
// chosen calendar's other events on those days. Add runs `cal add` once per
// ticked event. A meeting invitation is answered with `cal rsvp` instead.
import { For, Match, Show, Switch, createEffect, createResource, createSignal, onCleanup, onMount } from "solid-js";
import { ProposalSchedule } from "./ProposalSchedule";
import { localDate } from "./timeline";
import { EventFields } from "./EventFields";
import { createStore } from "solid-js/store";
import { isCommandError, run } from "../api";
import { aiSettings, notify } from "../state";
import {
  type Calendar,
  type Draft,
  addInput,
  calendarKey,
  defaultCalendar,
  draftOf,
  problem,
  rememberCalendar,
  sourceWords,
  toolWords,
} from "./proposals";
import "./calendar.css";

const errorText = (e: unknown) => (isCommandError(e) ? e.message : String(e));

type Created = { id: number | null; event: number | null; series: boolean; guests: boolean };

export function EventDialog(props: { message: number; from: string; onClose: () => void }) {
  const [proposal] = createResource(() => run("cal.propose", { from_mail: props.message, with_mail: true }));
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

  const [active, setActive] = createSignal(0);
  const [guests, setGuests] = createStore<Record<number, string[]>>({});
  let heading!: HTMLDivElement;
  // Drafts and the calendar are filled once both answers are in.
  let filled = false;
  createEffect(() => {
    if (proposal.error || calendars.error) return;
    const p = proposal();
    const cals = calendars();
    if (!p || !cals || filled) return;
    filled = true;
    setDrafts(p.events.map(e => draftOf(p.mail && p.events.length === 1 ? { ...e, subject: p.mail.subject } : e)));
    if (!drafts.length && p.mail) addBlank();
    setActive(Math.max(0, drafts.findIndex(d => d.add)));
    const d = defaultCalendar(cals, p.account, props.from);
    if (d) setChosen(calendarKey(d));
  });
  const ready = () => proposal() !== undefined && calendars() !== undefined;

  const calendar = (): Calendar | undefined => calendars()?.find((c) => calendarKey(c) === chosen());
  const ticked = () => drafts.filter((d) => d.add);
  const blocking = () => ticked().map(problem).find((p) => p !== null) ?? null;
  const accounts = () => [...new Set((calendars() ?? []).map((c) => c.account))];

  onMount(() => {
    heading.focus();
    const esc = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      e.preventDefault();
      if (busy()) return;
      if (editing() !== null) setEditing(null);
      else props.onClose();
    };
    window.addEventListener("keydown", esc);
    onCleanup(() => window.removeEventListener("keydown", esc));
  });

  const set = <K extends keyof Draft>(i: number, k: K, v: Draft[K]) => setDrafts(i, k, v);

  function addBlank() {
    const d = localDate(new Date());
    setActive(drafts.length);
    setDrafts(drafts.length, {
      ...draftOf({
        subject: proposal()?.mail?.subject ?? "",
        start: `${d} 09:00`,
        end: `${d} 10:00`,
        all_day: false,
        time_zone: Intl.DateTimeFormat().resolvedOptions().timeZone,
      }),
    });
  }

  async function add() {
    const c = calendar();
    if (!c || busy()) return;
    setBusy(true);
    setError(null);
    const done: Created[] = [];
    try {
      for (const [index, d] of drafts.entries()) {
        if (!d.add) continue;
        const attendee = guests[index] ?? [];
        const out = await run("cal.add", { ...addInput(d, props.message, c), attendee, yes: attendee.length > 0 });
        for (const op of out.ops) done.push({ id: op.id ?? null, event: op.item ?? null, series: d.every !== "none", guests: attendee.length > 0 });
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
        if (!cancelled && c.event !== null) await run("cal.delete", { event: c.event, series: c.series, yes: c.guests });
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
    <section class="proposal-workspace" role="dialog" aria-label="Add to calendar">
      <div
        class="proposal-editor"
        tabindex="-1" ref={heading}
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
                  <button onClick={() => void undo()} disabled={busy()}>{list().some(e => e.guests) ? "Undo and cancel invitations" : "Undo"}</button>
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
                    <li class="event-item" classList={{ off: !d.add }} onFocusIn={() => setActive(i())}>
                      <input
                        type="checkbox"
                        class="event-tick"
                        aria-label={`Add ${d.subject || "this event"}`}
                        checked={d.add}
                        onChange={(e) => set(i(), "add", e.currentTarget.checked)}
                      />
                      <div class="event-fields">
                        <Show when={drafts.length > 1}><button class="event-link" aria-pressed={active() === i()}
                          onClick={() => setActive(i())}>Show day</button></Show>
                        <EventFields draft={d} showNotes={!!proposal()?.mail} set={(k, v) => set(i(), k, v)} editing={editing() === i()}
                          toggleRepeat={() => setEditing(editing() === i() ? null : i())} />
                        <Show when={proposal()?.mail}>{mail => <>
                          <Show when={mail().text}><button class="event-link" onClick={() => set(i(), "notes",
                            [d.notes.trim(), mail().text].filter(Boolean).join("\n\n"))}>Use email text as notes</button></Show>
                          <Show when={mail().participants.length}>
                            <label class="event-check"><input type="checkbox" checked={!!guests[i()]?.length}
                              onChange={e => setGuests(i(), e.currentTarget.checked ? mail().participants : [])} />
                              Invite email participants</label>
                            <Show when={guests[i()]?.length}><ul class="event-guests"><For each={guests[i()]}>{addr => <li>{addr}</li>}</For></ul></Show>
                          </Show>
                        </>}</Show>
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
              <Show when={error()}>{(e) => <p class="warn" role="alert">{e()}</p>}</Show>
              <div class="event-actions">
                <span class="spacer" />
                <button onClick={props.onClose} disabled={busy()}>Cancel</button>
                <button
                  class="primary"
                  disabled={busy() || ticked().length === 0 || blocking() !== null || !calendar()}
                  onClick={() => void add()}
                >
                  {busy() ? "Adding…" : drafts.some((d, i) => d.add && guests[i]?.length) ? "Add and send invitations" : ticked().length === 1 ? "Add 1 event" : `Add ${ticked().length} events`}
                </button>
              </div>
            </div>
          </Match>
        </Switch>
      </div>
      <Show when={!failed() && !created() && drafts[active()] && calendar()}>{c =>
        <ProposalSchedule draft={drafts[active()]!} calendar={c()} />}</Show>
    </section>
  );
}
