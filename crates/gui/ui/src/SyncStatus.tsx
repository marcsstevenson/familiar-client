import { invoke } from "@tauri-apps/api/core";
import { For, Show, createSignal, onCleanup, onMount } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { Portal } from "solid-js/web";
import type { AccountSyncStatus, SyncPhase, SyncStatusOutput } from "../../../../bindings/commands";
import { type DaemonState, isCommandError, run } from "./api";
import "./styles/sync-status.css";

const phases: Record<SyncPhase, string> = {
  starting: "Starting", folder_tree: "Updating folders", headers: "Downloading messages",
  bodies: "Downloading message bodies", calendar: "Updating calendar", idle: "Up to date",
  backoff: "Waiting to retry", auth_required: "Sign-in problem", stopped: "Sync stopped",
};

function problem(a: AccountSyncStatus): string | undefined {
  if (a.phase === "stopped") return phases.stopped;
  if (a.detail && /can.t reach|network error|connection reset/i.test(a.detail)) return "Connection problem";
  if (a.phase === "backoff" || a.phase === "auth_required") return phases[a.phase];
  if (a.errors.length) return "Folder sync failed";
  if (a.detail) return "Sync problem";
}

/** Delta links in provider errors can include very long, opaque mailbox cursors. */
function readable(message: string) {
  return message.replace(/(https?:\/\/[^\s?]+)\?[^\s)]+/gi, "$1");
}

function errorText(e: unknown) {
  return readable(isCommandError(e) ? e.message : String(e));
}

function when(value?: string | null) {
  return value ? new Date(value).toLocaleString() : "-";
}

export function SyncStatus(props: { daemon: DaemonState }) {
  const [data, setData] = createStore<{ snapshot?: SyncStatusOutput }>({});
  const status = () => data.snapshot;
  const [readError, setReadError] = createSignal("");
  const [open, setOpen] = createSignal(false);
  const [sending, setSending] = createSignal(false);
  const [feedback, setFeedback] = createSignal("");
  let reading = false;

  async function refresh() {
    if (reading) return;
    reading = true;
    try {
      setData("snapshot", reconcile(await run("sync.status", {}), { key: "account" }));
      setReadError("");
    } catch (e) {
      setReadError(errorText(e));
    } finally {
      reading = false;
    }
  }
  onMount(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), 10000);
    onCleanup(() => clearInterval(timer));
  });

  const unavailable = () => !!readError() || !status()?.daemon.running;
  const affected = () => status()?.accounts.filter(a => problem(a)) ?? [];
  function summary() {
    const s = status();
    if (readError()) return "Sync status unavailable";
    if (!s || !s.daemon.running) {
      if (props.daemon.state === "starting") return "Starting sync.";
      return s || props.daemon.state === "unavailable" ? "Sync service unavailable" : "Checking sync.";
    }
    const [first, ...others] = affected();
    if (first) {
      const retryAt = first.next_poll_at && (first.phase === "auth_required" || first.phase === "backoff")
        ? ` � retry at ${new Date(first.next_poll_at).toLocaleTimeString()}` : "";
      return `${first.account}: ${problem(first)}${others.length ? ` (+${others.length} more)` : retryAt}`;
    }
    if (!s.accounts.length) return "Waiting for accounts";
    return s.accounts.every(a => a.phase === "idle" && a.first_sync_done) ? "Mail up to date" : "Syncing.";
  }
  async function retry(account?: string) {
    if (sending()) return;
    setSending(true);
    setFeedback("");
    try {
      await invoke("retry_sync", { account: account ?? null });
      setFeedback(account ? `Retry requested for ${account}.` : "Retry requested for all accounts.");
    } catch (e) {
      setFeedback(`Retry failed: ${errorText(e)}`);
    } finally {
      await refresh();
      setSending(false);
    }
  }

  let trigger!: HTMLButtonElement;
  return <>
    <button ref={trigger} class="statusbar-button sync-trigger" classList={{ warn: !!readError() || !!status() && unavailable() || affected().length > 0 }}
      aria-haspopup="dialog" aria-expanded={open()} title="Show sync details"
      onClick={() => { setFeedback(""); setOpen(true); void refresh(); }}>{summary()}</button>
    <Show when={open()}><Portal>
      <dialog class="sync-details" role="dialog" aria-modal="true" aria-labelledby="sync-details-title"
        ref={el => queueMicrotask(() => { if (el.isConnected) el.showModal(); })}
        onClose={() => { setOpen(false); trigger.focus(); }}>
        <header><h2 id="sync-details-title">Sync status</h2>
          <button autofocus onClick={e => e.currentTarget.closest("dialog")?.close()}>Close</button></header>
        <Show when={readError()}>{message => <p class="warn">Status unavailable: {message()}</p>}</Show>
        <Show when={status() && !status()!.daemon.running || props.daemon.state === "unavailable"}>
          <p class="warn">{props.daemon.state === "unavailable" ? props.daemon.message : "The sync service is unavailable."}</p>
        </Show>
        <Show when={readError() && status()}><p>Showing the last available account details.</p></Show>
        <For each={status()?.accounts} fallback={<p>{status() ? "No accounts have synced on this device." : summary()}</p>}>
          {a => <section><h3>{a.email || a.account}</h3>
            <p classList={{ warn: !!problem(a) }}>{problem(a) ?? phases[a.phase]}</p>
            <Show when={a.detail}>{msg => <p class="sync-error">{readable(msg())}</p>}</Show>
            <dl><dt>Sync phase</dt><dd>{phases[a.phase]}</dd>
              <dt>Last folder sync</dt><dd>{when(a.last_synced_at)}</dd>
              <dt>{a.phase === "backoff" || a.phase === "auth_required" ? "Next retry" : "Next poll"}</dt><dd>{when(a.next_poll_at)}</dd>
              <dt>Local mail</dt><dd>{a.messages.toLocaleString()} messages in {a.folders} folders</dd>
              <dt>Bodies awaiting download</dt><dd>{a.bodies_missing.toLocaleString()}</dd>
            </dl>
            <Show when={a.errors.length}><details><summary>Folder errors ({a.errors.length})</summary>
              <ul><For each={a.errors}>{err => <li>{err.resource} - {readable(err.message)}</li>}</For></ul>
            </details></Show>
            <button disabled={sending() || unavailable()} aria-label={`Retry sync for ${a.email || a.account}`} onClick={() => void retry(a.account)}>Retry now</button>
          </section>}
        </For>
        <footer><button disabled={sending() || unavailable() || !status()?.accounts.length} onClick={() => void retry()}>Retry all accounts</button>
          <Show when={unavailable()}><button onClick={() => void refresh()}>Check status again</button></Show>
          <span role="status">{feedback()}</span></footer>
      </dialog>
    </Portal></Show>
  </>;
}
