// SPDX-License-Identifier: MIT OR Apache-2.0
// Generated from the Familiar command registry by `fam schema --format typescript`.
// Do not edit. Regenerate with: FAMILIAR_BLESS=1 cargo test -p familiar-cli --test parity

/** Stable error categories. */
export type ErrorKind =
  | "internal"
  | "usage"
  | "config"
  | "auth_required"
  | "permission_denied"
  | "not_found"
  | "offline"
  | "throttled"
  | "conflict"
  | "confirmation_required"
  | "daemon_unavailable"
  | "server";

/** Process exit code for each error kind. 0 is success. */
export const EXIT_CODES: Readonly<Record<ErrorKind, number>> = {
  internal: 1,
  usage: 2,
  config: 3,
  auth_required: 4,
  permission_denied: 5,
  not_found: 6,
  offline: 7,
  throttled: 8,
  conflict: 9,
  confirmation_required: 10,
  daemon_unavailable: 11,
  server: 12,
};

/** An error returned by a command. */
export type CommandError = {
  kind: ErrorKind;
  message: string;
};

export type Access = "read" | "draft" | "write" | "send" | "delete" | "rules";

export type AccountSignIn = {
  /** When the current access token expires (RFC 3339, UTC). */
  access_expires_at?: string | null;
  /** The account name. */
  account: string;
  /** The authority used to sign in (for a signed-out account, the tenant in `config.toml`). */
  authority: string;
  /** Why a check failed. */
  detail?: string | null;
  /** The display name. */
  display_name?: string | null;
  /** The authority refreshes go to: `consumers` or the tenant ID. */
  refresh_authority?: string | null;
  /** When the tokens were last refreshed (RFC 3339, UTC). */
  refreshed_at?: string | null;
  /** Granted scopes. */
  scopes: Array<string>;
  /** When the account signed in (RFC 3339, UTC). */
  signed_in_at?: string | null;
  /** The state. */
  state: SignInState;
  /** The account's tenant ID. */
  tenant_id?: string | null;
  /** The sign-in name, usually the email address. */
  username?: string | null;
};

export type AuthLoginInput = {
  /** The account name, for example `hotmail` (lowercase letters, digits, - and _). */
  account: string;
  /** Where to sign in: `consumers` for Hotmail and Outlook.com, `organizations` or the tenant ID or domain for a work account, or `common`. Defaults to the account's `tenant` in config.toml. */
  authority?: string | null;
  /** The app (client) ID. Defaults to `[app] client_id` in config.toml, then Familiar's. */
  client_id?: string | null;
  /** Use a device code (enter it at microsoft.com/devicelogin on any device) instead of opening a browser here. */
  device_code?: boolean;
  /** The address to sign in as. Defaults to the account's `email` in config.toml. Sent as a hint to the sign-in page; a sign-in as anyone else is refused. */
  email?: string | null;
};

export type AuthLogoutInput = {
  /** The account to sign out. */
  account?: string | null;
  /** Sign out every account. */
  all?: boolean;
};

export type AuthLogoutOutput = {
  /** Accounts whose tokens were removed. Empty if none were signed in. */
  removed: Array<string>;
};

export type AuthStatusInput = {
  /** Only this account. With --check, a failed check is an error (exit code 4 or 7). */
  account?: string | null;
  /** Prove each sign-in works: refresh the token if needed and call Graph /me. */
  check?: boolean;
};

export type AuthStatusOutput = {
  /** Signed-in accounts and accounts in config.toml, by name. */
  accounts: Array<AccountSignIn>;
  /** Where tokens are kept: `keychain` or `file`. */
  store: string;
};

export type CommandDoc = {
  /** What the command may do. */
  access: Access;
  /** The CLI form, for example `fam mail list`. */
  cli: string;
  /** Longer help. */
  description: string;
  /** Error kinds the command can return. */
  errors: Array<ErrorKind>;
  /** JSON schema of the input. */
  input: unknown;
  /** The MCP tool name, for example `mail_list`. */
  mcp_tool: string;
  /** Dotted name, for example `mail.list`. */
  name: string;
  /** JSON schema of the `--json` output. */
  output: unknown;
  /** One line. */
  summary: string;
};

export type ConfigAccount = {
  /** The mailbox address. */
  email: string;
  /** The account name used with `--account`. */
  name: string;
  /** Whether new and reply signatures are set. */
  signatures: SignatureState;
  /** The sign-in tenant. */
  tenant: string;
};

export type ConfigPathInput = {
  /** The config file. Defaults to FAMILIAR_CONFIG, then the platform default (see `fam config path`). */
  path?: string | null;
};

export type ConfigPathOutput = {
  /** Whether the file exists. */
  exists: boolean;
  /** The config file Familiar uses. */
  path: string;
  /** Where the path came from. */
  source: ConfigPathSource;
};

export type ConfigPathSource = "explicit" | "env" | "default";

export type ConfigValidateOutput = {
  /** Accounts, by name. */
  accounts: Array<ConfigAccount>;
  /** The Microsoft app (client) ID in use. */
  client_id: string;
  /** The file that was checked. */
  path: string;
  /** The config file format version. */
  version: number;
  /** Problems that don't stop the config being used. */
  warnings: Array<ConfigWarning>;
};

export type ConfigWarning = {
  /** 1-based column, when known. */
  column?: number | null;
  /** Dotted key path, for example `accounts.parklands.tenant`. */
  key: string;
  /** 1-based line, when known. */
  line?: number | null;
  /** What looks wrong. */
  message: string;
};

export type ExitCodeDoc = {
  /** The process exit code. */
  code: number;
  /** What it means. */
  description: string;
  /** The error kind, or none for success. */
  kind?: ErrorKind | null;
};

export type McpAnnotations = {
  /** The tool may delete or send. */
  destructiveHint: boolean;
  /** The tool changes nothing. */
  readOnlyHint: boolean;
};

export type McpTool = {
  /** Behaviour hints for the client. */
  annotations: McpAnnotations;
  /** What the tool does. */
  description: string;
  /** JSON schema of the arguments. */
  inputSchema: unknown;
  /** Tool name, the command path joined with `_`. */
  name: string;
  /** JSON schema of the structured result, when it is an object. */
  outputSchema?: unknown;
  /** Short title. */
  title: string;
};

export type McpToolList = {
  /** One tool per command. */
  tools: Array<McpTool>;
};

export type SchemaDocument = {
  /** Every command, by name. */
  commands: Array<CommandDoc>;
  /** Every exit code `fam` can return. */
  exit_codes: Array<ExitCodeDoc>;
  /** The Familiar version that produced it. */
  familiar_version: string;
  /** The format version of this document. */
  schema_version: number;
};

export type SchemaFormat = "commands" | "mcp" | "typescript";

export type SchemaInput = {
  /** What to describe. */
  format?: SchemaFormat;
};

export type SchemaOutput = SchemaDocument | McpToolList | {
  /** The TypeScript source. */
  typescript: string;
};

export type SignInState = "signed_in" | "signed_out" | "ok" | "auth_required" | "offline" | "error";

export type SignatureState = {
  /** A signature for new messages is set. */
  new: boolean;
  /** A signature for replies is set. */
  reply: boolean;
};

export type VersionInput = {
  /** Also report the operating system and CPU architecture. */
  verbose?: boolean;
};

export type VersionOutput = {
  /** The CPU architecture, with `--verbose`. */
  arch?: string | null;
  /** The operating system, with `--verbose`. */
  os?: string | null;
  /** The version of the command schema (`fam schema --json`). */
  schema_version: number;
  /** The Familiar version. */
  version: string;
};

/** Every command, by dotted name, with its input and output types. */
export interface Commands {
  /** Sign in to a Microsoft account */
  "auth.login": { input: AuthLoginInput; output: AccountSignIn };
  /** Sign out: forget an account's tokens */
  "auth.logout": { input: AuthLogoutInput; output: AuthLogoutOutput };
  /** Show which accounts are signed in */
  "auth.status": { input: AuthStatusInput; output: AuthStatusOutput };
  /** Show which config file is used */
  "config.path": { input: ConfigPathInput; output: ConfigPathOutput };
  /** Check config.toml for errors */
  "config.validate": { input: ConfigPathInput; output: ConfigValidateOutput };
  /** Describe every command, its input, output and errors */
  "schema": { input: SchemaInput; output: SchemaOutput };
  /** Show the Familiar version */
  "version": { input: VersionInput; output: VersionOutput };
}

/** A command name. */
export type CommandName = keyof Commands;

/** Every command name, in order. */
export const COMMAND_NAMES = [
  "auth.login",
  "auth.logout",
  "auth.status",
  "config.path",
  "config.validate",
  "schema",
  "version",
] as const satisfies readonly CommandName[];
