// The reading pane (WP 3.3): opens whatever message `selectedMessage()` names.
//
// The header shows straight from the store; when the body isn't on this
// device the pane asks again without `local`, which downloads it through
// famd. Bodies arrive sanitised from Rust (crates/gui/src/reading) and are
// shown in a frame where nothing can run. Remote images stay blocked unless
// allowed for this message or always for the sender. Opening a message
// marks it read.
import { For, Show, createEffect, createSignal, on } from "solid-js";
import { run } from "../api";
import { type SpamResult, rememberSpam, selectedMessage, setEventDialog, setReadingFrom, setSpamDialog, spamResults, spamSettings } from "../state";
import { gradeLevel, gradeWords } from "../spam/SpamDialog";
import { resolvedTheme } from "../theme";
import { Attachments } from "./Attachments";
import { CopyButton } from "./CopyButton";
import { type MailFetchOutput, type ReadView, errorText, openLink, openMessage, trustSender } from "./api";
import { mismatch } from "./links";
import { type HoverLink, MessageFrame } from "./MessageFrame";
import "./reading.css";

type Addr = MailFetchOutput["from"];

const who = (a: Addr) => (a.name ? `${a.name} <${a.addr}>` : a.addr);

function when(rfc3339: string): string {
  const d = new Date(rfc3339);
  return Number.isNaN(d.getTime())
    ? rfc3339
    : d.toLocaleString(undefined, { dateStyle: "full", timeStyle: "short" });
}

export function ReadingPane() {
  const [view, setView] = createSignal<ReadView | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [loading, setLoading] = createSignal(false);
  const [hover, setHover] = createSignal<HoverLink>(null);
  const [status, setStatus] = createSignal<string | null>(null);
  // In the dark theme, messages show dark unless switched to light, one at a time.
  const [light, setLight] = createSignal(false);
  const dark = () => resolvedTheme() === "dark" && !light();
  let pane!: HTMLElement;
  // Answers for a message no longer selected are dropped.
  let generation = 0;
  let openedAt = 0;

  const load = async (id: number, showImages: boolean) => {
    const g = ++generation;
    setError(null);
    setStatus(null);
    try {
      let v = await openMessage(id, true, showImages);
      if (g !== generation) return;
      setView(v);
      setReadingFrom(v.message.from.addr);
      // Opening a message reads it, as in Outlook.
      if (!v.message.is_read) {
        run("mail.read", { messages: [id] }).catch((e) =>
          setStatus(`Couldn't mark it read: ${errorText(e)}`),
        );
      }
      if (v.html === null || v.inline_pending) {
        setLoading(true);
        v = await openMessage(id, false, showImages);
        if (g !== generation) return;
        setView(v);
      }
    } catch (e) {
      if (g === generation) setError(errorText(e));
    } finally {
      if (g === generation) setLoading(false);
    }
  };

  createEffect(
    on(selectedMessage, (id) => {
      setView(null);
      setHover(null);
      setLight(false);
      setReadingFrom("");
      if (id === null) {
        generation++;
        return;
      }
      openedAt = performance.now();
      void load(id, false);
    }),
  );

  const showImages = () => {
    const id = selectedMessage();
    if (id !== null) void load(id, true);
  };

  const setTrusted = async (addr: string, trusted: boolean) => {
    try {
      await trustSender(addr, trusted);
      const id = selectedMessage();
      if (id !== null) void load(id, false);
    } catch (e) {
      setStatus(`Couldn't save: ${errorText(e)}`);
    }
  };

  const open = (href: string) => {
    openLink(href).catch((e) => setStatus(`Couldn't open the link: ${errorText(e)}`));
  };

  // How long opening took, for the performance checks (WP 3.7).
  // A spam check saved for the open message (spam_check and spam_auto
  // accounts only). Read on every open: famd's background check can save
  // one at any time, and it's a small local file.
  createEffect(
    on(view, (v) => {
      const m = v?.message;
      if (!m || !spamSettings.accounts.includes(m.account)) return;
      run("mail.check-spam", { message: m.id, saved: true }).then(
        (r) => rememberSpam(m.id, r),
        () => rememberSpam(m.id, null),
      );
    }),
  );
  const spam = () => {
    const m = view()?.message;
    return m ? (spamResults()[m.id] ?? null) : null;
  };
  // The result stands out only when it disagrees with where the message is:
  // 50% or more outside Junk, or under 50% in Junk. Otherwise it shows only
  // while the pointer is over the header.
  const spamStandsOut = (r: SpamResult) => {
    if (!r.ai) return false;
    return Math.max(r.ai.spam, r.ai.scam) >= 50 !== (r.folder === "junkemail");
  };

  const loaded = () => {
    pane.dataset.openMs = (performance.now() - openedAt).toFixed(1);
  };

  return (
    <section class="pane reading" aria-label="Reading pane" ref={pane}>
      <Show
        when={view()}
        fallback={
          <div class="pane-empty">
            <Show when={!error() && selectedMessage() === null} fallback={error() ?? ""}>
              <span class="reading-empty-kea" aria-hidden="true" />
            </Show>
          </div>
        }
      >
        {(v) => {
          const m = () => v().message;
          return (
            <div class="reading-message">
              <header class="reading-header">
                <div class="reading-title">
                  <h1 class="reading-subject reading-copyable">
                    {m().subject || "(no subject)"}
                    <Show when={m().subject}>
                      <CopyButton text={() => m().subject} label="Copy subject" />
                    </Show>
                  </h1>
                  <button
                    class="reading-calendar"
                    title="Add the events in this email to a calendar (Ctrl+Shift+A)"
                    onClick={() => setEventDialog({ message: m().id, from: m().from.addr })}
                  >
                    Add to calendar
                  </button>
                  <Show when={resolvedTheme() === "dark" && v().html}>
                    <button
                      class="reading-mode"
                      aria-pressed={light()}
                      title={light() ? "Show this message dark" : "Show this message light"}
                      aria-label={light() ? "Show this message dark" : "Show this message light"}
                      onClick={() => setLight(!light())}
                    >
                      {light() ? "☾" : "☀"}
                    </button>
                  </Show>
                </div>
                <div class="reading-from reading-copyable">
                  {who(m().from)}
                  <Show when={m().from.addr}>
                    <CopyButton text={() => m().from.addr} label="Copy sender address" />
                  </Show>
                </div>
                <div class="reading-meta">
                  <Show when={m().to.length > 0}>
                    <span class="reading-copyable">
                      To: <For each={m().to}>{(a, i) => <>{i() > 0 ? ", " : ""}{who(a)}</>}</For>
                      <CopyButton
                        text={() => m().to.map((a) => a.addr).join(", ")}
                        label={m().to.length > 1 ? "Copy recipient addresses" : "Copy recipient address"}
                      />
                    </span>
                  </Show>
                  <Show when={m().cc.length > 0}>
                    <span>
                      Cc: <For each={m().cc}>{(a, i) => <>{i() > 0 ? ", " : ""}{who(a)}</>}</For>
                    </span>
                  </Show>
                  <span class="muted">{when(m().received_at)}</span>
                  <span class="muted">{m().account} / {m().folder}</span>
                </div>
                {/* Shown only while the pointer is over the header (or it has
                    focus), so it takes no room otherwise. */}
                <Show when={spam() && !spamStandsOut(spam()!) ? spam() : null}>
                  {(r) => (
                    <div class="reading-trusted" role="note">
                      <span class={`reading-spam spam-${gradeLevel(r())}`}>Checked: {gradeWords(r())}</span>
                      <button onClick={() => setSpamDialog({ message: m().id, saved: r() })}>Details</button>
                    </div>
                  )}
                </Show>
                <Show when={v().sender_trusted}>
                  <div class="reading-trusted" role="note">
                    <span>Images from {m().from.addr} are always shown.</span>
                    <button onClick={() => setTrusted(m().from.addr, false)}>Stop</button>
                  </div>
                </Show>
              </header>
              <Show when={v().blocked_images > 0 && !v().images_shown}>
                <div class="reading-banner" role="note">
                  <span>
                    {v().blocked_images === 1
                      ? "1 remote image was blocked to protect your privacy."
                      : `${v().blocked_images} remote images were blocked to protect your privacy.`}
                  </span>
                  <button onClick={showImages}>Show images</button>
                  <Show when={m().from.addr}>
                    <button onClick={() => setTrusted(m().from.addr, true)}>
                      Always show from {m().from.addr}
                    </button>
                  </Show>
                </div>
              </Show>
              <Show when={spam() && spamStandsOut(spam()!) ? spam() : null}>
                {(r) => (
                  <div class="reading-banner" role="note">
                    <span class={`reading-spam spam-${gradeLevel(r())}`}>Checked: {gradeWords(r())}</span>
                    <button onClick={() => setSpamDialog({ message: m().id, saved: r() })}>Details</button>
                  </div>
                )}
              </Show>
              <Show when={m().has_attachments}>
                <Attachments message={m().id} onStatus={setStatus} />
              </Show>
              <Show when={error()}>
                <div class="reading-banner warn" role="alert">{error()}</div>
              </Show>
              <Show
                when={v().html}
                fallback={
                  <div class="reading-wait">{loading() ? "Downloading the message…" : "The body isn't on this device."}</div>
                }
              >
                {(html) => <MessageFrame html={html()} dark={dark()} onHover={setHover} onOpen={open} onLoad={loaded} />}
              </Show>
              <Show when={hover() ?? status()}>
                <div class="reading-status" role="status">
                  <Show when={hover()} fallback={status()}>
                    {(h) => {
                      const claimed = () => mismatch(h().href, h().text);
                      return (
                        <>
                          <Show when={claimed()}>
                            <strong class="warn">Not {claimed()}: </strong>
                          </Show>
                          {h().href}
                        </>
                      );
                    }}
                  </Show>
                </div>
              </Show>
            </div>
          );
        }}
      </Show>
    </section>
  );
}
