/** Let the browser link the current text selection as one undoable edit. */
export function pasteLink(event: ClipboardEvent, editor: HTMLElement): boolean {
  const selection = window.getSelection();
  if (!editor.isContentEditable || !selection?.rangeCount || !selection.toString()
    || !editor.contains(selection.getRangeAt(0).commonAncestorContainer)) return false;

  const data = event.clipboardData;
  const uri = (data?.getData("text/plain") || data?.getData("text/uri-list") || "").trim();
  if (!/^(?:https?:\/\/|mailto:).+/i.test(uri) || /[\s\u0000-\u001f]/.test(uri)) return false;
  try {
    new URL(uri);
  } catch {
    return false;
  }
  if (!document.execCommand("createLink", false, uri)) return false;
  event.preventDefault();
  return true;
}
