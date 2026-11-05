// The compose window's IPC beyond `run`: identities and signatures from
// config.toml, and files dropped or picked (see crates/gui/src/compose.rs).
import { invoke } from "@tauri-apps/api/core";

export type Identity = {
  key: string;
  email: string;
  display_name: string | null;
  signature_new: string | null;
  signature_reply: string | null;
};

export type ComposeInfo = {
  identities: Identity[];
  send_delay_seconds: number;
  config_error: string | null;
};

export type FileInfo = {
  path: string;
  name: string;
  size: number;
  /** A `data:` URI when the file is an image that can go in the body. */
  image: string | null;
  /** Why it can't be attached, if it can't. */
  problem: string | null;
};

export const composeInfo = (): Promise<ComposeInfo> => invoke<ComposeInfo>("compose_info");
export const fileInfo = (paths: string[]): Promise<FileInfo[]> => invoke<FileInfo[]>("file_info", { paths });
export const pickFiles = (): Promise<FileInfo[]> => invoke<FileInfo[]>("pick_files");
