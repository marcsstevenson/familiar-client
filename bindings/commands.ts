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

export type AccountSyncStatus = {
  /** The account name. */
  account: string;
  /** Messages whose body is on this device. */
  bodies: number;
  /** Messages inside the sync window still waiting for a body. */
  bodies_missing: number;
  /** Bodies in the on-demand cache (older than the window). */
  cached: number;
  /** Compressed bytes in the on-demand cache. */
  cached_bytes: number;
  /** The daemon's last error for the account, while it lasts. */
  detail?: string | null;
  /** The mailbox address. */
  email: string;
  /** Folders whose last sync failed. */
  errors: Array<SyncError>;
  /** Whether the first sync of headers for all history has finished. */
  first_sync_done: boolean;
  /** Folders. */
  folders: number;
  /** When a folder last synced (RFC 3339, UTC). */
  last_synced_at?: string | null;
  /** Messages (headers) on this device. */
  messages: number;
  /** When the next poll is due (RFC 3339, UTC), from the daemon. */
  next_poll_at?: string | null;
  /** What the daemon is doing; `stopped` without a daemon. */
  phase: SyncPhase;
  /** Pinned bodies. */
  pinned: number;
  /** Unread messages. */
  unread: number;
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

export type DaemonInfo = {
  /** Its process ID. */
  pid?: number | null;
  /** Whether `famd` answered. */
  running: boolean;
  /** When it started (RFC 3339, UTC). */
  started_at?: string | null;
  /** Its version. */
  version?: string | null;
};

export type ExitCodeDoc = {
  /** The process exit code. */
  code: number;
  /** What it means. */
  description: string;
  /** The error kind, or none for success. */
  kind?: ErrorKind | null;
};

export type HitSource = "local" | "online";

export type MailPinInput = {
  /** The message's local ID. */
  message: number;
  /** Unpin instead: the body becomes an ordinary cached body again. */
  unpin?: boolean;
};

export type MailSearchInput = {
  /** The next-page token from a previous search. */
  after?: string | null;
  /** Most results per page (default 50, at most 500). */
  limit?: number | null;
  /** The query, for example 'from:club subject:"AGM" has:attachment after:2026-01-01'. Operators: from: to: subject: body: has:attachment before: after: folder: account: is:unread is:read is:flagged, "phrases" and -exclusions. */
  query: string;
  /** local (default) searches this device; all also searches message text on the server. */
  scope?: SearchScope;
};

export type MailSearchOutput = {
  /** Results, newest first. Server-only results are labelled `online`. */
  hits: Array<SearchHit>;
  /** Pass as `--after` for the next page of local results. */
  next?: string | null;
  /** Things to know about these results, such as text that wasn't searched. */
  notices: Array<string>;
  /** What happened to the server search. */
  online: OnlineStatus;
  /** How many results are `online`. */
  online_hits: number;
  /** The query as given. */
  query: string;
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

export type OnlineStatus = "not_requested" | "not_needed" | "searched" | "offline" | "daemon_unavailable" | "failed";

export type PinResult = {
  /** Whether the body was downloaded to pin it. */
  fetched: boolean;
  /** Whether the body is on this device now. */
  has_body: boolean;
  /** Local message ID. */
  message: number;
  /** Whether the body is now pinned. */
  pinned: boolean;
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

export type SearchHit = {
  /** Account name. */
  account: string;
  /** Whether the body is on this device. */
  body_local: boolean;
  /** Folder name, when known. */
  folder?: string | null;
  /** Sender address. */
  from_addr: string;
  /** Sender name. */
  from_name: string;
  /** Has file attachments. */
  has_attachments: boolean;
  /** Local message ID, when its header is on this device. */
  id?: number | null;
  /** Flagged state. */
  is_flagged: boolean;
  /** Read state. */
  is_read: boolean;
  /** Short text preview. */
  preview: string;
  /** Received time (RFC 3339, UTC). */
  received_at: string;
  /** Graph immutable ID. */
  remote_id: string;
  /** `local`, or `online` for a result only the server found. */
  source: HitSource;
  /** Subject. */
  subject: string;
};

export type SearchScope = "local" | "all";

export type SignInState = "signed_in" | "signed_out" | "ok" | "auth_required" | "offline" | "error";

export type SignatureState = {
  /** A signature for new messages is set. */
  new: boolean;
  /** A signature for replies is set. */
  reply: boolean;
};

export type SyncError = {
  /** The error. */
  message: string;
  /** The resource: `folders`, or `messages:<folder>`. */
  resource: string;
};

export type SyncPhase = "starting" | "folder_tree" | "headers" | "bodies" | "idle" | "backoff" | "auth_required" | "stopped";

export type SyncStatusInput = {
  /** Only this account (its name in config.toml). */
  account?: string | null;
};

export type SyncStatusOutput = {
  /** One entry per account in the store. */
  accounts: Array<AccountSyncStatus>;
  /** Compressed bytes of every body on this device. */
  body_bytes: number;
  /** The daemon. */
  daemon: DaemonInfo;
  /** The store directory. */
  store: string;
  /** The body window from config.toml (`sync.window`). */
  window: string;
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
  /** Keep a message's body on this device */
  "mail.pin": { input: MailPinInput; output: PinResult };
  /** Search mail across all accounts */
  "mail.search": { input: MailSearchInput; output: MailSearchOutput };
  /** Describe every command, its input, output and errors */
  "schema": { input: SchemaInput; output: SchemaOutput };
  /** Show how far each account's sync has got */
  "sync.status": { input: SyncStatusInput; output: SyncStatusOutput };
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
  "mail.pin",
  "mail.search",
  "schema",
  "sync.status",
  "version",
] as const satisfies readonly CommandName[];
