// What a compose window writes, and how other panes open one. A window gets
// its spec in the URL (`#compose=<json>`), so each draft has a window of its own.
import { invoke } from "@tauri-apps/api/core";

export type ComposeSpec =
  /** A new message, optionally from a given account and to given people. */
  | { kind: "new"; account?: string; to?: string[] }
  /** A reply (or reply all) to a message, by local message ID. */
  | { kind: "reply"; message: number; all?: boolean }
  /** A forward of a message. */
  | { kind: "forward"; message: number }
  /** An existing draft, by local message ID (made here, by `fam`, or by Outlook). */
  | { kind: "draft"; draft: number };

/** Opens a compose window. */
export function openCompose(spec: ComposeSpec): Promise<void> {
  return invoke("open_compose", { spec });
}

/** The spec this window was opened with, if it is a compose window. */
export function composeSpec(): ComposeSpec | null {
  const m = /^#compose=(.*)$/.exec(window.location.hash);
  if (!m?.[1]) return null;
  try {
    return JSON.parse(decodeURIComponent(m[1])) as ComposeSpec;
  } catch {
    return null;
  }
}
