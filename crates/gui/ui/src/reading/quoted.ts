// Outlook embeds earlier messages in the body. A From/Sent/To/Cc/Subject
// header identifies the start of each one, even when several subjects match.
import { addressOf, splitAddresses } from "../compose/Recipients";
import type { QuoteAction } from "../compose/spec";
import type { Identity } from "../compose/ipc";

type Line = { text: string; range: Range };
export type QuotedHeader = { row: Range; from: string[]; to: string[]; cc: string[]; subject: string };

// Keep the DOM position of each logical line. Outlook often splits a single
// field across several styled spans and uses <br> between fields.
function linesOf(doc: Document): Line[] {
  const lines: Line[] = [];
  let text = "";
  let range: Range | null = null;
  function finish() {
    if (range && text.trim()) lines.push({ text: text.trim(), range });
    text = "";
    range = null;
  }
  function visit(node: Node, pre: boolean) {
    if (node.nodeType === Node.TEXT_NODE) {
      const value = node.nodeValue ?? "";
      let offset = 0;
      for (const part of pre ? value.split("\n") : [value]) {
        if (offset) finish();
        if (part.length) {
          if (!range) {
            range = doc.createRange();
            range.setStart(node, offset);
          }
          range.setEnd(node, offset + part.length);
          text += part;
        }
        offset += part.length + 1;
      }
      return;
    }
    const el = node as Element;
    if (!el.tagName || el.matches("style,script")) return;
    if (el.tagName === "BR") { finish(); return; }
    const block = el.matches("div,p,pre,blockquote,li,td,tr,hr,h1,h2,h3,h4,h5,h6");
    if (block) finish();
    for (const child of el.childNodes) visit(child, pre || el.tagName === "PRE");
    // Some clients link the display name without printing the address.
    if (el.tagName === "A" && !el.textContent?.includes("@")) {
      const addr = /^mailto:([^?#\s<>]+@[^?#\s<>]+)$/i.exec(el.getAttribute("href") ?? "");
      if (addr) text += ` <${addr[1]}>`;
    }
    if (block) finish();
  }
  visit(doc.body, false);
  finish();
  return lines;
}

export function quotedHeaders(doc: Document): QuotedHeader[] {
  if (!/From:/i.test(doc.body.textContent ?? "")) return [];
  const lines = linesOf(doc);
  const headers: QuotedHeader[] = [];
  for (const [i, line] of lines.entries()) {
    const sender = /^From:\s*(.+)$/is.exec(line.text);
    if (!sender || !/^(Sent|Date):/i.test(lines[i + 1]?.text ?? "")) continue;
    let j = i + 2;
    const field = (label: string) => {
      const text = lines[j]?.text;
      if (!text?.toLowerCase().startsWith(label)) return null;
      j++;
      return text.slice(label.length).trim();
    };
    const to = field("to:") ?? "";
    const cc = field("cc:") ?? "";
    const subject = field("subject:");
    if (subject !== null) headers.push({
      row: line.range, from: splitAddresses(sender[1]!), to: splitAddresses(to), cc: splitAddresses(cc), subject,
    });
  }
  return headers;
}

export function quotedRecipients(header: QuotedHeader, action: QuoteAction, identity?: Identity) {
  const addressed = [...header.to, ...header.cc].map(m => addressOf(m).toLowerCase());
  const alias = identity?.aliases.find(a => addressed.includes(a.toLowerCase()));
  const seen = new Set([identity?.email.toLowerCase(), alias?.toLowerCase()]);
  const unique = (mailboxes: string[]) => mailboxes.filter(m => {
    const addr = addressOf(m).toLowerCase();
    if (seen.has(addr)) return false;
    seen.add(addr);
    return true;
  });
  const to = action === "replyAll" ? unique([...header.from, ...header.to]) : action === "reply" ? header.from : [];
  return { to, cc: action === "replyAll" ? unique(header.cc) : [], alias };
}

/** Copy from the selected header onwards, including its older history. */
export function quotedHtml(doc: Document, header: QuotedHeader): string {
  const range = header.row.cloneRange();
  const container = range.startContainer.parentElement?.closest("blockquote") ?? doc.body;
  range.setEnd(container, container.childNodes.length);
  const quote = doc.createElement("div");
  quote.append(range.cloneContents());
  // The reading pane's proxy URLs and blocked-image placeholders are local.
  for (const img of quote.querySelectorAll("img")) {
    if (!img.src.startsWith("data:image/") || img.src.endsWith("#blocked")) img.replaceWith(img.alt);
  }
  return quote.innerHTML;
}
