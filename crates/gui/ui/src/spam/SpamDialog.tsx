// Check for spam or scam (AI spam plan, WP S.3): runs `fam mail check-spam`
// and shows the AI's grade, its reasons and the facts Familiar found, with
// the same reports as the message menu (`fam mail report`). A saved result
// can be shown without checking again.
import { For, Match, Show, Switch, createResource, createSignal } from "solid-js";
import { isCommandError, run } from "../api";
import { toolWords } from "../calendar/proposals";
import { type SpamResult, aiSettings, rememberSpam, removeFromList } from "../state";
import "../calendar/calendar.css";
import "./spam.css";

const errorText = (e: unknown) => (isCommandError(e) ? e.message : String(e));

/** "Likely scam · 92%", or "Not graded" without an AI answer. */
export function gradeWords(r: SpamResult): string {
  const g = r.ai;
  return g ? `${g.headline} · ${Math.max(g.spam, g.scam)}%` : "Not graded";
}

/** `bad`, `careful` or `fine`, for colours. */
export function gradeLevel(r: SpamResult): string {
  const top = r.ai ? Math.max(r.ai.spam, r.ai.scam) : 0;
  return top >= 70 ? "bad" : top >= 30 ? "careful" : "fine";
}

const checkedWords = (at: number) =>
  new Date(at * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });

export function SpamDialog(props: { message: number; saved?: SpamResult; onClose: () => void }) {
  // Read once: the props go stale when the dialog closes, and the check and
  // the reports can outlive it.
  const message = props.message;
  // Bumped by "Check again". A saved result is read again so the folder (and
  // so the report buttons) is where the message is now.
  const [round, setRound] = createSignal(props.saved ? 0 : 1);
  const [result] = createResource(round, async (n) => {
    if (n === 0) return run("mail.check-spam", { message, saved: true });
    const r = await run("mail.check-spam", { message });
    if (r.ai) rememberSpam(message, r);
    return r;
  });

  function report(label: string, input: { not_junk?: boolean; phishing?: boolean }) {
    props.onClose();
    void removeFromList(message, label, () => run("mail.report", { messages: [message], ...input }));
  }

  const marks: Record<string, string> = { scam: "!!", spam: "!", safe: "✓" };

  return (
    <div class="palette-backdrop" onMouseDown={(e) => e.target === e.currentTarget && props.onClose()}>
      <div class="palette event-dialog spam-dialog" role="dialog" aria-label="Spam and scam check">
        <Switch>
          <Match when={result.error}>
            <div class="event-body">
              <h2>Spam and scam check</h2>
              <p class="warn" role="alert">{errorText(result.error)}</p>
              <div class="event-actions">
                <button onClick={props.onClose}>Close</button>
              </div>
            </div>
          </Match>
          <Match when={result.loading || !result()}>
            <div class="event-body">
              <h2>Spam and scam check</h2>
              <p class="event-wait" role="status">
                <span class="spinner" aria-hidden="true" /> Checking this email…
              </p>
              <Show when={aiSettings.tool}>
                <p class="muted">
                  The email and what Familiar found go to {toolWords(aiSettings.tool)}, which can take 10 to 20 seconds.
                  Closing this leaves the check to finish on its own; its result is kept.
                </p>
              </Show>
              <div class="event-actions">
                <button onClick={props.onClose}>Close</button>
              </div>
            </div>
          </Match>
          <Match when={result()}>
            {(r) => (
              <div class="event-body">
                <div class="spam-head">
                  <h2 class={`spam-grade spam-${gradeLevel(r())}`}>{gradeWords(r())}</h2>
                  <Show when={r().ai}>
                    {(g) => (
                      <span class="muted">
                        Spam {g().spam}% · Scam {g().scam}% · checked by {g().tool === "codex" ? "Codex" : "Claude"},{" "}
                        {checkedWords(r().checked_at)}
                      </span>
                    )}
                  </Show>
                </div>
                <Show when={r().ai}>
                  {(g) => (
                    <>
                      <Show when={g().summary}>
                        <p>{g().summary}</p>
                      </Show>
                      <Show when={g().reasons.length > 0}>
                        <dl class="spam-reasons">
                          <For each={g().reasons}>
                            {(reason) => (
                              <>
                                <dt>{reason.signal}</dt>
                                <dd>“{reason.evidence}”</dd>
                              </>
                            )}
                          </For>
                        </dl>
                      </Show>
                      <Show when={g().advice}>{(a) => <p class="spam-advice">{a()}</p>}</Show>
                    </>
                  )}
                </Show>
                <section class="spam-facts" aria-label="What the mail server and your mailbox say">
                  <h3>What the mail server and your mailbox say</h3>
                  <ul>
                    <For each={r().facts}>
                      {(f) => (
                        <li class={f.points ? `spam-fact-${f.points}` : undefined}>
                          <span class="spam-mark" aria-hidden="true">
                            {marks[f.points] ?? ""}
                          </span>
                          {f.detail}
                        </li>
                      )}
                    </For>
                  </ul>
                </section>
                <For each={r().notes}>{(n) => <p class="muted">{n}</p>}</For>
                <div class="event-actions">
                  <button onClick={props.onClose}>Close</button>
                  <Show when={round() === 0}>
                    <button onClick={() => setRound(1)}>Check again</button>
                  </Show>
                  <span class="spacer" />
                  <Show
                    when={r().folder === "junkemail"}
                    fallback={
                      <button
                        title="Tell your mail provider this is spam, and move it to Junk Email"
                        onClick={() => report("Reported as junk", {})}
                      >
                        Report junk
                      </button>
                    }
                  >
                    <button
                      title="Tell your mail provider this isn't spam, and move it to the Inbox"
                      onClick={() => report("Reported as not junk", { not_junk: true })}
                    >
                      Not junk
                    </button>
                  </Show>
                  <button
                    class="danger"
                    title="Tell your mail provider this is a scam, and move it out of the way"
                    onClick={() => report("Reported as phishing", { phishing: true })}
                  >
                    Report phishing
                  </button>
                </div>
              </div>
            )}
          </Match>
        </Switch>
      </div>
    </div>
  );
}
