// Body HTML helpers for the editor. The editor holds the body's HTML; images
// the draft already has are shown from their bytes but keep their `cid:` link
// (in data-cid) so saving doesn't attach them again.

/** The inside of `<body>`, from a whole HTML document or a fragment, made safe for the editor. */
export function bodyOf(html: string): string {
  const doc = new DOMParser().parseFromString(html, "text/html");
  cleanForEditor(doc.body);
  return doc.body.innerHTML;
}

// Elements that can run code, load things, submit, or (style) restyle the
// whole compose window. Text inside them goes too.
const DROP = "script,style,link,meta,base,iframe,frame,frameset,object,embed,applet,form,input,button,textarea,select,noscript,template,svg,math";
// Attributes that hold a URL, and the schemes each may use.
const URL_ATTRS: Record<string, string[]> = {
  href: ["http:", "https:", "mailto:", "#"],
  src: ["http:", "https:", "cid:", "data:image/"],
  background: [],
  action: [],
  formaction: [],
  srcdoc: [],
  srcset: [],
  "xlink:href": [],
};

/**
 * Strips a draft body of anything active before it goes into the editor,
 * which lives in the app's own page. A reply or forward quotes the sender's
 * HTML, so it is treated like a message in the reading pane: no scripts, no
 * event handlers, no frames or forms, no style blocks (they would restyle the
 * compose window), and only web, mail, `cid:` and image `data:` URLs.
 * `root` must come from DOMParser, which never runs or loads anything.
 */
export function cleanForEditor(root: HTMLElement): void {
  for (const el of root.querySelectorAll(DROP)) el.remove();
  for (const el of root.querySelectorAll("*")) {
    for (const a of [...el.attributes]) {
      const name = a.name.toLowerCase();
      const allowed = URL_ATTRS[name];
      if (name.startsWith("on")) {
        el.removeAttribute(a.name);
      } else if (allowed) {
        const v = a.value.trim().toLowerCase();
        if (!allowed.some((s) => v.startsWith(s))) el.removeAttribute(a.name);
      }
    }
  }
}

export function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

/** Plain text as HTML lines, the way `fam` turns a plain body into HTML. */
export function textToHtml(text: string): string {
  return escapeHtml(text).replace(/\r?\n/g, "<br>");
}

/** What HTML reads as, as plain text, line breaks included. */
export function htmlToText(html: string): string {
  const el = document.createElement("div");
  el.style.cssText = "position:fixed;left:-10000px;top:0;white-space:normal";
  el.innerHTML = html;
  document.body.appendChild(el);
  const text = el.innerText;
  el.remove();
  return text.replace(/\n{3,}/g, "\n\n").replace(/\s+$/, "");
}

const SIG = '[id="Signature"]';

/** The signature of the part being written: above a quoted original, never the original sender's. */
export function ownSignature(root: HTMLElement): HTMLElement | null {
  const sig = root.querySelector<HTMLElement>(SIG);
  const quote = root.querySelector("#divRplyFwdMsg");
  if (sig && quote && sig.compareDocumentPosition(quote) & Node.DOCUMENT_POSITION_PRECEDING) return null;
  return sig;
}

/** Puts `sig` in as the signature (replacing one there), or takes it out when null. */
export function setSignature(root: HTMLElement, sig: string | null): void {
  const old = ownSignature(root);
  if (old) {
    if (sig === null) {
      // The blank line `fam` puts before a signature goes with it.
      const prev = old.previousElementSibling;
      if (prev && prev.tagName === "DIV" && prev.textContent === "" && !prev.querySelector("img")) prev.remove();
      old.remove();
    } else {
      old.innerHTML = sig;
    }
    return;
  }
  if (sig === null) return;
  const gap = document.createElement("div");
  gap.innerHTML = "<br>";
  const div = document.createElement("div");
  div.id = "Signature";
  div.innerHTML = sig;
  // Above the quoted original of a reply or forward, otherwise at the end.
  const quote = root.querySelector("#divRplyFwdMsg");
  let at: Element | null = quote?.previousElementSibling?.tagName === "HR" ? quote.previousElementSibling : quote;
  while (at && at.parentElement !== root) at = at.parentElement;
  // In a fresh reply the new part is the empty <div> before the <hr>.
  const slot = at?.previousElementSibling;
  if (at && slot && slot.tagName === "DIV" && slot.textContent === "" && !slot.querySelector("img")) {
    // A line to type on, a blank line, then the signature.
    const line = document.createElement("div");
    line.innerHTML = "<br>";
    slot.append(line, gap, div);
  } else if (at) {
    root.insertBefore(gap, at);
    root.insertBefore(div, at);
  } else {
    root.append(gap, div);
  }
}

/** Shows `cid:` images from their bytes (content ID → data: URI). Unknown ones stay as they are. */
export function showCids(root: HTMLElement, images: Map<string, string>): void {
  for (const img of root.querySelectorAll<HTMLImageElement>('img[src^="cid:"]')) {
    const cid = img.getAttribute("src")!.slice(4);
    const data = images.get(cid);
    if (data) {
      img.dataset.cid = cid;
      img.src = data;
    }
  }
}

/** The editor's HTML as it is saved: shown `cid:` images get their links back. */
export function serialize(root: HTMLElement): string {
  const copy = root.cloneNode(true) as HTMLElement;
  for (const img of copy.querySelectorAll<HTMLImageElement>("img[data-cid]")) {
    img.setAttribute("src", `cid:${img.dataset.cid}`);
    img.removeAttribute("data-cid");
  }
  return copy.innerHTML;
}

/** Plain text back to HTML, with the signature's text turned back into the signature. */
export function plainToRich(text: string, sigText: string | null, sigHtml: string | null): string {
  if (sigText && sigHtml) {
    const at = text.indexOf(sigText);
    if (at >= 0) {
      return (
        textToHtml(text.slice(0, at).replace(/\n+$/, "")) +
        `<div><br></div><div id="Signature">${sigHtml}</div>` +
        textToHtml(text.slice(at + sigText.length))
      );
    }
  }
  return textToHtml(text);
}

/** True when the body has nothing typed in it (the signature doesn't count). */
export function isBlank(root: HTMLElement): boolean {
  const copy = root.cloneNode(true) as HTMLElement;
  ownSignature(copy)?.remove();
  return copy.textContent!.trim() === "" && !copy.querySelector("img");
}
