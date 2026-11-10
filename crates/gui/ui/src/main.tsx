import { render } from "solid-js/web";
import { uiInfo } from "./api";
import { App } from "./App";
import { ComposeWindow, composeSpec, installComposeKeys } from "./compose";
import { initTheme } from "./theme";
import "./styles/tokens.css";
import "./styles/app.css";

const info = await uiInfo();
document.documentElement.dataset.platform = info.platform;
const spec = composeSpec();
initTheme(info.theme, !spec);
if (!spec) installComposeKeys();
render(
  () => (spec ? <ComposeWindow info={info} spec={spec} /> : <App info={info} />),
  document.getElementById("root")!,
);
if (!spec && info.bench) {
  // A bench that fails still reports, so the harness shows why.
  void import("./bench")
    .then((b) => b.runBench())
    .catch((e) => import("@tauri-apps/api/core").then((c) => c.invoke("bench_report", { report: { error: String(e) } })));
}
