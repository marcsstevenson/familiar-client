import { render } from "solid-js/web";
import { uiInfo } from "./api";
import { App } from "./App";
import { setTheme } from "./theme";
import "./styles/tokens.css";
import "./styles/app.css";

const info = await uiInfo();
setTheme(info.theme);
document.documentElement.dataset.platform = info.platform;
render(() => <App info={info} />, document.getElementById("root")!);
