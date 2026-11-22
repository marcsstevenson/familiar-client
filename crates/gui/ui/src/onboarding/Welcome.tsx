// First run (onboarding WP OB.3). The main window shows this instead of the mail
// app until an account is signed in: Welcome adds the first account, Welcome back
// signs in the accounts a restored config.toml already names, and an invalid config
// shows its problems without ever being overwritten.
import { invoke } from "@tauri-apps/api/core";
import { For, Match, Show, Switch, createSignal, onCleanup, onMount } from "solid-js";
import { isCommandError, run } from "../api";
import { TitleBar } from "../TitleBar";
import keaUrl from "../assets/kea.svg";
import "./welcome.css";

export type Stage = "no_config" | "invalid" | "no_accounts" | "signed_out" | "ready";

export type OnboardingState = {
  stage: Stage;
  config_path: string;
  errors: string[];
  accounts: { name: string; email: string; provider: "microsoft" | "gmail"; signed_in: boolean }[];
};

export const onboardingState = (): Promise<OnboardingState> => invoke<OnboardingState>("onboarding_state");

type Provider = "microsoft" | "gmail";

/** Familiar's own Microsoft app ID, for the admin consent link. */
const CLIENT_ID = "c6057b6a-a692-47dd-b34e-12629372e32a";

/** What went wrong, in words a new person can act on. */
function explain(e: unknown, provider: Provider): { text: string; admin?: boolean } {
  const message = isCommandError(e) ? e.message : String(e);
  if (/AADSTS(90094|65001|90095)|admin (approval|consent)|need admin/i.test(message))
    return { text: "Your organisation needs an administrator to approve Familiar before you can sign in.", admin: true };
  if (/admin_policy_enforced|org_internal|blocked by your admin/i.test(message))
    return { text: "Your organisation's Google admin needs to allow Familiar before you can sign in.", admin: true };
  if (/already in Familiar as/.test(message)) return { text: message.replace(/;.*/, ".") };
  if (isCommandError(e) && e.kind === "offline")
    return { text: provider === "gmail" ? "Can't reach Google. Check your connection and try again." : "Can't reach Microsoft. Check your connection and try again." };
  if (/scope|permission|insufficient/i.test(message) && provider === "gmail")
    return { text: "Familiar needs every box ticked on Google's page to read and send your mail. Try again and tick them all." };
  if (/timed out|timeout|cancel|closed|access_denied/i.test(message))
    return { text: "Sign-in didn't finish. Try again when you're ready." };
  return { text: "Sign-in didn't work: " + message };
}

function adminRequest(provider: Provider) {
  return provider === "gmail"
    ? "Hello,\n\nI'd like to use Familiar, an open-source mail and calendar app, with my Google account. " +
        "Could you allow its OAuth client in the Google Admin console (Security, API controls, App access control)?\n\n" +
        "Client ID: see Familiar's sign-in page\n\nThank you."
    : "Hello,\n\nI'd like to use Familiar, an open-source mail and calendar app that talks only to Microsoft from my computer. " +
        "It needs an administrator to approve it once. You can do that here:\n\n" +
        `https://login.microsoftonline.com/common/adminconsent?client_id=${CLIENT_ID}\n\n` +
        "It asks for mail, calendar and contacts access for the signed-in user only.\n\nThank you.";
}

export function Welcome(props: { platform: string; state: OnboardingState }) {
  const [busy, setBusy] = createSignal<Provider | string | null>(null);
  const [error, setError] = createSignal<{ text: string; admin?: boolean; provider: Provider } | null>(null);
  const [copied, setCopied] = createSignal(false);
  const [state, setState] = createSignal(props.state);

  // The person may fix the config in an editor: look again when the window gets focus.
  const recheck = () => {
    if (busy()) return;
    void onboardingState().then((s) => (s.stage === "ready" ? restart() : setState(s)));
  };
  onMount(() => window.addEventListener("focus", recheck));
  onCleanup(() => window.removeEventListener("focus", recheck));

  /** Restarts famd and the app, so both read the new config. */
  const restart = () => invoke("restart_app");

  async function add(provider: Provider) {
    setError(null);
    setBusy(provider);
    try {
      await run("account.add", { provider, device_code: false });
      await restart();
    } catch (e) {
      setError({ ...explain(e, provider), provider });
      setBusy(null);
    }
  }

  async function signIn(account: string, provider: Provider) {
    setError(null);
    setBusy(account);
    try {
      await run("auth.login", { account, device_code: false });
      await restart();
    } catch (e) {
      setError({ ...explain(e, provider), provider });
      setBusy(null);
    }
  }

  async function copyRequest(provider: Provider) {
    await navigator.clipboard.writeText(adminRequest(provider));
    setCopied(true);
  }

  const waiting = () =>
    busy() === "gmail"
      ? <p class="welcome-wait">Finish signing in in your browser. Google will say it hasn't verified Familiar: choose <b>Advanced</b>, then <b>Go to Familiar</b>. Familiar is open source and talks only to Google, from this computer.</p>
      : <p class="welcome-wait">Finish signing in in your browser, then come back here.</p>;

  const choices = () => (
    <>
      <div class="welcome-choices">
        <button type="button" class="welcome-choice" disabled={!!busy()} onClick={() => void add("microsoft")}>
          <span class="welcome-choice-title">Microsoft</span>
          <span class="welcome-choice-hint">Outlook.com, Hotmail, Live, or a work or school account</span>
        </button>
        <button type="button" class="welcome-choice" disabled={!!busy()} onClick={() => void add("gmail")}>
          <span class="welcome-choice-title">Gmail</span>
          <span class="welcome-choice-hint">A Google account</span>
        </button>
      </div>
      <p class="welcome-note">Address ends in @gmail.com? Choose Gmail. Hotmail, Outlook or a work address? Choose Microsoft.</p>
    </>
  );

  return (
    <div class="app welcome-app">
      <TitleBar platform={props.platform} title="Familiar" />
      <main class="welcome">
        <img class="welcome-icon" src={keaUrl} alt="" />
        <Switch>
          <Match when={state().stage === "invalid"}>
            <h1>Your config has problems</h1>
            <p>Familiar won't change it. Fix these in <code>{state().config_path}</code>, then come back.</p>
            <ul class="welcome-errors">
              <For each={state().errors}>{(e) => <li><code>{e}</code></li>}</For>
            </ul>
            <div class="welcome-actions">
              <button type="button" class="primary" onClick={() => void invoke("open_config")}>Open config</button>
              <button type="button" onClick={recheck}>Check again</button>
            </div>
          </Match>
          <Match when={state().stage === "signed_out"}>
            <h1>Welcome back</h1>
            <p>Your config names these accounts. Sign in to each one you want on this computer.</p>
            <ul class="welcome-accounts">
              <For each={state().accounts}>
                {(a) => (
                  <li>
                    <span><b>{a.name}</b> · {a.email}</span>
                    <button type="button" class="primary" disabled={!!busy()} onClick={() => void signIn(a.name, a.provider)}>
                      {busy() === a.name ? "Signing in…" : "Sign in"}
                    </button>
                  </li>
                )}
              </For>
            </ul>
            <Show when={busy()}>{waiting()}</Show>
          </Match>
          <Match when={true}>
            <h1>Welcome to Familiar</h1>
            <p>Fast, local-first mail and calendar. Your mail stays on this computer and goes straight to your provider, with no server in between.</p>
            <h2>Add your first account</h2>
            {choices()}
            <Show when={busy()}>{waiting()}</Show>
            <p class="welcome-small">
              Already have a config.toml? Put it at <code>{state().config_path}</code> and Familiar picks it up.
              From a terminal: <code>fam account add</code>.
            </p>
          </Match>
        </Switch>
        <Show when={error()}>
          {(e) => (
            <div class="welcome-error" role="alert">
              <p>{e().text}</p>
              <Show when={e().admin}>
                <button type="button" onClick={() => void copyRequest(e().provider)}>
                  {copied() ? "Copied" : "Copy a request for your IT admin"}
                </button>
              </Show>
            </div>
          )}
        </Show>
      </main>
    </div>
  );
}
