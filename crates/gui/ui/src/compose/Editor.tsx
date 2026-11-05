// The body editor: rich text in a contenteditable with a small toolbar, or a
// plain textarea. Rich editing uses the browser's own editing commands, which
// WebView2 and WebKit both support.
import { For, Show, createSignal } from "solid-js";
import { ownSignature } from "./html";

type Tool = { cmd: string; label: string; text: string; key?: string };

const TOOLS: Tool[] = [
  { cmd: "bold", label: "Bold (Ctrl+B)", text: "B", key: "b" },
  { cmd: "italic", label: "Italic (Ctrl+I)", text: "I", key: "i" },
  { cmd: "underline", label: "Underline (Ctrl+U)", text: "U", key: "u" },
  { cmd: "strikeThrough", label: "Strikethrough", text: "S" },
  { cmd: "insertUnorderedList", label: "Bulleted list", text: "•" },
  { cmd: "insertOrderedList", label: "Numbered list", text: "1." },
  { cmd: "outdent", label: "Decrease indent", text: "⇤" },
  { cmd: "indent", label: "Increase indent", text: "⇥" },
  { cmd: "removeFormat", label: "Clear formatting", text: "T̸" },
];

export function RichEditor(props: {
  ref: (el: HTMLDivElement) => void;
  onInput: () => void;
  onPasteImage: (file: File) => void;
  onPicture: () => void;
  disabled?: boolean;
}) {
  let el!: HTMLDivElement;
  let saved: Range | null = null;
  const [linking, setLinking] = createSignal(false);
  const [href, setHref] = createSignal("https://");

  const exec = (cmd: string, value?: string) => {
    el.focus();
    document.execCommand(cmd, false, value);
    props.onInput();
  };
  const startLink = () => {
    const sel = window.getSelection();
    saved = sel && sel.rangeCount && el.contains(sel.anchorNode) ? sel.getRangeAt(0).cloneRange() : null;
    setHref("https://");
    setLinking(true);
  };
  const applyLink = () => {
    setLinking(false);
    el.focus();
    if (saved) {
      const sel = window.getSelection()!;
      sel.removeAllRanges();
      sel.addRange(saved);
    }
    const url = href().trim();
    if (!url || url === "https://") return;
    if (saved && !saved.collapsed) exec("createLink", url);
    else exec("insertHTML", `<a href="${url.replace(/"/g, "&quot;")}">${url.replace(/</g, "&lt;")}</a>`);
  };

  return (
    <div class="editor-wrap">
      <div class="toolbar" role="toolbar" aria-label="Formatting" onMouseDown={(e) => {
        // Keep the selection in the body while a button is pressed.
        if ((e.target as HTMLElement).closest("button")) e.preventDefault();
      }}>
        <For each={TOOLS}>
          {(t) => (
            <button type="button" class={`tool tool-${t.cmd}`} title={t.label} aria-label={t.label} disabled={props.disabled} onClick={() => exec(t.cmd)}>
              {t.text}
            </button>
          )}
        </For>
        <button type="button" class="tool" title="Link" aria-label="Link" disabled={props.disabled} onClick={startLink}>
          🔗
        </button>
        <button type="button" class="tool" title="Picture" aria-label="Insert picture" disabled={props.disabled} onClick={() => props.onPicture()}>
          🖼
        </button>
        <Show when={linking()}>
          <input
            class="link-input"
            aria-label="Link address"
            value={href()}
            ref={(i) => queueMicrotask(() => i.select())}
            onInput={(e) => setHref(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                applyLink();
              } else if (e.key === "Escape") {
                e.preventDefault();
                setLinking(false);
                el.focus();
              }
            }}
            onBlur={() => setLinking(false)}
          />
        </Show>
      </div>
      <div
        ref={(d) => {
          el = d;
          props.ref(d);
        }}
        class="editor"
        contentEditable={!props.disabled}
        role="textbox"
        aria-multiline="true"
        aria-label="Message body"
        spellcheck={true}
        onInput={() => props.onInput()}
        onKeyDown={(e) => {
          if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
            e.preventDefault();
            startLink();
          }
        }}
        onPaste={(e) => {
          const files = [...(e.clipboardData?.files ?? [])].filter((f) => f.type.startsWith("image/"));
          if (files.length) {
            e.preventDefault();
            files.forEach(props.onPasteImage);
          }
        }}
      />
    </div>
  );
}

export function PlainEditor(props: { value: string; onInput: (v: string) => void; disabled?: boolean }) {
  return (
    <div class="editor-wrap">
      <textarea
        class="editor plain"
        aria-label="Message body"
        disabled={props.disabled}
        value={props.value}
        onInput={(e) => props.onInput(e.currentTarget.value)}
      />
    </div>
  );
}

/** Puts an image at the caret, or at a point in the window. */
export function insertImage(editor: HTMLElement, src: string, at?: { x: number; y: number }): void {
  const img = document.createElement("img");
  img.src = src;
  img.style.maxWidth = "100%";
  let range: Range | null = null;
  if (at) {
    const doc = document as Document & { caretRangeFromPoint?: (x: number, y: number) => Range | null };
    range = doc.caretRangeFromPoint?.(at.x, at.y) ?? null;
  } else {
    const sel = window.getSelection();
    if (sel && sel.rangeCount) range = sel.getRangeAt(0);
  }
  if (range && editor.contains(range.startContainer)) {
    range.deleteContents();
    range.insertNode(img);
    range.setStartAfter(img);
    range.collapse(true);
    const sel = window.getSelection();
    sel?.removeAllRanges();
    sel?.addRange(range);
  } else {
    // Above the signature, or at the end.
    const sig = ownSignature(editor);
    const line = document.createElement("div");
    line.append(img);
    if (sig) (sig.previousElementSibling ?? sig).before(line);
    else editor.append(line);
  }
}
