// Each mail store has its own message, folder and calendar IDs.
let prefix = "";
export function initSession(store: string | null, bench: boolean) {
  prefix = store && !bench ? "familiar.session." + JSON.stringify(store) + "/" : "";
}

export function readSession<T>(key: string): T | undefined {
  if (!prefix) return;
  try { return JSON.parse(localStorage.getItem(prefix + key) ?? "null") ?? undefined; }
  catch { return undefined; }
}

export function writeSession(key: string, value: unknown) {
  if (!prefix) return;
  try {
    if (value === undefined) localStorage.removeItem(prefix + key);
    else localStorage.setItem(prefix + key, JSON.stringify(value));
  } catch (e) { console.warn("Couldn't remember " + key, e); }
}

export function readerWindows(): number[] {
  if (!prefix) return [];
  const start = prefix + "reader/";
  return Object.keys(localStorage).filter(k => k.startsWith(start))
    .map(k => Number(k.slice(start.length))).filter(Number.isSafeInteger);
}

// An editor can contain several megabytes of inline images. IndexedDB keeps
// these working copies off the localStorage quota and commits them separately.
let editors: IDBDatabase | undefined;
export async function connectEditors() {
  if (!prefix) return;
  editors = await new Promise<IDBDatabase>((resolve, reject) => {
    const open = indexedDB.open(prefix + "editors", 1);
    open.onupgradeneeded = () => open.result.createObjectStore("windows");
    open.onsuccess = () => resolve(open.result);
    open.onerror = () => reject(open.error);
  });
}

export function editorStore(mode: IDBTransactionMode) {
  return editors?.transaction("windows", mode).objectStore("windows");
}

// Working-copy transactions are opened in the input event's turn. A later
// read on the same store (even in the main window) waits for all earlier
// writes to commit, without transferring any of the message bodies.
export async function flushEditorWrites() {
  const store = editorStore("readonly");
  if (store) await result(store.count());
}

export function result<T>(request: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}
