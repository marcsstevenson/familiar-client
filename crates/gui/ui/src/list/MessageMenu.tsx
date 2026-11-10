// The message context menu: the everyday actions plus reporting junk and
// phishing. Each item runs the same registry command as `fam` (`fam mail
// report` for the reports), so it goes through the op queue and works offline.
import { For, Show, createSignal, onMount } from "solid-js";
import { isCommandError, run } from "../api";
import { notify, removeFromList, setSpamDialog, spamSettings } from "../state";
import type { Row } from "./api";
import { snoozeChoices, snoozeLabel } from "./format";

export function MessageMenu(props: { row: Row; x: number; y: number; onClose: () => void }) {
  // A copy: the menu closes (and its props go stale) before a command runs.
  const row = { ...props.row };
  const ids = [row.id];
  let box!: HTMLElement;

  onMount(() => {
    // Keep the menu inside the window.
    const r = box.getBoundingClientRect();
    box.style.left = `${Math.max(4, Math.min(props.x, window.innerWidth - r.width - 4))}px`;
    box.style.top = `${Math.max(4, Math.min(props.y, window.innerHeight - r.height - 4))}px`;
    box.querySelector<HTMLElement>("button")?.focus();
  });

  async function act(label: string, job: () => Promise<unknown>) {
    props.onClose();
    try {
      await job();
      notify(label);
    } catch (e) {
      notify(isCommandError(e) ? e.message : String(e), true);
    }
  }

  /** For an action that takes the message out of the list: opens the next one. */
  function leave(label: string, job: () => Promise<unknown>) {
    props.onClose();
    void removeFromList(row.id, label, job);
  }

  // The snooze choices open in place of the menu's lower part.
  const [snoozing, setSnoozing] = createSignal(false);
  const [when, setWhen] = createSignal("");

  function snooze(until: string, at?: string) {
    if (!until.trim()) return;
    leave(at ? `Snoozed until ${at}` : "Snoozed", () => run("mail.snooze", { messages: ids, until }));
  }

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      props.onClose();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const items = [...box.querySelectorAll<HTMLElement>("[role=menuitem]")];
      const i = items.indexOf(document.activeElement as HTMLElement);
      const next = (i + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
      items[next]?.focus();
    }
  };

  return (
    <div
      class="folder-menu-backdrop"
      role="dialog"
      aria-label={`Message ${row.subject || "(no subject)"}`}
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) props.onClose();
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        if (e.target === e.currentTarget) props.onClose();
      }}
      onKeyDown={onKeyDown}
    >
      <div class="folder-menu" role="menu" ref={(el) => (box = el)} style={{ left: `${props.x}px`, top: `${props.y}px` }}>
        <button
          role="menuitem"
          onClick={() =>
            act(row.is_read ? "Marked unread" : "Marked read", () =>
              run("mail.read", { messages: ids, unread: row.is_read }),
            )
          }
        >
          {row.is_read ? "Mark unread" : "Mark read"}
        </button>
        <button
          role="menuitem"
          onClick={() =>
            act(row.is_flagged ? "Flag cleared" : "Flagged", () =>
              run("mail.flag", { messages: ids, clear: row.is_flagged }),
            )
          }
        >
          {row.is_flagged ? "Clear flag" : "Flag"}
        </button>
        <button role="menuitem" onClick={() => leave("Archived", () => run("mail.archive", { messages: ids }))}>
          Archive
        </button>
        <button role="menuitem" onClick={() => leave("Moved to Deleted Items", () => run("mail.delete", { messages: ids }))}>
          Delete
        </button>
        <Show when={row.snoozed_until}>
          {(t) => (
            <button
              role="menuitem"
              title={`Snoozed until ${snoozeLabel(t())}; bring it back to where it was now`}
              onClick={() => leave("Unsnoozed", () => run("mail.unsnooze", { messages: ids }))}
            >
              Unsnooze
            </button>
          )}
        </Show>
        <button
          role="menuitem"
          aria-expanded={snoozing()}
          title="Move it to Snoozed; it comes back, unread, at the time you pick (while Familiar is running)"
          onClick={() => {
            setSnoozing(!snoozing());
            box.querySelector<HTMLElement>(".snooze-choices button")?.focus();
          }}
        >
          {row.snoozed_until ? "Snooze until another time…" : "Snooze…"}
        </button>
        <Show when={snoozing()}>
          <div class="snooze-choices">
            <For each={snoozeChoices()}>
              {(c) => (
                <button role="menuitem" onClick={() => snooze(c.until, c.at)}>
                  <span>{c.label}</span>
                  <span class="snooze-at">{c.at}</span>
                </button>
              )}
            </For>
            <form
              onSubmit={(e) => {
                e.preventDefault();
                snooze(when());
              }}
            >
              <input
                type="text"
                aria-label="Snooze until"
                placeholder="fri 2pm, 3h, 21 nov 8am"
                value={when()}
                onInput={(e) => setWhen(e.currentTarget.value)}
                onKeyDown={(e) => e.stopPropagation()}
              />
            </form>
          </div>
        </Show>
        <hr class="menu-rule" />
        <Show when={spamSettings.accounts.includes(row.account)}>
          <button
            role="menuitem"
            title="See how likely this is to be spam or a scam, from the mail server's checks and the AI tool"
            onClick={() => {
              props.onClose();
              setSpamDialog({ message: row.id });
            }}
          >
            Check for spam or scam
          </button>
        </Show>
        <Show
          when={row.in_junk}
          fallback={
            <button
              role="menuitem"
              title="Tell your mail provider this is spam, and move it to Junk Email"
              onClick={() => leave("Reported as junk", () => run("mail.report", { messages: ids }))}
            >
              Report junk
            </button>
          }
        >
          <button
            role="menuitem"
            title="Tell your mail provider this isn't spam, and move it to the Inbox"
            onClick={() => leave("Reported as not junk", () => run("mail.report", { messages: ids, not_junk: true }))}
          >
            Not junk
          </button>
        </Show>
        <button
          role="menuitem"
          class="danger"
          title="Tell your mail provider this is a scam; Outlook then deletes it"
          onClick={() => leave("Reported as phishing", () => run("mail.report", { messages: ids, phishing: true }))}
        >
          Report phishing (scam)
        </button>
      </div>
    </div>
  );
}
