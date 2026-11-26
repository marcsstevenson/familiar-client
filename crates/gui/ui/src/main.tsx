import { render } from "solid-js/web";
import { Show } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { uiInfo } from "./api";
import { App } from "./App";
import { Welcome, onboardingState } from "./onboarding/Welcome";
import { ComposeWindow, composeSpec, installComposeKeys } from "./compose";
import { followWindowIcon } from "./icon";
import { flushPreferences, initPreferences } from "./preferences";
import { importPreferences } from "./importPreferences";
import { initTheme } from "./theme";
import { ReadingPane } from "./reading/ReadingPane";
import { SpamDialog } from "./spam/SpamDialog";
import { notify, setSelectedMessage, setSpamDialog, spamDialog, spamSettings } from "./state";
import { connectEditors, flushEditorWrites, initSession, readSession, readerWindows, writeSession } from "./session";
import { openEditorIds, readEditor } from "./compose/recovery";
import "./styles/tokens.css";
import "./styles/app.css";

const info = await uiInfo();
initSession(info.session_store, info.bench);
const reader = /^#reader=(\d+)$/.exec(window.location.hash);
if (!reader) await connectEditors().catch(e => notify("Editor recovery unavailable: " + e, true));
document.documentElement.dataset.platform = info.platform;
const request = composeSpec();
const editorId = request?.kind === "resume" ? request.editor : request ? crypto.randomUUID() : undefined;
const recovered = request?.kind === "resume" ? await readEditor(request.editor) : undefined;
const spec = request?.kind === "resume" ? recovered?.spec : request;
const mainWindow = !request && !reader;
// First run: until an account is signed in, the main window shows Welcome.
const onboarding = mainWindow && !info.bench
  ? await onboardingState().catch((e) => { notify("Couldn't check the setup: " + e, true); return null; })
  : null;
const welcome = onboarding && onboarding.stage !== "ready" ? onboarding : null;
const active = readSession<string>("active");
const windowKey = editorId ?? (reader ? "reader/" + reader[1] : "main");
const rememberFocus = () => writeSession("active", windowKey);
window.addEventListener("focus", rememberFocus);
if (document.hasFocus()) rememberFocus();
if (mainWindow && !info.bench && !info.config_error) {
  await importPreferences().catch(e => notify("Couldn't import desktop settings: " + e, true));
}
await initPreferences();
initTheme();
if (info.platform !== "macos") followWindowIcon();
if (mainWindow) {
  installComposeKeys();
  await getCurrentWindow().onCloseRequested(async e => {
    try { await Promise.all([flushEditorWrites(), flushPreferences()]); }
    catch (err) { e.preventDefault(); notify("Couldn't finish saving the session: " + err, true); }
  });
}
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
  ) : welcome ? <Welcome platform={info.platform} state={welcome} /> : mainWindow ? <App info={info} /> : <p>This editor's working copy is unavailable.</p>,
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
if (mainWindow && !info.bench && !welcome) void reopenWindows().catch(e => notify("Couldn't reopen a window: " + e, true));
if (mainWindow && info.bench) {
  // A bench that fails still reports, so the harness shows why.
  void import("./bench")
    .then((b) => b.runBench())
    .catch((e) => import("@tauri-apps/api/core").then((c) => c.invoke("bench_report", { report: { error: String(e) } })));
}
