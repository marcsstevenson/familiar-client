// The reading pane's own IPC commands (crates/gui/src/reading). Everything
// else goes through `run` in ../api.ts.
import { invoke } from "@tauri-apps/api/core";
import type { AttachmentInfo, MailFetchOutput } from "../../../../../bindings/commands";

export type { AttachmentInfo, MailFetchOutput };

export type ReadView = {
  /** The header, as `fam mail fetch` returns it (without the body). */
  message: MailFetchOutput;
  /** The sanitised document, or null when the body isn't on this device yet. */
  html: string | null;
  blocked_images: number;
  images_shown: boolean;
  sender_trusted: boolean;
  /** Inline images haven't been downloaded yet. */
  inline_pending: boolean;
  took_ms: number;
};

export type Preview =
  | { kind: "image"; src: string }
  | { kind: "text"; text: string }
  | { kind: "none" };

export const openMessage = (message: number, local: boolean, showImages: boolean) =>
  invoke<ReadView>("reading_open", { message, local, showImages });

export const trustSender = (address: string, trusted: boolean) =>
  invoke<void>("reading_trust_sender", { address, trusted });

export const listAttachments = (message: number) =>
  invoke<AttachmentInfo[]>("reading_attachments", { message });

export const previewAttachment = (message: number, attachment: string) =>
  invoke<Preview>("reading_preview", { message, attachment });

export const saveAttachment = (message: number, attachment: string) =>
  invoke<string>("reading_save", { message, attachment });

export const openLink = (url: string) => invoke<void>("reading_open_link", { url });

/** The error text of a failed command or IPC call. */
export function errorText(e: unknown): string {
  if (typeof e === "object" && e !== null && "message" in e) return String(e.message);
  return String(e);
}
