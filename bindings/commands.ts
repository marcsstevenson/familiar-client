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

export type AccountCheck = {
  /** The account's key in `config.toml`. */
  account: string;
  /** Remaining differences by kind. */
  by_kind: Array<DriftCount>;
  /** The first differences (up to the limit). */
  drift: Array<Drift>;
  /** Total differences that remain. */
  drift_total: number;
  /** Whether the first sync had finished. Before it, drift is expected. */
  first_sync_done: boolean;
  /** Folders compared. */
  folders: number;
  /** Messages in the store (inside the kept header range). */
  local_messages: number;
  /** Seconds the check took. */
  seconds: number;
  /** Messages listed on the server (inside the kept header range). */
  server_messages: number;
  /** Whether a sync round ran before the comparison and before re-checking. */
  settled: boolean;
  /** Differences that the first pass found but were gone when re-checked: changes that were still syncing. */
  settled_on_recheck: number;
};

export type AccountGrants = {
  /** Account name in config.toml. */
  account: string;
  /** Access levels granted: read, draft, write, send, delete, rules. */
  allow: Array<string>;
};

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

export type AgentStatusInput = Record<string, never>;

export type AgentStatusOutput = {
  /** Per account. */
  accounts: Array<AccountGrants>;
  /** True when this session's writes are checked against the grants. */
  agent: boolean;
  /** Why config.toml couldn't be read, if it couldn't; agents then get the defaults (read and draft) until it's fixed. */
  config_error?: string | null;
  /** The source its writes are logged under (`cli` for a person). */
  source: string;
};

export type AnyValue = unknown;

export type ApiInput = {
  /** The account whose token to use (its name in config.toml). May be left out when famd syncs only one account. */
  account?: string | null;
  /** HTTP method. Only GET is allowed until writes arrive in Phase 2. */
  method?: string | null;
  /** A Graph path under https://graph.microsoft.com/v1.0, such as '/me/mailFolders?$top=5', or a full link Graph returned, such as an @odata.nextLink. */
  path: string;
};

export type AttachmentInfo = {
  /** MIME type, such as `application/pdf`. */
  content_type: string;
  /** Graph attachment ID. */
  id: string;
  /** Shown inside the body (a `cid:` image) rather than in the list. */
  is_inline: boolean;
  /** File, item or reference. */
  kind: AttachmentKind;
  /** File name. */
  name: string;
  /** Where it was saved, with --save. */
  saved_to?: string | null;
  /** Size in bytes, as Graph reports it. */
  size: number;
};

export type AttachmentKind = "file" | "item" | "reference";

export type AuditListInput = {
  /** Most rows (default 50). */
  limit?: number | null;
  /** Only this outcome: queued, denied, done or failed. */
  outcome?: string | null;
  /** Only this far back: a period like 7d, 4w or 6m. Default all. */
  since?: string | null;
  /** Only this source, e.g. mcp or agent:claude-code; a trailing * matches the start (agent:*). */
  source?: string | null;
};

export type AuditListOutput = {
  /** Rows, newest first. */
  entries: Array<AuditView>;
};

export type AuditView = {
  /** Account name, if any. */
  account?: string | null;
  /** When (RFC 3339, UTC). */
  at: string;
  /** The command, e.g. mail.move. */
  command: string;
  /** The summary, or why it was refused or failed. */
  detail: string;
  /** Row ID. */
  id: number;
  /** The op it queued, if any. */
  op?: number | null;
  /** queued, denied, done or failed. */
  outcome: string;
  /** Who asked: cli, gui, mcp or agent:<name>. */
  source: string;
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

export type BodyFormat = "text" | "html";

export type BodyType = "text" | "html";

export type CalAddInput = {
  /** Account (its name in config.toml), when --calendar doesn't settle it. */
  account?: string | null;
  /** All-day event (also when --start is a date alone). */
  all_day?: boolean;
  /** Invite someone ("Name <addr>" or an address); repeat or separate with commas. */
  attendee?: Array<string>;
  /** Notes (plain text). */
  body?: string | null;
  /** Calendar name ("Family Room") or a purpose from [calendar.purposes] ("family"). Default: the account's main calendar. */
  calendar?: string | null;
  /** Outlook category; repeat for more. */
  category?: Array<string>;
  /** Or: repeat this many times. */
  count?: number | null;
  /** All-day: how many days when there's no --end (default 1). */
  days?: number | null;
  /** Only show what would happen. */
  dry_run?: boolean;
  /** End: a date and time, or a time on the start's day. All-day: the last day. */
  end?: string | null;
  /** Repeat every this many days, weeks, months or years (default 1). */
  every?: number | null;
  /** Make it from this email (local message ID): suggests the subject, date, time and place, and links the email in the event. */
  from_mail?: number | null;
  /** Location. */
  location?: string | null;
  /** Length in minutes when there's no --end (default 60). */
  minutes?: number | null;
  /** No reminder. */
  no_reminder?: boolean;
  /** Weekly: the days, such as mon,wed (default the start's day). */
  on?: Array<string>;
  /** Invite someone as optional. */
  optional?: Array<string>;
  /** A purpose from [calendar.purposes], such as family. */
  purpose?: string | null;
  /** Reminder, minutes before the start (default 15). */
  reminder?: number | null;
  /** Repeat: daily, weekly, monthly or yearly. */
  repeat?: string | null;
  /** Show as free, tentative, busy (the default), oof or workingElsewhere. */
  show_as?: string | null;
  /** Start: "2026-11-21 18:00", "sat 6pm", "tomorrow 9:30am", or a date alone for an all-day event. With --from-mail, found in the email when left out. */
  start?: string | null;
  /** Subject. With --from-mail, the email's subject when left out. */
  subject?: string | null;
  /** Time zone for the times (IANA or Windows). Default: this computer's. */
  tz?: string | null;
  /** Repeat until this date. */
  until?: string | null;
  /** Wait until the server has it (needs famd). */
  wait?: boolean;
  /** Seconds --wait waits at most (default 60). */
  wait_secs?: number | null;
  /** Go ahead with more than 10 invitees. */
  yes?: boolean;
};

export type CalCalendar = {
  /** Account name. */
  account: string;
  /** You can add and change events. */
  can_edit: boolean;
  /** The last sync error, if the last sync failed. */
  error?: string | null;
  /** Events on this device. */
  events: number;
  /** The account's default calendar. */
  is_default: boolean;
  /** When the events last synced (RFC 3339, UTC). */
  last_synced_at?: string | null;
  /** Calendar name. */
  name: string;
  /** Owner's address. */
  owner?: string;
  /** Purposes from `[calendar.purposes]` that point here. */
  purposes?: Array<string>;
  /** Start of the synced window (date), once a sync has run. */
  synced_from?: string | null;
  /** End of the synced window (date, exclusive). */
  synced_to?: string | null;
};

export type CalCalendarsInput = {
  /** Only this account (its name in config.toml). */
  account?: string | null;
};

export type CalCalendarsOutput = {
  /** Calendars, by account, the default first. */
  calendars: Array<CalCalendar>;
};

export type CalDeleteInput = {
  /** For a meeting you organised: a note in the cancellation attendees get. */
  comment?: string | null;
  /** Only show what would happen. */
  dry_run?: boolean;
  /** Local event ID (from fam cal list). */
  event: number;
  /** Delete the whole recurring series. */
  series?: boolean;
  /** Wait until the server has it (needs famd). */
  wait?: boolean;
  /** Seconds --wait waits at most (default 60). */
  wait_secs?: number | null;
  /** Go ahead with cancelling a meeting you organised (attendees are told). */
  yes?: boolean;
};

export type CalEditInput = {
  /** New attendee list (replaces it); repeat for more. */
  attendee?: Array<string>;
  /** New notes (plain text). */
  body?: string | null;
  /** New categories (replaces them); repeat for more. */
  category?: Array<string>;
  /** Only show what would change. */
  dry_run?: boolean;
  /** New end. */
  end?: string | null;
  /** Local event ID (from fam cal list). */
  event: number;
  /** New location. */
  location?: string | null;
  /** New length in minutes. */
  minutes?: number | null;
  /** Turn the reminder off. */
  no_reminder?: boolean;
  /** New reminder, minutes before the start. */
  reminder?: number | null;
  /** Change every event in its series, not just this one. */
  series?: boolean;
  /** Show as free, tentative, busy, oof or workingElsewhere. */
  show_as?: string | null;
  /** New start (as fam cal add takes it). The length stays unless --end or --minutes. */
  start?: string | null;
  /** New subject. */
  subject?: string | null;
  /** Time zone for the times (IANA or Windows). Default: this computer's. */
  tz?: string | null;
  /** Wait until the server has the change (needs famd). */
  wait?: boolean;
  /** Seconds --wait waits at most (default 60). */
  wait_secs?: number | null;
  /** Go ahead with more than 10 attendees. */
  yes?: boolean;
};

export type CalEvent = {
  /** Account name. */
  account: string;
  /** All-day event. */
  all_day: boolean;
  /** Calendar name. */
  calendar: string;
  /** Cancelled meeting. */
  cancelled: boolean;
  /** End: RFC 3339 in the output zone, or the day after the last day for all-day events. */
  end: string;
  /** Local event ID. */
  id: number;
  /** Location. */
  location?: string;
  /** Organiser's address. */
  organizer?: string;
  /** Part of a recurring series. */
  recurring: boolean;
  /** Your response, when Graph gives one. */
  response?: string | null;
  /** `free`, `tentative`, `busy`, `oof`, `workingElsewhere` or `unknown`. */
  show_as: string;
  /** Start: RFC 3339 in the output zone, or a date (YYYY-MM-DD) for all-day events. */
  start: string;
  /** Subject. */
  subject: string;
  /** The organiser's time zone (IANA), when known. */
  time_zone?: string | null;
};

export type CalListInput = {
  /** Only this account (its name in config.toml). */
  account?: string | null;
  /** A calendar's name (such as "Family Room") or a purpose from `[calendar.purposes]` (such as "family"). All calendars if left out. */
  calendar?: string | null;
  /** How many days to list, from 1 to 366. Default 7. */
  days?: number | null;
  /** First day: `today` (the default), `tomorrow`, `yesterday` or a date like 2026-11-05. */
  from?: string | null;
  /** Time zone to show times in, IANA (Pacific/Auckland) or Windows (New Zealand Standard Time). Default: this computer's. */
  tz?: string | null;
};

export type CalListOutput = {
  /** Calendars searched, as `account/calendar`. */
  calendars: Array<string>;
  /** Events, by start time. */
  events: Array<CalEvent>;
  /** Start of the range (RFC 3339). */
  from: string;
  /** The zone times are shown in (IANA). */
  time_zone: string;
  /** End of the range, exclusive (RFC 3339). */
  to: string;
  /** Problems worth knowing, such as a calendar that has not synced yet or a range outside the synced window. */
  warnings?: Array<string>;
};

export type CalProposeInput = {
  /** The email (local message ID). */
  from_mail: number;
  /** Don't ask the AI tool, even when [ai] is enabled. */
  no_ai?: boolean;
  /** Time zone to give times in (IANA or Windows). Default: this computer's. */
  tz?: string | null;
};

export type CalProposeOutput = {
  /** Its account. */
  account: string;
  /** Events to add. */
  events: Array<ProposedEvent>;
  /** Set when the email is a meeting request or cancellation; answer it with `fam cal rsvp` (or remove it with `fam cal delete`) instead. */
  invitation?: Invitation | null;
  /** The email. */
  message: number;
  /** What was skipped or guessed. */
  notes: Array<string>;
  /** Where the proposals came from: `invite`, `ics`, `ai:claude`, `ai:codex`, `heuristic` or `none`. */
  source: string;
};

export type CalRsvpInput = {
  /** A note to the organiser. */
  comment?: string | null;
  /** Only show what would happen. */
  dry_run?: boolean;
  /** Local event ID (from fam cal list). */
  event: number;
  /** Don't tell the organiser. */
  no_reply?: boolean;
  /** accept, tentative or decline. */
  response: string;
  /** Answer for the whole recurring series. */
  series?: boolean;
  /** Wait until the server has it (needs famd). */
  wait?: boolean;
  /** Seconds --wait waits at most (default 60). */
  wait_secs?: number | null;
};

export type CheckReport = {
  /** One entry per account checked. */
  accounts: Array<AccountCheck>;
  /** True when no account has drift. */
  clean: boolean;
};

export type CheckSpamInput = {
  /** The message's local ID. */
  message: number;
  /** Only the facts: don't send the email to the AI tool. */
  no_ai?: boolean;
  /** Don't check: show the result saved by an earlier check, if any. */
  saved?: boolean;
};

export type CheckSpamOutput = {
  /** Its account. */
  account: string;
  /** The AI's grade, when it was asked and answered. */
  ai?: Grade | null;
  /** When it was checked (Unix seconds). */
  checked_at: number;
  /** What Familiar found, from the mail server's headers, your mailbox, the links and the attachments. */
  facts: Array<Fact>;
  /** The folder it is in: its well-known name (`inbox`, `junkemail`, ...) or its name. */
  folder: string;
  /** The message. */
  message: number;
  /** What was skipped and why. */
  notes: Array<string>;
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

export type ContactsSearchInput = {
  /** Only this account (its name in config.toml). */
  account?: string | null;
  /** How many to show, 1 to 200. Default 20. */
  limit?: number | null;
  /** The start of a name, address or company (or of any word in a name). Empty lists the people written to most. */
  query?: string;
};

export type ContactsSearchOutput = {
  /** Best first: contacts, then people written to most, then heard from. */
  people: Array<PersonView>;
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

export type DraftCreateInput = {
  /** The account to write from (config name or address); needed with more than one. */
  account?: string | null;
  /** Attach a file (up to 3 MB); repeat for more. */
  attach?: Array<string>;
  /** Bcc. */
  bcc?: Array<string>;
  /** Body text (plain text unless --html). */
  body?: string | null;
  /** Read the body from a file. */
  body_file?: string | null;
  /** Cc. */
  cc?: Array<string>;
  /** Only show what would happen. */
  dry_run?: boolean;
  /** Send from this alias of the account (listed in `aliases` in config.toml). */
  from?: string | null;
  /** The body is HTML. */
  html?: boolean;
  /** low, normal or high. */
  importance?: string | null;
  /** Add an inline image, referenced in an HTML body as cid:<file name>. */
  inline?: Array<string>;
  /** Leave the account's signature off. */
  no_signature?: boolean;
  /** Subject. */
  subject?: string | null;
  /** To: an address or `Name <address>`; repeat, or separate with commas. */
  to?: Array<string>;
  /** Wait until the server has the draft (needs famd). */
  wait?: boolean;
  /** Seconds --wait waits at most (default 60). */
  wait_secs?: number | null;
};

export type DraftForwardInput = {
  /** Attach another file; repeat for more. */
  attach?: Array<string>;
  /** Bcc. */
  bcc?: Array<string>;
  /** Body text, above the forwarded message. */
  body?: string | null;
  /** Read the body from a file. */
  body_file?: string | null;
  /** Cc. */
  cc?: Array<string>;
  /** Only show what would happen. */
  dry_run?: boolean;
  /** Send from this alias of the account; defaults to the alias the original was sent to. */
  from?: string | null;
  /** The body is HTML. */
  html?: boolean;
  /** Local ID of the message to forward. */
  message: number;
  /** Leave the account's reply signature off. */
  no_signature?: boolean;
  /** To. */
  to?: Array<string>;
  /** Wait until the server has the draft (needs famd). */
  wait?: boolean;
  /** Seconds --wait waits at most (default 60). */
  wait_secs?: number | null;
};

export type DraftReplyInput = {
  /** Reply to everyone on it, not just the sender. */
  all?: boolean;
  /** Attach a file; repeat for more. */
  attach?: Array<string>;
  /** Bcc. */
  bcc?: Array<string>;
  /** Body text, above the quoted original (plain text unless --html). */
  body?: string | null;
  /** Read the body from a file. */
  body_file?: string | null;
  /** Cc (replaces the default with --all). */
  cc?: Array<string>;
  /** Only show what would happen. */
  dry_run?: boolean;
  /** Send from this alias of the account; defaults to the alias the original was sent to. */
  from?: string | null;
  /** The body is HTML. */
  html?: boolean;
  /** Add an inline image. */
  inline?: Array<string>;
  /** Local ID of the message to reply to. */
  message: number;
  /** Leave the account's reply signature off. */
  no_signature?: boolean;
  /** Replace the recipients (default: the sender, or everyone with --all). */
  to?: Array<string>;
  /** Wait until the server has the draft (needs famd). */
  wait?: boolean;
  /** Seconds --wait waits at most (default 60). */
  wait_secs?: number | null;
};

export type DraftSendInput = {
  /** Local ID of the draft. */
  draft: number;
  /** Only show what would happen. */
  dry_run?: boolean;
  /** Skip the undo-send delay. */
  now?: boolean;
  /** Wait until it has gone (needs famd; includes the undo-send delay). */
  wait?: boolean;
  /** Seconds --wait waits at most (default 60). */
  wait_secs?: number | null;
  /** Send even to many outside recipients who would see each other. */
  yes?: boolean;
};

export type DraftUpdateInput = {
  /** Attach a file; repeat for more. */
  attach?: Array<string>;
  /** Replace Bcc (`""` empties it). */
  bcc?: Array<string>;
  /** Replace the whole body (plain text unless --html; no signature is added). */
  body?: string | null;
  /** Replace the whole body from a file. */
  body_file?: string | null;
  /** Replace Cc (`""` empties it). */
  cc?: Array<string>;
  /** Local ID of the draft. */
  draft: number;
  /** Only show what would happen. */
  dry_run?: boolean;
  /** Send from this alias, or the account's own address. */
  from?: string | null;
  /** The body is HTML. */
  html?: boolean;
  /** Add an inline image. */
  inline?: Array<string>;
  /** Take an attachment off, by file name (or Graph attachment ID); repeat for more. */
  remove_attachment?: Array<string>;
  /** New subject. */
  subject?: string | null;
  /** Replace To (`""` empties it). */
  to?: Array<string>;
  /** Wait until the server has the change (needs famd). */
  wait?: boolean;
  /** Seconds --wait waits at most (default 60). */
  wait_secs?: number | null;
};

export type Drift = {
  /** What the store has against what the server has. */
  detail: string;
  /** The folder (its name in the store, or on the server when missing locally). */
  folder?: string | null;
  /** What differs. */
  kind: DriftKind;
  /** The message's local ID. */
  message?: number | null;
  /** The message's server ID. */
  remote_id?: string | null;
};

export type DriftCount = {
  /** How many. */
  count: number;
  /** The kind. */
  kind: DriftKind;
};

export type DriftKind = "folder_missing" | "folder_extra" | "folder_changed" | "message_missing" | "message_extra" | "wrong_folder" | "read_state" | "flag" | "received_at" | "body_missing" | "body_outside_window" | "counter";

export type ExitCodeDoc = {
  /** The process exit code. */
  code: number;
  /** What it means. */
  description: string;
  /** The error kind, or none for success. */
  kind?: ErrorKind | null;
};

export type Fact = {
  /** The fact in words. */
  detail: string;
  /** What it is: `dmarc`, `scl`, `reply_to`, `link_mismatch`, ... */
  name: string;
  /** Which way it points: `spam`, `scam`, `safe`, or empty for neither. */
  points: string;
  /** Its value, short: `fail`, `6`, `gmail.com`, ... */
  value: string;
};

export type FolderColorInput = {
  /** The account, when more than one has the folder. */
  account?: string | null;
  /** red, orange, yellow, green, teal, blue, purple, pink, grey, or none. */
  color: string;
  /** The folder (path, or name). */
  folder: string;
};

export type FolderCreateInput = {
  /** The account (config name or address); needed with more than one. */
  account?: string | null;
  /** Only show what would change. */
  dry_run?: boolean;
  /** The new folder's name. */
  name: string;
  /** Parent folder (path, well-known name or name). Default: the top level. */
  parent?: string | null;
  /** Wait until the server has the change (needs famd). */
  wait?: boolean;
  /** Seconds --wait waits at most (default 60). */
  wait_secs?: number | null;
};

export type FolderDeleteInput = {
  /** The account; needed with more than one. */
  account?: string | null;
  /** Only show what would change. */
  dry_run?: boolean;
  /** The folder (path, or name). */
  folder: string;
  /** Delete for good. Only for a folder already in Deleted Items; needs --yes. */
  permanent?: boolean;
  /** Wait until the server has the change (needs famd). */
  wait?: boolean;
  /** Seconds --wait waits at most (default 60). */
  wait_secs?: number | null;
  /** Confirm a permanent delete when not interactive. */
  yes?: boolean;
};

export type FolderListInput = {
  /** Only this account (its name in config.toml). */
  account?: string | null;
};

export type FolderListOutput = {
  /** Folders by account, each account's tree depth first, siblings by name. */
  folders: Array<FolderRow>;
};

export type FolderMoveInput = {
  /** The account; needed with more than one. */
  account?: string | null;
  /** Only show what would change. */
  dry_run?: boolean;
  /** The folder (path, or name). */
  folder: string;
  /** The new parent folder. */
  to?: string | null;
  /** Move it to the top level instead. */
  top?: boolean;
  /** Wait until the server has the change (needs famd). */
  wait?: boolean;
  /** Seconds --wait waits at most (default 60). */
  wait_secs?: number | null;
};

export type FolderPinInput = {
  /** The account, when more than one has the folder. */
  account?: string | null;
  /** The folder (path, or name). */
  folder: string;
};

export type FolderRenameInput = {
  /** The account; needed with more than one. */
  account?: string | null;
  /** Only show what would change. */
  dry_run?: boolean;
  /** The folder (path, or name). */
  folder: string;
  /** Its new name. */
  name: string;
  /** Wait until the server has the change (needs famd). */
  wait?: boolean;
  /** Seconds --wait waits at most (default 60). */
  wait_secs?: number | null;
};

export type FolderRow = {
  /** Account name. */
  account: string;
  /** Its colour (`fam folder color`), on this device only. */
  color?: string | null;
  /** How deep it is: 0 at the top level. */
  depth: number;
  /** Local folder ID. */
  id: number;
  /** Display name. */
  name: string;
  /** Parent folder's local ID; absent at the top level. */
  parent?: number | null;
  /** Full path from the top of the mailbox, such as `Inbox/Club`. Accepted by `fam mail list --folder`. */
  path: string;
  /** Pinned to the top of the folder pane (`fam folder pin`), on this device only. */
  pinned?: boolean;
  /** Graph folder ID. */
  remote_id: string;
  /** Messages in the folder on this device. */
  total: number;
  /** Unread messages in the folder. */
  unread: number;
  /** Graph well-known name (`inbox`, `sentitems`, `deleteditems`, ...), if any. */
  well_known?: string | null;
};

export type Grade = {
  /** What to do, when there is something to say. */
  advice?: string | null;
  /** `legitimate`, `marketing`, `spam`, `phishing`, `scam`, `malware` or `unsure`. */
  category: string;
  /** The grade in words: `Looks fine`, `Be careful`, `Likely spam` or `Likely scam`, from the higher of the two. */
  headline: string;
  /** Up to five reasons, quoting the email. */
  reasons: Array<Reason>;
  /** How likely it is a scam (phishing, fraud, malware, impersonation), 0 to 100. */
  scam: number;
  /** How likely it is unwanted bulk or unsolicited mail, 0 to 100. */
  spam: number;
  /** One sentence on what the email is. */
  summary: string;
  /** `claude` or `codex`. */
  tool: string;
};

export type HitSource = "local" | "online";

export type Importance = "low" | "normal" | "high";

export type Invitation = {
  /** The calendar it is in, when synced. */
  calendar?: string | null;
  /** The meeting's local event ID, once it has synced to this device. */
  event?: number | null;
  /** `request` or `cancelled`. */
  kind: string;
  /** Your answer so far, when synced. */
  response?: string | null;
  /** Its subject, when synced. */
  subject?: string | null;
  /** When it is, in words, when synced. */
  when?: string | null;
};

export type ListedFolder = {
  /** Account name. */
  account: string;
  /** Folder path. */
  folder: string;
  /** Local folder ID. */
  folder_id: number;
  /** Messages in the folder. */
  total: number;
  /** Unread messages in the folder. */
  unread: number;
};

export type MailAddress = {
  /** Email address. */
  addr: string;
  /** Display name, possibly empty. */
  name: string;
};

export type MailArchiveInput = {
  /** Only show what would change. */
  dry_run?: boolean;
  /** Local message IDs. */
  messages: Array<number>;
  /** Wait until the server has the change (needs famd). */
  wait?: boolean;
  /** Seconds --wait waits at most (default 60). */
  wait_secs?: number | null;
};

export type MailAttachmentsInput = {
  /** Include inline images (the ones shown inside the body). */
  inline?: boolean;
  /** The message's local ID. */
  message: number;
  /** Only the attachment with this name or ID (with --save). */
  name?: string | null;
  /** Save the attachments into this folder. A file that already exists is kept and the new one gets a number, like `report (2).pdf`. */
  save?: string | null;
};

export type MailAttachmentsOutput = {
  /** The attachments, in the order Graph lists them. */
  attachments: Array<AttachmentInfo>;
  /** The message's local ID. */
  message: number;
};

export type MailDeleteInput = {
  /** Only show what would change. */
  dry_run?: boolean;
  /** Local message IDs. */
  messages: Array<number>;
  /** Delete for good. Only for messages already in Deleted Items; needs --yes. */
  permanent?: boolean;
  /** Wait until the server has the change (needs famd). */
  wait?: boolean;
  /** Seconds --wait waits at most (default 60). */
  wait_secs?: number | null;
  /** Confirm a permanent delete when not interactive. */
  yes?: boolean;
};

export type MailFetchInput = {
  /** text (default) converts HTML to plain text; html returns the body as stored. */
  format?: BodyFormat;
  /** Don't download: show the body only if it is already on this device. */
  local?: boolean;
  /** The message's local ID. */
  message: number;
};

export type MailFetchOutput = {
  /** Account name. */
  account: string;
  /** The body; absent with `--local` when it isn't on this device. */
  body?: string | null;
  /** Whether the body is on this device. */
  body_local: boolean;
  /** Whether the body is pinned to this device. */
  body_pinned: boolean;
  /** The type of `body` as returned. */
  body_type?: BodyType | null;
  /** Whether the body is kept in the on-demand cache (it is outside the sync window and not pinned), so it may be evicted later. */
  cached: boolean;
  /** Cc recipients. */
  cc: Array<MailAddress>;
  /** Whether the body was downloaded for this request. */
  downloaded: boolean;
  /** Folder path, such as `Inbox` or `Inbox/Club`. */
  folder: string;
  /** Local folder ID. */
  folder_id: number;
  /** Sender. */
  from: MailAddress;
  /** Has file attachments. */
  has_attachments: boolean;
  /** Local message ID, for `fam mail thread`, `fetch` and `pin`. */
  id: number;
  /** Importance. */
  importance: Importance;
  /** RFC 5322 Message-ID, when known. */
  internet_message_id?: string | null;
  /** Follow-up flag. */
  is_flagged: boolean;
  /** Read state. */
  is_read: boolean;
  /** Short text preview. */
  preview: string;
  /** Received time (RFC 3339, UTC). */
  received_at: string;
  /** Graph immutable ID, for `fam api`. */
  remote_id: string;
  /** Sent time (RFC 3339, UTC), when known. */
  sent_at?: string | null;
  /** Subject. */
  subject: string;
  /** Local conversation ID. */
  thread: number;
  /** To recipients. */
  to: Array<MailAddress>;
};

export type MailFlagInput = {
  /** Clear the flag instead. */
  clear?: boolean;
  /** Only show what would change. */
  dry_run?: boolean;
  /** Local message IDs. */
  messages: Array<number>;
  /** Wait until the server has the change (needs famd). */
  wait?: boolean;
  /** Seconds --wait waits at most (default 60). */
  wait_secs?: number | null;
};

export type MailListInput = {
  /** Only this account (its name in config.toml, or its address). Default: every account. */
  account?: string | null;
  /** The next-page token from a previous list. */
  after?: string | null;
  /** Only flagged messages. */
  flagged?: boolean;
  /** Folder name, path (Inbox/Club) or well-known name (inbox, sentitems, drafts, deleteditems, archive, junkemail). Default: inbox. */
  folder?: string | null;
  /** Most messages per page (default 50, at most 500). */
  limit?: number | null;
  /** Only unread messages. */
  unread?: boolean;
};

export type MailListOutput = {
  /** The folders listed (one per account when no account is named). */
  folders: Array<ListedFolder>;
  /** Messages, newest first, across those folders. */
  messages: Array<MessageHeader>;
  /** Pass as `--after` for the next page. */
  next?: string | null;
};

export type MailMoveInput = {
  /** Only show what would change. */
  dry_run?: boolean;
  /** Local message IDs. */
  messages: Array<number>;
  /** Destination folder: path (Inbox/Club), well-known name or name, in each message's own account. */
  to: string;
  /** Wait until the server has the change (needs famd). */
  wait?: boolean;
  /** Seconds --wait waits at most (default 60). */
  wait_secs?: number | null;
};

export type MailPinInput = {
  /** The message's local ID. */
  message: number;
  /** Unpin instead: the body becomes an ordinary cached body again. */
  unpin?: boolean;
};

export type MailProcessInput = {
  /** Only show what would change. */
  dry_run?: boolean;
  /** Local message IDs. */
  messages: Array<number>;
  /** Wait until the server has the change (needs famd). */
  wait?: boolean;
  /** Seconds --wait waits at most (default 60). */
  wait_secs?: number | null;
};

export type MailReadInput = {
  /** Only show what would change. */
  dry_run?: boolean;
  /** Local message IDs (from fam mail list). */
  messages: Array<number>;
  /** Mark unread instead. */
  unread?: boolean;
  /** Wait until the server has the change (needs famd). */
  wait?: boolean;
  /** Seconds --wait waits at most (default 60). */
  wait_secs?: number | null;
};

export type MailReportInput = {
  /** Only show what would change. */
  dry_run?: boolean;
  /** Local message IDs. */
  messages: Array<number>;
  /** Report them as not junk, moving them from Junk Email to the Inbox. */
  not_junk?: boolean;
  /** Report them as phishing (a scam) instead of junk. */
  phishing?: boolean;
  /** Wait until the server has the report (needs famd). */
  wait?: boolean;
  /** Seconds --wait waits at most (default 60). */
  wait_secs?: number | null;
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

export type MailSendInput = {
  /** The account to send from (config name or address); needed with more than one. */
  account?: string | null;
  /** Attach a file (up to 3 MB); repeat for more. */
  attach?: Array<string>;
  /** Bcc. */
  bcc?: Array<string>;
  /** Body text (plain text unless --html). */
  body?: string | null;
  /** Read the body from a file. */
  body_file?: string | null;
  /** Cc. */
  cc?: Array<string>;
  /** Only show what would happen. */
  dry_run?: boolean;
  /** Send from this alias of the account (listed in `aliases` in config.toml). */
  from?: string | null;
  /** The body is HTML. */
  html?: boolean;
  /** low, normal or high. */
  importance?: string | null;
  /** Add an inline image. */
  inline?: Array<string>;
  /** Leave the account's signature off. */
  no_signature?: boolean;
  /** Skip the undo-send delay. */
  now?: boolean;
  /** Subject. */
  subject?: string | null;
  /** To: an address or `Name <address>`; repeat, or separate with commas. */
  to?: Array<string>;
  /** Wait until it has gone (needs famd; includes the undo-send delay). */
  wait?: boolean;
  /** Seconds --wait waits at most (default 60). */
  wait_secs?: number | null;
  /** Send even to many outside recipients who would see each other. */
  yes?: boolean;
};

export type MailThreadInput = {
  /** The local ID of any message in the conversation. */
  message: number;
  /** Leave out messages in Deleted Items and Junk Email. */
  no_trash?: boolean;
};

export type MailThreadOutput = {
  /** Account name. */
  account: string;
  /** Graph conversation ID. */
  conversation_id: string;
  /** Where the conversation's messages are. */
  folders: Array<ThreadFolder>;
  /** Newest message time (RFC 3339, UTC). */
  last_received_at: string;
  /** Messages after copies are merged (trash included). */
  message_count: number;
  /** The messages, oldest first, across every folder. */
  messages: Array<ThreadMessage>;
  /** Everyone on the conversation, in order of first appearance. */
  participants: Array<MailAddress>;
  /** Subject without RE:/FW: prefixes. */
  subject: string;
  /** Local conversation ID. */
  thread: number;
  /** Messages with an unread copy. */
  unread_count: number;
};

export type McpAnnotations = {
  /** The tool may delete or send. */
  destructiveHint: boolean;
  /** The tool changes nothing. */
  readOnlyHint: boolean;
};

export type McpServeInput = {
  /** Access levels whose commands are offered, comma-separated or repeated: read, draft, write, send, delete, rules. Default read,draft. famd still checks each write against [agent] in config.toml. */
  allow?: Array<string>;
};

export type McpServeOutput = {
  /** Requests answered. */
  requests: number;
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

export type MessageHeader = {
  /** Account name. */
  account: string;
  /** Whether the body is on this device. */
  body_local: boolean;
  /** Whether the body is pinned to this device. */
  body_pinned: boolean;
  /** Cc recipients. */
  cc: Array<MailAddress>;
  /** Folder path, such as `Inbox` or `Inbox/Club`. */
  folder: string;
  /** Local folder ID. */
  folder_id: number;
  /** Sender. */
  from: MailAddress;
  /** Has file attachments. */
  has_attachments: boolean;
  /** Local message ID, for `fam mail thread`, `fetch` and `pin`. */
  id: number;
  /** Importance. */
  importance: Importance;
  /** RFC 5322 Message-ID, when known. */
  internet_message_id?: string | null;
  /** Follow-up flag. */
  is_flagged: boolean;
  /** Read state. */
  is_read: boolean;
  /** Short text preview. */
  preview: string;
  /** Received time (RFC 3339, UTC). */
  received_at: string;
  /** Graph immutable ID, for `fam api`. */
  remote_id: string;
  /** Sent time (RFC 3339, UTC), when known. */
  sent_at?: string | null;
  /** Subject. */
  subject: string;
  /** Local conversation ID. */
  thread: number;
  /** To recipients. */
  to: Array<MailAddress>;
};

export type OnlineStatus = "not_requested" | "not_needed" | "searched" | "offline" | "daemon_unavailable" | "failed";

export type OpIdInput = {
  /** The op's ID (from fam ops list). */
  id: number;
};

export type OpIdOutput = {
  /** Whether anything changed (false: it was not in a state that allows it). */
  changed: boolean;
  /** The op. */
  id: number;
};

export type OpView = {
  /** Account key. */
  account: string;
  /** Times tried. */
  attempts: number;
  /** Last error. */
  error?: string | null;
  /** Op ID. */
  id: number;
  /** Kind. */
  kind: string;
  /** When it was queued (RFC 3339). */
  queued_at: string;
  /** Earliest next try (RFC 3339), for a pending op waiting out a retry. */
  retry_at?: string | null;
  /** Who asked. */
  source: string;
  /** `pending`, `in_flight`, `done` or `failed`. */
  state: string;
  /** What it does. */
  summary: string;
};

export type OpsListInput = {
  /** Also show changes the server got in the last hour. */
  all?: boolean;
  /** Most rows (default 100). */
  limit?: number | null;
};

export type OpsListOutput = {
  /** Ops, newest first. */
  ops: Array<OpView>;
};

export type PersonView = {
  /** Accounts that know them. */
  accounts: Array<string>;
  /** Address. */
  address: string;
  /** Company, from the contact. */
  company?: string;
  /** In Outlook contacts. */
  contact: boolean;
  /** Last message either way (RFC 3339), if any. */
  last?: string | null;
  /** `Name <address>`, ready for --to. */
  mailbox: string;
  /** Name, possibly empty. */
  name: string;
  /** Messages received from them. */
  received: number;
  /** Messages sent to them. */
  sent: number;
};

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

export type ProposedEvent = {
  /** All day. */
  all_day: boolean;
  /** The local ID of an event already in the calendar that matches it. */
  already?: number | null;
  /** Same form. All day: the last day. None: `fam cal add`'s default length. */
  end?: string | null;
  /** The words in the email it came from, when the AI found it. */
  evidence?: string | null;
  /** Location. */
  location?: string | null;
  /** Notes. */
  notes?: string | null;
  /** How it repeats. */
  repeat?: ProposedRepeat | null;
  /** `YYYY-MM-DD HH:MM`, or `YYYY-MM-DD` for all day, in `time_zone`. */
  start: string;
  /** Subject. */
  subject: string;
  /** IANA zone the times are in. */
  time_zone: string;
  /** iCalendar UID, from an `.ics`. */
  uid?: string | null;
};

export type ProposedRepeat = {
  /** Or this many times (`--count`). */
  count?: number | null;
  /** `daily`, `weekly`, `monthly` or `yearly` (`--repeat`). */
  every: string;
  /** Every this many (`--every`). */
  interval: number;
  /** Weekly: the days, such as `tue` (`--on`). */
  on: Array<string>;
  /** The last date, `YYYY-MM-DD` (`--until`). */
  until?: string | null;
};

export type QueuedOp = {
  /** Account key. */
  account: string;
  /** Why it failed, or the last retry's error. */
  error?: string | null;
  /** Op ID, for `fam ops wait` and `fam ops list`; `None` on a dry run. */
  id?: number | null;
  /** Local ID of the message, folder or event the op created, if any. */
  item?: number | null;
  /** Op kind, e.g. `move_message`. */
  kind: string;
  /** When it is due to go (RFC 3339), if held back by the undo-send delay. */
  not_before?: string | null;
  /** `planned` (dry run), `pending`, `in_flight`, `done` or `failed`. */
  state: string;
  /** What it does, in words. */
  summary: string;
};

export type Reason = {
  /** The words in the email it is based on. */
  evidence: string;
  /** What kind of sign: `impersonation`, `credential request`, ... */
  signal: string;
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

export type SyncCheckInput = {
  /** Only this account (its name in config.toml). */
  account?: string | null;
  /** Most differences listed per account (default 50); counts are always complete. */
  limit?: number | null;
  /** Compare at once, without first waiting for a sync round. Changes still syncing then show as drift. */
  no_settle?: boolean;
};

export type SyncError = {
  /** The error. */
  message: string;
  /** The resource: `folders`, or `messages:<folder>`. */
  resource: string;
};

export type SyncPhase = "starting" | "folder_tree" | "headers" | "bodies" | "calendar" | "idle" | "backoff" | "auth_required" | "stopped";

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

export type ThreadFolder = {
  /** Messages (copies included) in the folder. */
  count: number;
  /** Folder path. */
  folder: string;
  /** Local folder ID. */
  folder_id: number;
};

export type ThreadMessage = {
  /** Account name. */
  account: string;
  /** Whether the body is on this device. */
  body_local: boolean;
  /** Whether the body is pinned to this device. */
  body_pinned: boolean;
  /** Cc recipients. */
  cc: Array<MailAddress>;
  /** Local IDs of other copies of the same message (same Message-ID), such as mail sent to yourself, which is in both Inbox and Sent Items. */
  copies: Array<number>;
  /** Folder path, such as `Inbox` or `Inbox/Club`. */
  folder: string;
  /** Local folder ID. */
  folder_id: number;
  /** Sender. */
  from: MailAddress;
  /** Sent from the account's own address. */
  from_me: boolean;
  /** Has file attachments. */
  has_attachments: boolean;
  /** Local message ID, for `fam mail thread`, `fetch` and `pin`. */
  id: number;
  /** Importance. */
  importance: Importance;
  /** In Deleted Items or Junk Email. */
  in_trash: boolean;
  /** RFC 5322 Message-ID, when known. */
  internet_message_id?: string | null;
  /** Follow-up flag. */
  is_flagged: boolean;
  /** Read state. */
  is_read: boolean;
  /** Short text preview. */
  preview: string;
  /** Received time (RFC 3339, UTC). */
  received_at: string;
  /** Graph immutable ID, for `fam api`. */
  remote_id: string;
  /** Sent time (RFC 3339, UTC), when known. */
  sent_at?: string | null;
  /** Subject. */
  subject: string;
  /** Local conversation ID. */
  thread: number;
  /** To recipients. */
  to: Array<MailAddress>;
};

export type UiDone = {
  /** What the app now shows, in words. */
  shown: string;
};

export type UiFolderInput = {
  /** The account the folder is in (its name in config.toml). Needed when a folder is given and there is more than one account. */
  account?: string | null;
  /** Folder path (Inbox/Club) or well-known name (inbox, sentitems, drafts, archive). Default: every account's Inbox together. */
  folder?: string | null;
};

export type UiOpenInput = {
  /** Local ID of the message to open. */
  message: number;
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

export type WatchInput = {
  /** Only messages this search matches (`fam mail search` syntax), for example `from:club`. */
  match?: string | null;
};

export type WatchOutput = {
  /** Events printed. */
  events: number;
};

export type WriteOutput = {
  /** True when nothing was changed (`--dry-run`). */
  dry_run: boolean;
  /** The ops, in queue order. */
  ops: Array<QueuedOp>;
  /** True when the command waited for the server (`--wait`). */
  waited: boolean;
};

/** Every command, by dotted name, with its input and output types. */
export interface Commands {
  /** Show what agents may do */
  "agent.status": { input: AgentStatusInput; output: AgentStatusOutput };
  /** Call Microsoft Graph directly (GET only) */
  "api": { input: ApiInput; output: AnyValue };
  /** List the audit log of writes */
  "audit.list": { input: AuditListInput; output: AuditListOutput };
  /** Sign in to a Microsoft account */
  "auth.login": { input: AuthLoginInput; output: AccountSignIn };
  /** Sign out: forget an account's tokens */
  "auth.logout": { input: AuthLogoutInput; output: AuthLogoutOutput };
  /** Show which accounts are signed in */
  "auth.status": { input: AuthStatusInput; output: AuthStatusOutput };
  /** Add an event */
  "cal.add": { input: CalAddInput; output: WriteOutput };
  /** List the calendars on this device */
  "cal.calendars": { input: CalCalendarsInput; output: CalCalendarsOutput };
  /** Delete an event, or cancel a meeting you organised */
  "cal.delete": { input: CalDeleteInput; output: WriteOutput };
  /** Change an event */
  "cal.edit": { input: CalEditInput; output: WriteOutput };
  /** List events in a date range */
  "cal.list": { input: CalListInput; output: CalListOutput };
  /** Suggest calendar events from an email, without adding them */
  "cal.propose": { input: CalProposeInput; output: CalProposeOutput };
  /** Answer a meeting invitation */
  "cal.rsvp": { input: CalRsvpInput; output: WriteOutput };
  /** Show which config file is used */
  "config.path": { input: ConfigPathInput; output: ConfigPathOutput };
  /** Check config.toml for errors */
  "config.validate": { input: ConfigPathInput; output: ConfigValidateOutput };
  /** Find people to write to */
  "contacts.search": { input: ContactsSearchInput; output: ContactsSearchOutput };
  /** Save a new draft */
  "draft.create": { input: DraftCreateInput; output: WriteOutput };
  /** Save a forward as a draft */
  "draft.forward": { input: DraftForwardInput; output: WriteOutput };
  /** Save a reply as a draft */
  "draft.reply": { input: DraftReplyInput; output: WriteOutput };
  /** Send a draft */
  "draft.send": { input: DraftSendInput; output: WriteOutput };
  /** Change a draft */
  "draft.update": { input: DraftUpdateInput; output: WriteOutput };
  /** Colour a mail folder in the folder pane */
  "folder.color": { input: FolderColorInput; output: FolderRow };
  /** Create a mail folder */
  "folder.create": { input: FolderCreateInput; output: WriteOutput };
  /** Delete a mail folder (to Deleted Items) */
  "folder.delete": { input: FolderDeleteInput; output: WriteOutput };
  /** List mail folders */
  "folder.list": { input: FolderListInput; output: FolderListOutput };
  /** Move a mail folder under another, or to the top level */
  "folder.move": { input: FolderMoveInput; output: WriteOutput };
  /** Pin a mail folder to the top of the folder pane */
  "folder.pin": { input: FolderPinInput; output: FolderRow };
  /** Rename a mail folder */
  "folder.rename": { input: FolderRenameInput; output: WriteOutput };
  /** Unpin a mail folder */
  "folder.unpin": { input: FolderPinInput; output: FolderRow };
  /** Move messages to Archive */
  "mail.archive": { input: MailArchiveInput; output: WriteOutput };
  /** List or save a message's attachments */
  "mail.attachments": { input: MailAttachmentsInput; output: MailAttachmentsOutput };
  /** Check how likely an email is to be spam or a scam */
  "mail.check-spam": { input: CheckSpamInput; output: CheckSpamOutput };
  /** Move messages to Deleted Items */
  "mail.delete": { input: MailDeleteInput; output: WriteOutput };
  /** Show a message with its text */
  "mail.fetch": { input: MailFetchInput; output: MailFetchOutput };
  /** Flag messages for follow-up, or clear the flag */
  "mail.flag": { input: MailFlagInput; output: WriteOutput };
  /** List messages in a folder */
  "mail.list": { input: MailListInput; output: MailListOutput };
  /** Move messages to another folder */
  "mail.move": { input: MailMoveInput; output: WriteOutput };
  /** Keep a message's body on this device */
  "mail.pin": { input: MailPinInput; output: PinResult };
  /** Move messages to their account's processed folder */
  "mail.process": { input: MailProcessInput; output: WriteOutput };
  /** Mark messages read or unread */
  "mail.read": { input: MailReadInput; output: WriteOutput };
  /** Report messages as junk, phishing or not junk */
  "mail.report": { input: MailReportInput; output: WriteOutput };
  /** Search mail across all accounts */
  "mail.search": { input: MailSearchInput; output: MailSearchOutput };
  /** Write and send a message */
  "mail.send": { input: MailSendInput; output: WriteOutput };
  /** Show a whole conversation */
  "mail.thread": { input: MailThreadInput; output: MailThreadOutput };
  /** Serve the commands to an MCP client over stdio */
  "mcp.serve": { input: McpServeInput; output: McpServeOutput };
  /** Cancel a change that hasn't reached the server */
  "ops.cancel": { input: OpIdInput; output: OpIdOutput };
  /** List changes waiting for the server */
  "ops.list": { input: OpsListInput; output: OpsListOutput };
  /** Try a failed change again */
  "ops.retry": { input: OpIdInput; output: OpIdOutput };
  /** Describe every command, its input, output and errors */
  "schema": { input: SchemaInput; output: SchemaOutput };
  /** Compare the store with the server and report drift */
  "sync.check": { input: SyncCheckInput; output: CheckReport };
  /** Show how far each account's sync has got */
  "sync.status": { input: SyncStatusInput; output: SyncStatusOutput };
  /** Show a folder in the app */
  "ui.folder": { input: UiFolderInput; output: UiDone };
  /** Open a message in the app */
  "ui.open": { input: UiOpenInput; output: UiDone };
  /** Show the Familiar version */
  "version": { input: VersionInput; output: VersionOutput };
  /** Print new mail as it arrives, as JSON lines */
  "watch": { input: WatchInput; output: WatchOutput };
}

/** A command name. */
export type CommandName = keyof Commands;

/** Every command name, in order. */
export const COMMAND_NAMES = [
  "agent.status",
  "api",
  "audit.list",
  "auth.login",
  "auth.logout",
  "auth.status",
  "cal.add",
  "cal.calendars",
  "cal.delete",
  "cal.edit",
  "cal.list",
  "cal.propose",
  "cal.rsvp",
  "config.path",
  "config.validate",
  "contacts.search",
  "draft.create",
  "draft.forward",
  "draft.reply",
  "draft.send",
  "draft.update",
  "folder.color",
  "folder.create",
  "folder.delete",
  "folder.list",
  "folder.move",
  "folder.pin",
  "folder.rename",
  "folder.unpin",
  "mail.archive",
  "mail.attachments",
  "mail.check-spam",
  "mail.delete",
  "mail.fetch",
  "mail.flag",
  "mail.list",
  "mail.move",
  "mail.pin",
  "mail.process",
  "mail.read",
  "mail.report",
  "mail.search",
  "mail.send",
  "mail.thread",
  "mcp.serve",
  "ops.cancel",
  "ops.list",
  "ops.retry",
  "schema",
  "sync.check",
  "sync.status",
  "ui.folder",
  "ui.open",
  "version",
  "watch",
] as const satisfies readonly CommandName[];
