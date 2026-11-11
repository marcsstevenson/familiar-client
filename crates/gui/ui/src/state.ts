// Window-wide selection, shared by the panes. Each pane lives in its own folder
// (folders/, list/, reading/, compose/) and talks to the others only through
// these signals.
import { createSignal } from "solid-js";
import { type Commands, isCommandError, run } from "./api";
import { refreshViews } from "./list/api";

/** What `fam mail check-spam` gives. */
export type SpamResult = Commands["mail.check-spam"]["output"];

/** The folder shown in the list: a local folder ID, or null for the unified Inbox. */
export const [selectedFolder, setSelectedFolder] = createSignal<number | null>(null);

/** The message open in the reading pane, by local message ID. */
export const [selectedMessage, setSelectedMessage] = createSignal<number | null>(null);

/** The row selected in the list, for actions that need more than its ID. */
export type SelectedRow = { id: number; account: string; is_read: boolean; is_flagged: boolean };
export const [selectedRow, setSelectedRow] = createSignal<SelectedRow | null>(null);

/** A short line in the status bar after an action, cleared after a few seconds. */
export const [notice, setNotice] = createSignal<{ text: string; error: boolean } | null>(null);
let noticeTimer: ReturnType<typeof setTimeout> | undefined;
export function notify(text: string, error = false): void {
  setNotice({ text, error });
  clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => setNotice(null), error ? 8000 : 4000);
}

/** The messages being dragged from the list, for the folder pane to drop on.
 * `account` is null when they come from more than one account (no folder
 * takes them then). */
export const [dragged, setDragged] = createSignal<{ ids: number[]; account: string | null } | null>(null);

/** Messages picked with Ctrl or Shift (or Ctrl+A) in the list, by ID, with
 * each one's account. Two or more make a multiple selection: nothing is open
 * in the reading pane then, which shows what can be done to them all. With
 * fewer the list is back to one open message. */
export const [picked, setPicked] = createSignal<ReadonlyMap<number, string>>(new Map());
export const multiPicked = () => picked().size >= 2;

/** Moves the list selection; set by the list while it is mounted. */
export const listNav = {
  move: (_delta: number): void => {},
  /** Called with messages about to leave the list (deleted, archived, moved).
   * If the open message is one of them, or they are the multiple selection,
   * the list then opens the one that takes their place, or the new last one. */
  leaving: (_ids: readonly number[]): void => {},
  /** Called if the command that was to take them out failed. */
  stayed: (): void => {},
};

/** The multiple selection's IDs and its single account (null when mixed). */
export function pickedMessages(): { ids: number[]; account: string | null } {
  const accounts = new Set(picked().values());
  return { ids: [...picked().keys()], account: accounts.size === 1 ? [...accounts][0]! : null };
}

/** Messages fading out of the list while the command that takes them out
 * (delete, archive, move) runs. The list draws their rows as leaving. */
export const [leavingRows, setLeavingRows] = createSignal<ReadonlySet<number>>(new Set());
const setLeaving = (ids: readonly number[], on: boolean) =>
  setLeavingRows((s) => {
    const next = new Set(s);
    for (const id of ids) {
      if (on) next.add(id);
      else next.delete(id);
    }
    return next;
  });

/** How long a row takes to fade out; matches `.list-row.leaving` in app.css. */
const LEAVE_MS = 140;
const reducedMotion = () => window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;

/** Runs a command that takes messages `ids` out of the list (delete, archive,
 * move) and reports it in the status bar. The rows fade out while the command
 * runs, then the list refreshes at once (the command has changed the store)
 * rather than at its next poll. If the open message or the multiple selection
 * is leaving, the list opens the next one once the command has taken effect. */
export async function removeFromList(ids: number | readonly number[], label: string, job: () => Promise<unknown>): Promise<void> {
  const all = typeof ids === "number" ? [ids] : ids;
  // Already on their way out (a second Delete press before the list moved on).
  const going = all.filter((id) => !leavingRows().has(id));
  if (going.length === 0) return;
  listNav.leaving(going);
  setLeaving(going, true);
  const fade = new Promise((r) => setTimeout(r, reducedMotion() ? 0 : LEAVE_MS));
  try {
    await Promise.all([job(), fade]);
    notify(label);
    await refreshViews();
    // The refresh has dropped the rows by now; if not, they come back until
    // the next poll does.
    setTimeout(() => setLeaving(going, false), 500);
  } catch (e) {
    setLeaving(going, false);
    listNav.stayed();
    notify(isCommandError(e) ? e.message : String(e), true);
  }
}

/** "3 messages" or "1 message". */
export const countLabel = (n: number) => `${n.toLocaleString()} message${n === 1 ? "" : "s"}`;

/** Runs a command on the multiple selection that leaves it in the list (mark
 * read, flag), and reports it. */
export async function onPicked(label: string, job: (ids: number[]) => Promise<unknown>): Promise<void> {
  const { ids } = pickedMessages();
  if (ids.length === 0) return;
  try {
    await job(ids);
    notify(`${label}: ${countLabel(ids.length)}`);
  } catch (e) {
    notify(isCommandError(e) ? e.message : String(e), true);
  }
}

/** Runs a command that takes the multiple selection out of the list. */
export function removePicked(label: string, job: (ids: number[]) => Promise<unknown>): Promise<void> {
  const { ids } = pickedMessages();
  return removeFromList(ids, `${label}: ${countLabel(ids.length)}`, () => job(ids));
}

/** The Move to folder picker, open for these messages. */
export const [movePicker, setMovePicker] = createSignal<{ ids: number[]; accounts: string[] } | null>(null);

/** Each account's processed folder (config.toml `processed_folder`), by
 * account key; filled once at start. Accounts without one get no tick. */
export const processedFolders: Record<string, string> = {};

/** Moves message `id` of `account` to that account's processed folder, then
 * opens the next one as delete does. */
export function processMessage(id: number, account: string): Promise<void> {
  const folder = processedFolders[account];
  return removeFromList(id, `Moved to ${folder}`, () => run("mail.process", { messages: [id] }));
}

/** True while the list shows the Follow up view (messages with a follow-up
 * reminder) instead of a folder. A search still takes precedence. */
export const [followUpView, setFollowUpView] = createSignal(false);

/** The search shown in the list instead of a folder, in `fam mail search` syntax. */
export const [searchQuery, setSearchQuery] = createSignal<string | null>(null);

/** Server-only search results for the current search, when asked for. */
export type OnlineHit = { subject: string; from: string; received_at: string; account: string };
export const [onlineHits, setOnlineHits] = createSignal<OnlineHit[] | null>(null);

/** The message the Add to calendar dialog is open for, with its sender's
 * address (for remembering the calendar used for that sender). */
export const [eventDialog, setEventDialog] = createSignal<{ message: number; from: string } | null>(null);

/** The open message's sender address, set by the reading pane. */
export const [readingFrom, setReadingFrom] = createSignal<string>("");

/** The message the spam and scam check is open for. `saved` is a result to
 * show instead of checking again. */
export const [spamDialog, setSpamDialog] = createSignal<{ message: number; saved?: SpamResult } | null>(null);

/** Spam check results known this session, by message ID: checked in the
 * dialog, or read back from the saved results. */
export const [spamResults, setSpamResults] = createSignal<Record<number, SpamResult | null>>({});
export function rememberSpam(id: number, r: SpamResult | null): void {
  setSpamResults((all) => ({ ...all, [id]: r }));
}

/** [accounts] with spam_check or spam_auto = true, from config.toml. */
export const spamSettings: { accounts: string[] } = { accounts: [] };

/** [ai] from config.toml: the tool, and the accounts whose mail may go to it. */
export const aiSettings: { tool: string | null; accounts: string[] } = { tool: null, accounts: [] };
