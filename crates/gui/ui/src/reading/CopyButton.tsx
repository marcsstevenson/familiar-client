// A small copy-to-clipboard icon for a header field. It stays out of sight
// until the pointer is over its row (or it has keyboard focus), and shows a
// tick for a moment after copying.

import { createSignal, onCleanup } from "solid-js";

export function CopyButton(props: { text: () => string; label: string }) {
  const [copied, setCopied] = createSignal(false);
  let timer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(timer));

  const copy = async (e: MouseEvent) => {
    e.stopPropagation();
    try {
      await navigator.clipboard.writeText(props.text());
      setCopied(true);
      clearTimeout(timer);
      timer = setTimeout(() => setCopied(false), 1200);
    } catch (err) {
      console.warn("copy to clipboard failed", err);
    }
  };

  return (
    <button
      type="button"
      class="reading-copy"
      classList={{ copied: copied() }}
      title={copied() ? "Copied" : props.label}
      aria-label={props.label}
      onClick={copy}
    >
      {copied() ? (
        <svg viewBox="0 0 16 16" aria-hidden="true">
          <path d="M3 8.5l3 3 7-7" />
        </svg>
      ) : (
        <svg viewBox="0 0 16 16" aria-hidden="true">
          <rect x="5.5" y="5.5" width="8" height="8" rx="1.5" />
          <path d="M10.5 3.5v-.5a1.5 1.5 0 0 0-1.5-1.5H4A1.5 1.5 0 0 0 2.5 3v5A1.5 1.5 0 0 0 4 9.5h.5" />
        </svg>
      )}
    </button>
  );
}
