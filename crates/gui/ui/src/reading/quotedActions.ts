import { looksValid } from "../compose/Recipients";
import type { QuoteAction } from "../compose/spec";
import { quotedHeaders } from "./quoted";
import css from "./quotedActions.css?inline";

/** Decorate the reading frame only. The compose window reads a clean copy. */
export function addQuotedActions(doc: Document, open: (index: number, action: QuoteAction) => void) {
  const headers = quotedHeaders(doc);
  if (!headers.length) return;
  const style = doc.createElement("style");
  style.textContent = css;
  doc.head.append(style);
  headers.forEach(({ row, from, to, cc }, index) => {
    // Include the inline formatting around the sender in the new row.
    let start = row.startContainer;
    while (row.startOffset === 0 && start.parentElement?.matches("b,strong,span,i,font,a,em")) {
      start = start.parentElement;
      row.setStartBefore(start);
    }
    const wrapper = doc.createElement("span");
    wrapper.className = "fam-quoted-from";
    const bar = doc.createElement("span");
    bar.className = "fam-quoted-actions";
    bar.setAttribute("role", "group");
    bar.setAttribute("aria-label", "Actions for this quoted message");
    const actions = [["reply", "Reply"], ["replyAll", "Reply all"], ["forward", "Forward"]] as const;
    for (const [action, label] of actions) {
      const b = doc.createElement("button");
      b.type = "button";
      b.textContent = label;
      const targets = action === "reply" ? from : action === "replyAll" ? [...from, ...to, ...cc] : [];
      b.disabled = targets.some(m => !looksValid(m));
      b.title = b.disabled ? "The quoted header is missing an email address" : label;
      b.addEventListener("click", () => open(index, action));
      bar.append(b);
    }
    wrapper.append(bar, row.extractContents());
    row.insertNode(wrapper);
  });
}
