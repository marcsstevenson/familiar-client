// Which accounts are Gmail, for the few places the folder pane words things
// differently for them (deleting a label, the account name's tooltip).
import { invoke } from "@tauri-apps/api/core";
import { createResource, createRoot } from "solid-js";

type Account = { name: string; provider: "microsoft" | "gmail" };

const providers = createRoot(() => {
  const [state] = createResource(() =>
    invoke<{ accounts: Account[] }>("onboarding_state").catch(() => ({ accounts: [] as Account[] })),
  );
  return () => new Map((state()?.accounts ?? []).map((a) => [a.name, a.provider]));
});

/** Whether the account named `account` in config.toml is a Gmail account. */
export const isGmail = (account: string) => providers().get(account) === "gmail";

/** A tooltip naming the account's provider, or undefined while unknown. */
export const providerTitle = (account: string) => {
  const p = providers().get(account);
  if (p === "gmail") return "Gmail account: its folders are Gmail labels";
  return p === "microsoft" ? "Microsoft account" : undefined;
};
