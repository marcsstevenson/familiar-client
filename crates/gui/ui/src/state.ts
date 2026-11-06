// Window-wide selection, shared by the panes. Each pane lives in its own folder
// (folders/, list/, reading/, compose/) and talks to the others only through
// these signals.
import { createSignal } from "solid-js";

/** The folder shown in the list: a local folder ID, or null for the unified Inbox. */
export const [selectedFolder, setSelectedFolder] = createSignal<number | null>(null);

/** The message open in the reading pane, by local message ID. */
export const [selectedMessage, setSelectedMessage] = createSignal<number | null>(null);

/** The row selected in the list, for actions that need more than its ID. */
export type SelectedRow = { id: number; is_read: boolean; is_flagged: boolean };
export const [selectedRow, setSelectedRow] = createSignal<SelectedRow | null>(null);

/** A short line in the status bar after an action, cleared after a few seconds. */
export const [notice, setNotice] = createSignal<{ text: string; error: boolean } | null>(null);
let noticeTimer: ReturnType<typeof setTimeout> | undefined;
export function notify(text: string, error = false): void {
  setNotice({ text, error });
  clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => setNotice(null), error ? 8000 : 4000);
}

/** The message being dragged from the list, for the folder pane to drop on. */
export const [dragged, setDragged] = createSignal<{ id: number; account: string } | null>(null);

/** Moves the list selection; set by the list while it is mounted. */
export const listNav = {
  move: (_delta: number): void => {},
  /** A message about to leave the list (deleted, archived, moved). If it is the
   * one open, the list then opens the next one down, or up at the end. */
  leaving: null as number | null,
};

/** The search shown in the list instead of a folder, in `fam mail search` syntax. */
export const [searchQuery, setSearchQuery] = createSignal<string | null>(null);

/** Server-only search results for the current search, when asked for. */
export type OnlineHit = { subject: string; from: string; received_at: string; account: string };
export const [onlineHits, setOnlineHits] = createSignal<OnlineHit[] | null>(null);
