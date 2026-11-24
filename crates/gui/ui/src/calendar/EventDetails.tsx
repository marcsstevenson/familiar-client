import { For, Show, createResource, createSignal } from "solid-js";
import type { CalShowOutput, WriteOutput } from "../../../../../bindings/commands";
import { isCommandError, run } from "../api";
import { notify } from "../state";
import { EventMenu, type MenuPoint } from "./EventMenu";
import { EditEvent } from "./EditEvent";
import { dateWords } from "./proposals";
import { nextDate } from "./timeline";

const answers: Record<string, string> = {
  accepted: "Accepted", tentativelyAccepted: "Tentative", declined: "Declined",
  notResponded: "Awaiting reply", none: "No response", organizer: "Organiser",
};

function times(d: CalShowOutput) {
  const e = d.event;
  const first = dateWords(e.start.slice(0, 10));
  if (e.all_day) {
    const last = dateWords(nextDate(e.end, -1));
    return (first === last ? first : first + " – " + last) + " · All day";
  }
  const last = e.start.slice(0, 10) === e.end.slice(0, 10) ? "" : dateWords(e.end.slice(0, 10)) + " ";
  return first + " " + e.start.slice(11, 16) + " – " + last + e.end.slice(11, 16) + " · " + d.display_zone;
}

export function EventDetails(props: {
  menu?: MenuPoint; onOpenDetails: () => void;
  id: number; revision: number; expand?: () => void; onClose: () => void; onChanged: () => void;
}) {
  const [details, { refetch }] = createResource(() => [props.id, props.revision] as const,
    ([event]) => run("cal.show", { event }));
  const live = () => details.error ? undefined : details();
  const [mode, setMode] = createSignal<"view" | "edit" | "delete">("view");
  const [series, setSeries] = createSignal(false);
  const [tell, setTell] = createSignal(true);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const meeting = () => !!live()?.attendees.length;
  const canRespond = () => !live()?.is_organizer && (meeting() || (live()?.event.response && live()?.event.response !== "none"));
  const editLabel = () => live()?.is_organizer ? "Edit event" : "Edit reminder";
  const deleteLabel = () => meeting() ? "Cancel meeting" : "Delete event";
  const writable = () => live()?.can_edit && !live()?.event.cancelled;
  const whole = () => !!live()?.can_act_on_series && series();

  async function write(job: () => Promise<WriteOutput>, text: string, close = false) {
    if (busy() || !writable()) return;
    setBusy(true);
    setError("");
    try {
      const out = await job();
      const failed = out.ops.find(op => op.state === "failed");
      if (failed) throw new Error(failed.error || failed.summary);
      const queued = out.ops.some(op => op.state === "pending" || op.state === "in_flight");
      notify(text + (queued ? " · queued for sync" : ""));
      props.onChanged();
      if (close) props.onClose();
      else {
        setMode("view");
        setSeries(false);
        void refetch();
      }
    } catch (e) {
      setError(isCommandError(e) ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  function answer(response: "accept" | "tentative" | "decline") {
    const text = { accept: "Accepted", tentative: "Answered tentative", decline: "Declined" }[response];
    void write(() => run("cal.rsvp", { event: props.id, response, no_reply: !tell(), series: whole() }), text, response === "decline");
  }
  function back() { setError(""); setMode("view"); }

  function open(mode: "view" | "edit" | "delete") {
    setMode(mode);
    props.onOpenDetails();
  }
  function openSource(message: number) {
    void run("ui.open", { message }).catch(err => setError(isCommandError(err) ? err.message : String(err)));
  }

  const detailsDialog = () => <dialog ref={el => queueMicrotask(() => { if (el.isConnected) el.showModal(); })} role="dialog" aria-label="Event details" class="new-event event-dialog"
    onCancel={e => { e.preventDefault(); if (!busy()) props.onClose(); }}>
    <div class="event-body">
      <Show when={details.error}>{err => {
        const cause = err() instanceof Error ? err().cause : err();
        return <p role="alert" class="warn">{isCommandError(cause) ? cause.message : String(err())}</p>;
      }}</Show>
      <Show when={!details.error && !live()}><p role="status">Loading event…</p></Show>
      <Show when={live()}>{detail => {
        const e = () => detail().event;
        return <>
          <h2>{e().subject || "(No subject)"}</h2>
          <p>{times(detail())}</p>
          <p class="muted">{e().account} · {e().calendar}{detail().can_edit ? "" : " · Read-only"}</p>
          <Show when={mode() === "view" && detail().from_mail}>{message =>
            <button onClick={() => openSource(message())}>Open source email</button>
          }</Show>
          <Show when={mode() !== "edit" || !writable()}>
            <Show when={e().location}><p>{e().location}</p></Show>
            <Show when={e().organizer}><p>Organiser · {e().organizer}</p></Show>
            <Show when={!detail().is_organizer && e().response}><p>Your response · {answers[e().response!] ?? e().response}</p></Show>
            <p>Reminder · {detail().reminder_minutes == null ? "Off" : detail().reminder_minutes === 0 ? "At the start" : detail().reminder_minutes + " minutes before"}</p>
            <Show when={meeting()}>
              <div><strong>Guests</strong><ul class="event-guests"><For each={detail().attendees}>{g =>
                <li>{g.name ? g.name + " <" + g.addr + ">" : g.addr} · {g.kind} · {answers[g.response] ?? g.response}</li>}</For></ul></div>
            </Show>
            <Show when={detail().notes_preview}><div><strong>Notes preview</strong><p class="event-preview">{detail().notes_preview}</p></div></Show>
          </Show>
          <Show when={writable()}>
            <Show when={detail().can_act_on_series}>
              <label>Apply to <select value={whole() ? "series" : "one"} disabled={mode() !== "view" || busy()}
                onChange={ev => setSeries(ev.currentTarget.value === "series")}>
                <option value="one">This occurrence</option><option value="series">Entire series</option>
              </select></label>
            </Show>
            <Show when={mode() === "edit"}>
              <EditEvent detail={detail()} series={whole()} busy={busy()} onCancel={back}
                onSave={patch => void write(() => run("cal.edit", patch), "Changes saved", whole())} />
            </Show>
            <Show when={mode() === "delete" && detail().is_organizer}>
              <p>{meeting() ? "Cancel " : "Delete "}{whole() ? "the entire series" : e().recurring ? "this occurrence" : "this event"}?</p>
              <Show when={meeting()}><p>Cancellation goes to the guests: {detail().attendees.map(g => g.addr || g.name).join(", ")}.</p></Show>
              <div class="event-actions"><button disabled={busy()} onClick={back}>Keep event</button>
                <button disabled={busy()} onClick={() => void write(
                  () => run("cal.delete", { event: props.id, series: whole(), yes: meeting() }),
                  meeting() ? "Meeting cancelled" : "Event deleted", true,
                )}>{meeting() ? "Confirm cancellation" : "Confirm delete"}</button></div>
            </Show>
            <Show when={mode() === "view"}>
              <div class="event-actions"><button disabled={busy()} onClick={() => { setError(""); setMode("edit"); }}>
                {editLabel()}</button>
                <Show when={detail().is_organizer}><button disabled={busy()} onClick={() => { setError(""); setMode("delete"); }}>
                  {deleteLabel()}</button></Show>
              </div>
              <Show when={canRespond()}>
                <label><input type="checkbox" checked={tell()} onChange={ev => setTell(ev.currentTarget.checked)} /> Tell the organiser</label>
                <div class="event-actions"><button disabled={busy()} onClick={() => answer("accept")}>Accept</button>
                  <button disabled={busy()} onClick={() => answer("tentative")}>Tentative</button>
                  <button disabled={busy()} onClick={() => answer("decline")}>Decline</button></div>
              </Show>
            </Show>
          </Show>
        </>;
      }}</Show>
      <Show when={error()}>{msg => <p class="warn" role="alert">{msg()}</p>}</Show>
      <div class="event-actions"><Show when={props.expand && mode() === "view"}><button disabled={busy()} onClick={props.expand}>Open full calendar</button></Show>
        <span class="spacer" /><button disabled={busy()} onClick={props.onClose}>Close</button></div>
    </div>
  </dialog>;

  return <Show when={props.menu} fallback={detailsDialog()}>{point => <EventMenu point={point()} onClose={props.onClose}>
    <button role="menuitem" onClick={() => open("view")}>Open event</button>
    <Show when={writable()}>
      <button role="menuitem" onClick={() => open("edit")}>{editLabel()}…</button>
      <Show when={canRespond()}>
        <For each={["accept", "tentative", "decline"] as const}>{response =>
          <button role="menuitem" onClick={() => { open("view"); answer(response); }}>
            {{ accept: "Accept", tentative: "Tentative", decline: "Decline" }[response]}</button>
        }</For>
      </Show>
      <Show when={live()?.is_organizer}>
        <button role="menuitem" class="danger" onClick={() => open("delete")}>{deleteLabel()}…</button>
      </Show>
    </Show>
    <Show when={live()?.from_mail}>{message =>
      <button role="menuitem" onClick={() => { const id = message(); open("view"); openSource(id); }}>Open source email</button>
    }</Show>
  </EventMenu>}</Show>;
}
