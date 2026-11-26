# config.toml

Everything a person sets in Familiar lives in one TOML file: accounts, signatures, calendar purposes, sync settings, views, rules, keys and agent permissions. It never holds passwords, tokens or mail; tokens are in the OS keychain. A new install doesn't need to write it by hand: `fam account add` (or the desktop app's Welcome screen) creates it and adds each account from its sign-in, and `fam init` creates just the commented starting file. For a full example, see [`config.example.toml`](../config.example.toml).

Your own config has your signatures in it, so keep it in a private repo, not in the public Familiar repo.

## Where it lives

`fam config path` shows the file in use. In order: a path given to the command, then `FAMILIAR_CONFIG`, then the platform default:

| OS | Default |
| --- | --- |
| Windows | `%USERPROFILE%\.familiar\config.toml` (not AppData: Windows redirects AppData for packaged apps such as the Claude desktop app, and for everything they start) |
| macOS | `~/Library/Application Support/Familiar/config.toml` |
| Linux | `$XDG_CONFIG_HOME/familiar/config.toml`, else `~/.config/familiar/config.toml` |

## Checking it

`fam config validate [path]` reports every problem with its line and column, then exits 3 (`config`) if there are errors. Warnings, such as `tenant = "organizations"`, don't fail. It checks:

- TOML syntax, types, required keys, and unknown or misspelt keys.
- `version = 1`, the client ID is a GUID, `send_delay_seconds` is 0 to 120.
- Account names are lowercase letters, digits, `-` and `_`; emails look like addresses and aren't repeated; `tenant` is `consumers`, a tenant ID or a domain. `organizations` and `common` give a warning, because token refresh must go to the account's own tenant.
- Calendar purposes and rules name accounts that exist; rule names are unique per account; rules have at least one condition and one action.
- `sync.window` and `sync.headers` are periods (`90d`, `4w`, `6m`, `1y`), headers cover at least the window, `cache_limit` is a size (`500MB`, `2GB`).
- View names are unique, queries aren't empty, and no key is bound twice.
- `agent.allow` uses the access levels `read`, `draft`, `write`, `send`, `delete`, `rules`. `send`, `delete` and `rules` can't be in `agent.allow`; they go under `[agent.accounts.<name>]`, which must name an account. See [agent.md](agent.md).

## Sections

| Section | Keys |
| --- | --- |
| top level | `version` (required, 1) |
| `[app]` | `client_id` (defaults to Familiar's own, `c6057b6a-a692-47dd-b34e-12629372e32a`), `theme` (`system`, `light`, `dark`), `send_delay_seconds` (default 10) |
| `[app.google]` | `client_file`, the path to Google Desktop app credentials for Gmail |
| `[desktop]` | `accent` (e.g. `"#99D9EA"`, omitted for Familiar blue), `neon` (false), `panes` (folder and list widths in pixels, 240 and 400), `sort` (newest, oldest, from, subject), `unread_only` (false), `day_panel_open` (false), `calendar_view` (Day, Week, Month, Agenda), `collapsed` and `hidden_calendars` (see the example for key formats) |
| `[desktop.sender_calendars]` | Sender address mapped to an account, calendar name and optional provider ID |
| `[folders.<account>]` | Each provider folder ID maps to its `color`, `pinned`, `order` and `pin_order` |
| `[reading]` | `image_senders`, the addresses whose remote images you always allow |
| `[calendar]` | `past` (3m) and `future` (1y), the event sync window |
| `[accounts.<name>]` | `email`, `provider` (graph or gmail), `tenant` (required for Microsoft), `display_name`, `aliases` (other addresses to send from), `processed_folder` (where the Processed tick and `fam mail process` move messages; created at the top level when missing; off when unset), `spam_check` (offer the spam and scam check on this account's mail, `false` by default; see [spam.md](spam.md)), `spam_auto` (also check new Inbox and Junk mail in the background; implies `spam_check`; `false` by default) |
| `[accounts.<name>.signature]` | `new`, `reply`, as HTML |
| `[calendar.purposes]` | `<purpose> = { account, calendar }` |
| `[sync]` | `window` (`1y`), `headers` (`all`), `older_bodies` (`on-demand`, `none`), `cache_limit` (`2GB`), `attachments` (`on-demand`, `window`) |
| `[[views]]` | `name`, `query` |
| `[[rules]]` | `name`, `account`, `run` (`server`, `local`), `enabled` (default true), `when`, `then`. Conditions and actions are checked by the rules engine later. |
| `[[hooks]]` | `name`, `on` (`new_mail`), `match` (a search; default every new message), `run` (a command line), `enabled` (default true), `timeout_seconds` (`60`). See [hooks.md](hooks.md). |
| `[keys]` | `<action> = "<key>"` |
| `[agent]` | `allow` (default `["read", "draft"]`; may add `write`) |
| `[ai]` | `enabled` (default `false`), `tool` (`auto`, `claude`, `codex`, `none` or a path), `model`, `timeout_seconds` (`90`), `accounts`. See [ai.md](ai.md). |
| `[agent.accounts.<name>]` | `allow`: more access on that account, e.g. `["send", "delete"]` |

## Upgrading existing preferences

Familiar imports preferences previously saved by the webview and the old folder and reader JSON files. Explicit TOML values take precedence. The JSON files get a `.migrated` suffix after a successful import. Window positions, open messages, scroll positions and draft recovery use the local session.

## Writing back

The app edits the file through `familiar_config::ConfigDocument`, which changes only the values it sets. Comments, blank lines, key order, string styles and Windows line endings are kept, new keys go at the end of their table, and signatures are written as `'''` multi-line strings so the HTML stays readable. Saves are atomic (temporary file, then rename) and refused if the result wouldn't validate. `crates/config/tests/roundtrip.rs` checks all of this against the example config.
