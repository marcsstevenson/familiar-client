// IPC for the virtualised message list (crates/gui/src/list.rs).
import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";

export type Sort = "newest" | "oldest" | "from" | "subject";

export type ViewSpec = {
  /** Local folder IDs; empty for the unified Inbox. */
  folders: number[];
  unread_only: boolean;
  sort: Sort;
  /** A search instead of folders. */
  query: string | null;
  /** The Follow up view instead of folders. */
  followups?: boolean;
};

export type Row = {
  id: number;
  account: string;
  from: string;
  subject: string;
  preview: string;
  /** Unix seconds. */
  received_at: number;
  is_read: boolean;
  is_flagged: boolean;
  has_attachments: boolean;
  importance: number;
  /** In its account's Junk Email folder. */
  in_junk: boolean;
  /** "Likely scam · 92%" when a saved spam check is 50%+ outside Junk Email. */
  spam_warning: string | null;
  /** When its follow-up reminder is due (Unix seconds), if it has one. */
  follow_up_due: number | null;
};

// Each open is numbered, so Rust can ignore an older open that takes its lock late.
let generation = 0;
export type Opened = { total: number; notices: string[]; capped: boolean };
export const listOpen = (spec: ViewSpec) =>
  invoke<Opened>("list_open", { spec: { ...spec, generation: ++generation } });
export const listRows = (start: number, count: number) =>
  invoke<Row[]>("list_rows", { start, count });
export const listIndexOf = (id: number) => invoke<number | null>("list_index_of", { id });

/** Calls `f` whenever the store changes (sync, or a write from anywhere). */
export const onStoreChanged = (f: () => void) => listen("store_changed", () => f());

/** Tells the panes to refresh after a change outside the store, such as a
 * follow-up reminder (kept in followups.json, which `store_changed` doesn't watch). */
export const refreshViews = () => emit("store_changed");
