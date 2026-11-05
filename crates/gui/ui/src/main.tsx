import { render } from "solid-js/web";
import { uiInfo } from "./api";
import { App } from "./App";
import { ComposeWindow, composeSpec, installComposeKeys } from "./compose";
import { setTheme } from "./theme";
import "./styles/tokens.css";
import "./styles/app.css";

const info = await uiInfo();
setTheme(info.theme);
document.documentElement.dataset.platform = info.platform;
const spec = composeSpec();
if (!spec) installComposeKeys();
render(
  () => (spec ? <ComposeWindow info={info} spec={spec} /> : <App info={info} />),
  document.getElementById("root")!,
);
if (!spec && info.bench) void import("./bench").then((b) => b.runBench());
