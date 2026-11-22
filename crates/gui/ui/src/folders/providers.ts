// Which accounts are Gmail, for the few places the folder pane words things
// differently for them (a Gmail badge, deleting a label).
import { invoke } from "@tauri-apps/api/core";
import { createResource, createRoot } from "solid-js";

type Account = { name: string; provider: "microsoft" | "gmail" };

const gmail = createRoot(() => {
  const [state] = createResource(() =>
    invoke<{ accounts: Account[] }>("onboarding_state").catch(() => ({ accounts: [] as Account[] })),
  );
  return () => new Set((state()?.accounts ?? []).filter((a) => a.provider === "gmail").map((a) => a.name));
});

/** Whether the account named `account` in config.toml is a Gmail account. */
export const isGmail = (account: string) => gmail().has(account);
