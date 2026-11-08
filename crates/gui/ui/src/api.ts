// The front end's only way into the engine. `run` calls a command from the
// registry, the same one `fam` and the MCP server use, with types generated
// from it (bindings/commands.ts).
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { CommandError, CommandName, Commands } from "../../../../bindings/commands";

export type { CommandError, CommandName, Commands };

/** Runs a command. Rejects with a {@link CommandError}. */
export function run<K extends CommandName>(
  name: K,
  input: Commands[K]["input"],
): Promise<Commands[K]["output"]> {
  return invoke<Commands[K]["output"]>("run", { name, input });
}

/** True if `e` is an error a command returned (rather than an IPC failure). */
export function isCommandError(e: unknown): e is CommandError {
  return typeof e === "object" && e !== null && "kind" in e && "message" in e;
}

export type Theme = "system" | "light" | "dark";
export type Platform = "windows" | "macos" | "linux";

export type UiInfo = {
  version: string;
  platform: Platform | string;
  theme: Theme;
  config_error: string | null;
  /** config.toml's [keys]: action ID to key. */
  keys: Record<string, string>;
  /** config.toml's [[views]]: saved searches. */
  views: { name: string; query: string }[];
  /** config.toml's processed_folder, by account key. */
  processed: Record<string, string>;
  /** Run the timing script (FAMILIAR_BENCH_OUT is set). */
  bench: boolean;
};

export const uiInfo = (): Promise<UiInfo> => invoke<UiInfo>("ui_info");

export type DaemonState =
  | { state: "starting" }
  | { state: "running"; started_by_app: boolean }
  | { state: "unavailable"; message: string };

/** Calls `f` with famd's state now and whenever it changes. */
export async function watchDaemon(f: (s: DaemonState) => void): Promise<() => void> {
  const stop = await listen<DaemonState>("daemon", (e) => f(e.payload));
  f(await invoke<DaemonState>("daemon_state"));
  return stop;
}
