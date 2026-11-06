// The message body: a sanitised document in a frame with no scripts.
//
// The frame is sandboxed with `allow-same-origin` and without `allow-scripts`,
// so nothing in the message can run, while this pane can still reach into the
// document to show where a link goes and to open links outside the app.
import { createEffect, onCleanup } from "solid-js";
import { onGlobalKey } from "../actions/registry";

export type HoverLink = { href: string; text: string } | null;

// Dark mode for a message written for a white page, as Outlook does: the body
// is inverted with its hues kept, and pictures are inverted back (very
// saturated colours in them shift a little: ☀ shows the message as sent). The page
// behind the body is set to what white becomes, so short messages match.
const DARK_CSS = `html.fam-dark{background:#121212}
html.fam-dark body{filter:invert(1) hue-rotate(180deg) contrast(0.87);min-height:100vh;box-sizing:border-box}
html.fam-dark :is(img,video,picture,svg,[style*="background-image"],[background]){filter:contrast(1.149) invert(1) hue-rotate(180deg)}
html.fam-dark :is(picture,svg,[style*="background-image"],[background]) :is(img,video,picture,svg,[style*="background-image"],[background]){filter:none}`;

export function MessageFrame(props: {
  html: string;
  onHover: (l: HoverLink) => void;
  onOpen: (href: string) => void;
  onLoad: () => void;
  /** Show it dark (the app is dark and the message isn't switched to light). */
  dark: boolean;
}) {
  let frame!: HTMLIFrameElement;

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
    const doc = frame.contentDocument;
    if (!doc) return;
    const style = doc.createElement("style");
    style.textContent = DARK_CSS;
    doc.head.append(style);
    doc.documentElement.classList.toggle("fam-dark", props.dark);
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

  createEffect(() => {
    frame.srcdoc = props.html;
  });
  createEffect(() => {
    frame.contentDocument?.documentElement.classList.toggle("fam-dark", props.dark);
  });
  onCleanup(() => props.onHover(null));

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
