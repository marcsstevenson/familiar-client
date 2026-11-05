// Reading drafts for the editor. Saving and sending use the same `draft.*`
// commands as `fam draft`, so what this window writes the CLI can read and
// change, and the other way round.
import { type Commands, run } from "../api";

type WriteOutput = Commands["draft.create"]["output"];
type Address = Commands["mail.fetch"]["output"]["to"][number];

/** The local ID of the draft a create, reply or forward made. */
export function createdDraft(out: WriteOutput): number | null {
  return out.ops.find((o) => o.kind === "create_draft")?.item ?? null;
}

/** `Name <addr>` (quoted when the name has a comma), or the bare address. */
export function mailbox(a: { name: string; addr: string }): string {
  if (!a.name || a.name === a.addr) return a.addr;
  const name = /[,;"<>]/.test(a.name) ? `"${a.name.replace(/"/g, "")}"` : a.name;
  return `${name} <${a.addr}>`;
}

export type Attachment = {
  name: string;
  size: number;
  /** A file to upload on the next save. */
  path?: string;
  /** Graph's ID, for one the draft already has. */
  id?: string;
};

export type LoadedDraft = {
  id: number;
  account: string;
  remoteId: string;
  subject: string;
  to: string[];
  cc: string[];
  /** Null when it couldn't be read from the server (the store doesn't keep Bcc). */
  bcc: string[] | null;
  /** The inside of `<body>`. */
  html: string;
  attachments: Attachment[];
  /** Inline images by content ID, as `data:` URIs. */
  images: Map<string, string>;
};

const onServer = (remoteId: string) => remoteId !== "" && !remoteId.startsWith("local:");

/**
 * Reads a draft: header and body from the store (downloading the body if it
 * isn't on this device), and Bcc and attachments from the server through
 * famd. Without famd or a connection those stay unknown; images then show as
 * missing but keep their links, so saving leaves them on the draft.
 */
export async function loadDraft(id: number, body: (html: string) => string): Promise<LoadedDraft> {
  const f = await run("mail.fetch", { message: id, format: "html" });
  const d: LoadedDraft = {
    id,
    account: f.account,
    remoteId: f.remote_id,
    subject: f.subject,
    to: f.to.map(mailbox),
    cc: f.cc.map(mailbox),
    bcc: null,
    html: body(f.body ?? ""),
    attachments: [],
    images: new Map(),
  };
  if (!onServer(f.remote_id)) {
    d.bcc = [];
    return d;
  }
  const path = `/me/messages/${f.remote_id}`;
  try {
    const m = (await run("api", { account: f.account, path: `${path}?$select=bccRecipients` })) as {
      bccRecipients?: { emailAddress: { name?: string; address: string } }[];
    };
    d.bcc = (m.bccRecipients ?? []).map((r) => mailbox({ name: r.emailAddress.name ?? "", addr: r.emailAddress.address }));
  } catch {
    // Unknown: Bcc is only sent if it is changed here.
  }
  if (f.has_attachments || d.html.includes("cid:")) {
    try {
      const a = (await run("api", { account: f.account, path: `${path}/attachments` })) as {
        value?: { id: string; name: string; size?: number; isInline?: boolean; contentId?: string | null; contentType?: string; contentBytes?: string }[];
      };
      for (const x of a.value ?? []) {
        const cid = x.contentId?.replace(/^<|>$/g, "");
        if (x.isInline && cid && x.contentBytes && d.html.includes(`cid:${cid}`)) {
          d.images.set(cid, `data:${x.contentType ?? "image/png"};base64,${x.contentBytes}`);
        } else if (!x.isInline) {
          d.attachments.push({ name: x.name, size: x.size ?? 0, id: x.id });
        }
      }
    } catch {
      // Offline: the attachments stay on the draft, just not listed here.
    }
  }
  return d;
}

export type { Address, WriteOutput };
