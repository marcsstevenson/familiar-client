// IPC for the virtualised message list (crates/gui/src/list.rs).
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type Sort = "newest" | "oldest" | "from" | "subject";

export type ViewSpec = {
  /** Local folder IDs; empty for the unified Inbox. */
  folders: number[];
  unread_only: boolean;
  sort: Sort;
  /** A search instead of folders. */
  query: string | null;
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
