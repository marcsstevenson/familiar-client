// The attachment bar: names and sizes, the viewer for images, PDFs and text
// (AttachmentViewer.tsx), and Save, which puts the file in Downloads without
// replacing anything there.
import { For, Show, createResource, createSignal } from "solid-js";
import {
  type AttachmentInfo,
  type Preview,
  errorText,
  listAttachments,
  previewAttachment,
  saveAttachment,
} from "./api";
import { AttachmentViewer } from "./AttachmentViewer";

export function humanSize(n: number): string {
  if (n >= 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  if (n >= 1024) return `${Math.ceil(n / 1024)} KB`;
  return `${n} bytes`;
}

export function Attachments(props: { message: number; onStatus: (s: string) => void }) {
  const [list] = createResource(() => props.message, listAttachments);
  const [viewing, setViewing] = createSignal<{ index: number; first: Preview } | null>(null);
  // The attachments the viewer steps through: real files, not Outlook items or links.
  const files = () => (list() ?? []).filter((a) => a.kind === "file");
  const [busy, setBusy] = createSignal<string | null>(null);

  const open = async (a: AttachmentInfo) => {
    setBusy(a.id);
    try {
      const p = await previewAttachment(props.message, a.id);
      if (p.kind === "none") {
        props.onStatus(`No preview for ${a.name}; use Save to open it in its own app.`);
      } else {
        setViewing({ index: files().findIndex((f) => f.id === a.id), first: p });
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
                  title={`View ${a.name}`}
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
      <Show when={viewing()}>
        {(v) => (
          <AttachmentViewer
            message={props.message}
            files={files()}
            index={v().index}
            first={v().first}
            onIndex={(index) => setViewing({ ...v(), index })}
            onSave={save}
            onClose={() => setViewing(null)}
          />
        )}
      </Show>
    </>
  );
}
