// Window-wide selection, shared by the panes. Each pane lives in its own folder
// (folders/, list/, reading/, compose/) and talks to the others only through
// these signals.
import { createSignal } from "solid-js";

/** The folder shown in the list: a local folder ID, or null for the unified Inbox. */
export const [selectedFolder, setSelectedFolder] = createSignal<number | null>(null);

/** The message open in the reading pane, by local message ID. */
export const [selectedMessage, setSelectedMessage] = createSignal<number | null>(null);
