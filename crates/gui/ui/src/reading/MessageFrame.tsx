// The message body: a sanitised document in a frame with no scripts.
//
// The frame is sandboxed with `allow-same-origin` and without `allow-scripts`,
// so nothing in the message can run, while this pane can still reach into the
// document to show where a link goes and to open links outside the app.
import { createEffect, onCleanup } from "solid-js";
import { onGlobalKey } from "../actions/registry";
import { DARK_FIRST_CSS, darken } from "./dark";
import type { QuoteAction } from "../compose/spec";
import { addQuotedActions } from "./quotedActions";
import { readSession, writeSession } from "../session";

export type HoverLink = { href: string; text: string } | null;

export function MessageFrame(props: {
  message: number;
  standalone?: boolean;
  html: string;
  onHover: (l: HoverLink) => void;
  onOpen: (href: string) => void;
  onLoad: () => void;
  onQuote: (index: number, action: QuoteAction) => void;
  /** Show it dark (the app is dark and the message isn't switched to light). */
  dark: boolean;
}) {
  let frame!: HTMLIFrameElement;
  let visibility: ResizeObserver | undefined;

  const linkOf = (t: EventTarget | null): HTMLAnchorElement | null => {
    let el = t as Element | null;
    while (el && el.nodeType === 1) {
      if (el.tagName === "A" && (el as HTMLAnchorElement).hasAttribute("href")) {
        return el as HTMLAnchorElement;
      }
      el = el.parentElement;
    }
    return null;
  };

  const attach = () => {
    visibility?.disconnect();
    const doc = frame.contentDocument;
    if (!doc) return;
    if (props.dark) darken(doc);
    addQuotedActions(doc, props.onQuote, props.dark);
    const id = props.message;
    const key = props.standalone ? "readerScroll/" + id : "readingScroll";
    const page = doc.scrollingElement;
    if (page) {
      let visible = false;
      const restore = () => {
        const showing = frame.clientHeight > 0;
        if (showing && !visible) {
          const saved = readSession<{ id: number; top: number }>(key);
          if (saved?.id === id) page.scrollTop = saved.top;
        }
        visible = showing;
      };
      restore();
      visibility = new ResizeObserver(restore);
      visibility.observe(frame);
      doc.addEventListener("scroll", () => {
        if (visible && frame.clientHeight) writeSession(key, { id, top: page.scrollTop });
      });
    }
    // Keys pressed while the body has focus don't reach the app's window.
    doc.addEventListener("keydown", onGlobalKey);
    doc.addEventListener("mouseover", (e) => {
      const a = linkOf(e.target);
      props.onHover(a ? { href: a.getAttribute("href") ?? "", text: a.textContent ?? "" } : null);
    });
    doc.addEventListener("mouseleave", () => props.onHover(null));
    doc.addEventListener("focusin", (e) => {
      const a = linkOf(e.target);
      props.onHover(a ? { href: a.getAttribute("href") ?? "", text: a.textContent ?? "" } : null);
    });
    doc.addEventListener("click", (e) => {
      const a = linkOf(e.target);
      if (!a) return;
      const href = a.getAttribute("href") ?? "";
      if (href.startsWith("#")) return; // Jumps within the message.
      e.preventDefault();
      props.onOpen(href);
    });
    // Middle click and drag-out must not navigate either.
    doc.addEventListener("auxclick", (e) => {
      if (linkOf(e.target)) e.preventDefault();
    });
    props.onLoad();
  };

  // Switching between dark and light reloads the message, which is local.
  createEffect(() => {
    frame.srcdoc = props.dark ? props.html.replace("</head>", `<style>${DARK_FIRST_CSS}</style></head>`) : props.html;
  });
  onCleanup(() => { visibility?.disconnect(); props.onHover(null); });

  return (
    <iframe
      ref={frame}
      class="message-frame"
      classList={{ dark: props.dark }}
      title="Message body"
      sandbox="allow-same-origin"
      referrerpolicy="no-referrer"
      onLoad={attach}
    />
  );
}
