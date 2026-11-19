import { render } from "solid-js/web";
import { Show } from "solid-js";
import { uiInfo } from "./api";
import { App } from "./App";
import { ComposeWindow, composeSpec, installComposeKeys } from "./compose";
import { initTheme } from "./theme";
import { ReadingPane } from "./reading/ReadingPane";
import { SpamDialog } from "./spam/SpamDialog";
import { setSelectedMessage, setSpamDialog, spamDialog, spamSettings } from "./state";
import "./styles/tokens.css";
import "./styles/app.css";

const info = await uiInfo();
document.documentElement.dataset.platform = info.platform;
const spec = composeSpec();
const reader = /^#reader=(\d+)$/.exec(window.location.hash);
const mainWindow = !spec && !reader;
initTheme(info.theme, mainWindow);
if (mainWindow) installComposeKeys();
if (reader) {
  setSelectedMessage(Number(reader[1]));
  spamSettings.accounts = info.spam_accounts;
}
render(
  () => spec ? <ComposeWindow info={info} spec={spec} /> : reader ? (
    <main class="reader-window">
      <ReadingPane standalone />
      <Show when={spamDialog()}>
        {(d) => <SpamDialog message={d().message} saved={d().saved} onClose={() => setSpamDialog(null)} />}
      </Show>
    </main>
  ) : <App info={info} />,
  document.getElementById("root")!,
);
if (mainWindow && info.bench) {
  // A bench that fails still reports, so the harness shows why.
  void import("./bench")
    .then((b) => b.runBench())
    .catch((e) => import("@tauri-apps/api/core").then((c) => c.invoke("bench_report", { report: { error: String(e) } })));
}
