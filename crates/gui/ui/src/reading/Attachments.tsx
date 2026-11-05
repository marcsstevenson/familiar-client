// The attachment bar: names and sizes, a preview for images and text, and
// Save, which puts the file in Downloads without replacing anything there.
import { For, Show, createResource, createSignal } from "solid-js";
import {
  type AttachmentInfo,
  type Preview,
  errorText,
  listAttachments,
  previewAttachment,
  saveAttachment,
} from "./api";

export function humanSize(n: number): string {
  if (n >= 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  if (n >= 1024) return `${Math.ceil(n / 1024)} KB`;
  return `${n} bytes`;
}

export function Attachments(props: { message: number; onStatus: (s: string) => void }) {
  const [list] = createResource(() => props.message, listAttachments);
  const [preview, setPreview] = createSignal<{ a: AttachmentInfo; p: Preview } | null>(null);
  const [busy, setBusy] = createSignal<string | null>(null);

  const open = async (a: AttachmentInfo) => {
    setBusy(a.id);
    try {
      const p = await previewAttachment(props.message, a.id);
      if (p.kind === "none") {
        props.onStatus(`No preview for ${a.name}; use Save to open it in its own app.`);
      } else {
        setPreview({ a, p });
      }
    } catch (e) {
      props.onStatus(`Couldn't open ${a.name}: ${errorText(e)}`);
    } finally {
      setBusy(null);
    }
  };

  const save = async (a: AttachmentInfo) => {
    setBusy(a.id);
    try {
      props.onStatus(`Saved to ${await saveAttachment(props.message, a.id)}`);
    } catch (e) {
      props.onStatus(`Couldn't save ${a.name}: ${errorText(e)}`);
    } finally {
      setBusy(null);
    }
  };

  return (
    <>
      <Show when={list.error}>
        <div class="reading-attachments muted">Attachments unavailable: {errorText(list.error)}</div>
      </Show>
      <Show when={(list() ?? []).length > 0}>
        <ul class="reading-attachments" aria-label="Attachments">
          <For each={list()}>
            {(a) => (
              <li class="attachment" aria-busy={busy() === a.id}>
                <button
                  class="attachment-name"
                  title={`Preview ${a.name}`}
                  disabled={a.kind !== "file"}
                  onClick={() => open(a)}
                >
                  {a.name}
                  <span class="muted">
                    {a.kind === "item" ? " Outlook item" : a.kind === "reference" ? " Cloud link" : ` ${humanSize(a.size)}`}
                  </span>
                </button>
                <Show when={a.kind === "file"}>
                  <button class="attachment-save" title={`Save ${a.name} to Downloads`} onClick={() => save(a)}>
                    Save
                  </button>
                </Show>
              </li>
            )}
          </For>
        </ul>
      </Show>
      <Show when={preview()}>
        {(v) => (
          <div
            class="attachment-preview"
            role="dialog"
            aria-label={`Preview of ${v().a.name}`}
            tabindex="-1"
            ref={(el) => queueMicrotask(() => el.focus())}
            onKeyDown={(e) => e.key === "Escape" && setPreview(null)}
          >
            <div class="attachment-preview-bar">
              <span>{v().a.name}</span>
              <span class="statusbar-spacer" />
              <button onClick={() => save(v().a)}>Save</button>
              <button onClick={() => setPreview(null)} aria-label="Close preview">
                Close
              </button>
            </div>
            <div class="attachment-preview-body">
              {(() => {
                const p = v().p;
                return p.kind === "image" ? (
                  <img src={p.src} alt={v().a.name} />
                ) : p.kind === "text" ? (
                  <pre>{p.text}</pre>
                ) : null;
              })()}
            </div>
          </div>
        )}
      </Show>
    </>
  );
}
