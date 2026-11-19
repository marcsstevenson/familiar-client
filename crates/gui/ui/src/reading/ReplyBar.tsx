// The icon toolbar in the reading pane's header: Reply, Reply all, Forward,
// and "..." for the message menu (the same menu as a right click on the row
// in the list, with "Add to calendar…" first).
import { Show, createSignal } from "solid-js";
import { openCompose } from "../compose/spec";
import { type Row, listIndexOf, listRows } from "../list/api";
import { MessageMenu } from "../list/MessageMenu";
import { setEventDialog } from "../state";
import type { MailFetchOutput } from "./api";
import { replyIcon } from "./replyIcon";
import "./readingActions.css";

/** The list's row for the message, or one made from its header when the
 * message isn't in the open list (snooze and reminder state then unknown). */
async function rowFor(m: MailFetchOutput): Promise<Row> {
  const i = await listIndexOf(m.id).catch(() => null);
  if (i !== null) {
    const [row] = await listRows(i, 1).catch(() => [] as Row[]);
    if (row && row.id === m.id) return row;
  }
  const folder = m.folder.toLowerCase().replace(/\s+/g, "");
  return {
    id: m.id,
    account: m.account,
    from: m.from.name || m.from.addr,
    subject: m.subject,
    preview: m.preview,
    received_at: Math.floor(new Date(m.received_at).getTime() / 1000),
    is_read: m.is_read,
    is_flagged: m.is_flagged,
    has_attachments: m.has_attachments,
    importance: m.importance === "high" ? 2 : m.importance === "low" ? 0 : 1,
    in_junk: folder === "junkemail" || folder === "junk",
    spam_warning: null,
    snoozed_until: null,
    follow_up_due: null,
  };
}

export function ReplyBar(props: { message: MailFetchOutput; standalone?: boolean }) {
  const [menu, setMenu] = createSignal<{ row: Row; x: number; y: number } | null>(null);
  let more!: HTMLButtonElement;

  const openMenu = async () => {
    const row = await rowFor(props.message);
    const r = more.getBoundingClientRect();
    // Right-aligned under the button; MessageMenu keeps it inside the window.
    setMenu({ row, x: r.right - 220, y: r.bottom + 2 });
  };

  return (
    <div class="reading-actions" role="toolbar" aria-label="Message actions">
      <button
        type="button"
        title="Reply (R)"
        aria-label="Reply"
        onClick={() => void openCompose({ kind: "reply", message: props.message.id })}
      >
        {replyIcon("reply")}
      </button>
      <button
        type="button"
        title="Reply all (A)"
        aria-label="Reply all"
        onClick={() => void openCompose({ kind: "reply", message: props.message.id, all: true })}
      >
        {replyIcon("replyAll")}
      </button>
      <button
        type="button"
        title="Forward (F)"
        aria-label="Forward"
        onClick={() => void openCompose({ kind: "forward", message: props.message.id })}
      >
        {replyIcon("forward")}
      </button>
      <Show when={!props.standalone}>
        <button
          type="button"
          ref={more}
          title="More actions"
          aria-label="More actions"
          aria-haspopup="menu"
          aria-expanded={menu() !== null}
          onClick={() => void openMenu()}
        >
          <svg viewBox="0 0 16 16" aria-hidden="true" class="filled">
            <circle cx="3.5" cy="8" r="1.25" />
            <circle cx="8" cy="8" r="1.25" />
            <circle cx="12.5" cy="8" r="1.25" />
          </svg>
        </button>
        <Show when={menu()}>
          {(m) => (
            <MessageMenu
              row={m().row}
              x={m().x}
              y={m().y}
              onAddToCalendar={() => setEventDialog({ message: props.message.id, from: props.message.from.addr })}
              onClose={() => {
                setMenu(null);
                more?.focus();
              }}
            />
          )}
        </Show>
      </Show>
    </div>
  );
}
