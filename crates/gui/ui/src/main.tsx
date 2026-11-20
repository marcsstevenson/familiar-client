import { render } from "solid-js/web";
import { Show } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { uiInfo } from "./api";
import { App } from "./App";
import { ComposeWindow, composeSpec, installComposeKeys } from "./compose";
import { initTheme } from "./theme";
import { ReadingPane } from "./reading/ReadingPane";
import { SpamDialog } from "./spam/SpamDialog";
import { notify, setSelectedMessage, setSpamDialog, spamDialog, spamSettings } from "./state";
import { initSession, readSession, readerWindows, writeSession } from "./session";
import { openEditorIds, readEditor } from "./compose/recovery";
import "./styles/tokens.css";
import "./styles/app.css";

const info = await uiInfo();
initSession(info.session_store, info.bench);
document.documentElement.dataset.platform = info.platform;
const request = composeSpec();
const editorId = request?.kind === "resume" ? request.editor : request ? crypto.randomUUID() : undefined;
const recovered = request?.kind === "resume" ? await readEditor(request.editor) : undefined;
const spec = request?.kind === "resume" ? recovered?.spec : request;
const reader = /^#reader=(\d+)$/.exec(window.location.hash);
const mainWindow = !request && !reader;
const active = readSession<string>("active");
const windowKey = editorId ?? (reader ? "reader/" + reader[1] : "main");
const rememberFocus = () => writeSession("active", windowKey);
window.addEventListener("focus", rememberFocus);
if (document.hasFocus()) rememberFocus();
initTheme(info.theme, mainWindow);
if (mainWindow) installComposeKeys();
if (reader) {
  const win = getCurrentWindow();
  writeSession(windowKey, await win.title());
  await win.onCloseRequested(() => {
    writeSession(windowKey, undefined);
    writeSession("readerScroll/" + reader[1], undefined);
  });
  setSelectedMessage(Number(reader[1]));
  spamSettings.accounts = info.spam_accounts;
}
render(
  () => spec ? <ComposeWindow info={info} spec={spec} editorId={editorId} recovered={recovered} /> : reader ? (
    <main class="reader-window">
      <ReadingPane standalone />
      <Show when={spamDialog()}>
        {(d) => <SpamDialog message={d().message} saved={d().saved} onClose={() => setSpamDialog(null)} />}
      </Show>
    </main>
  ) : mainWindow ? <App info={info} /> : <p>This editor's working copy is unavailable.</p>,
  document.getElementById("root")!,
);
async function reopenWindows() {
  for (const message of readerWindows()) {
    const key = "reader/" + message;
    await invoke("open_reader", { message, subject: readSession<string>(key) ?? "Message", focus: active === key });
  }
  for (const editor of await openEditorIds()) {
    await invoke("open_compose", { spec: { kind: "resume", editor }, focus: active === editor });
  }
}
if (mainWindow && !info.bench) void reopenWindows().catch(e => notify("Couldn't reopen a window: " + e, true));
if (mainWindow && info.bench) {
  // A bench that fails still reports, so the harness shows why.
  void import("./bench")
    .then((b) => b.runBench())
    .catch((e) => import("@tauri-apps/api/core").then((c) => c.invoke("bench_report", { report: { error: String(e) } })));
}
