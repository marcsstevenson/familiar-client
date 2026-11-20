import type { Attachment } from "./draft";
import type { ComposeSpec } from "./spec";
import { editorStore, result } from "../session";

export type Field = "from" | "to" | "cc" | "bcc" | "subject" | "body";
export type SigChoice = "new" | "reply" | "none";
export type WorkingCopy = {
  spec: ComposeSpec;
  draftId: number | null;
  account: string | null;
  from: string | null;
  to: string[];
  cc: string[];
  bcc: string[];
  pending: Record<"to" | "cc" | "bcc", string>;
  showCc: boolean;
  showBcc: boolean;
  subject: string;
  mode: "rich" | "plain";
  body: string;
  signature: SigChoice;
  attachments: Attachment[];
  remindDays: number;
  edited: Field[];
  removals: string[];
  dirty: boolean;
  sending: { op: number; at: number; later?: Date } | null;
};

export async function readEditor(id: string): Promise<WorkingCopy | undefined> {
  const store = editorStore("readonly");
  return store && result(store.get(id));
}

export async function openEditorIds() {
  const store = editorStore("readonly");
  return store ? (await result(store.getAllKeys())).map(String) : [];
}

export async function rememberEditor(id: string, copy?: WorkingCopy) {
  const store = editorStore("readwrite");
  if (!store) return;
  if (copy) store.put(copy, id);
  else store.delete(id);
  await new Promise<void>((resolve, reject) => {
    store.transaction.oncomplete = () => resolve();
    store.transaction.onabort = () => reject(store.transaction.error);
  });
}
