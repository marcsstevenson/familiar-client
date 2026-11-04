# config.toml

Everything a person sets in Familiar lives in one TOML file: accounts, signatures, calendar purposes, sync settings, views, rules, keys and agent permissions. It never holds passwords, tokens or mail; tokens are in the OS keychain. Start from [`config.example.toml`](../config.example.toml).

Your own config has your signatures in it, so keep it in a private repo, not in the public Familiar repo.

## Where it lives

`fam config path` shows the file in use. In order: a path given to the command, then `FAMILIAR_CONFIG`, then the platform default:

| OS | Default |
| --- | --- |
| Windows | `%APPDATA%\Familiar\config.toml` |
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
- `agent.allow` uses the access levels `read`, `draft`, `write`, `send`, `delete`, `rules`.

## Sections

| Section | Keys |
| --- | --- |
| top level | `version` (required, 1) |
| `[app]` | `client_id` (defaults to Familiar's own, `c6057b6a-a692-47dd-b34e-12629372e32a`), `theme` (`system`, `light`, `dark`), `send_delay_seconds` (default 10) |
| `[accounts.<name>]` | `email`, `tenant` (both required), `display_name` |
| `[accounts.<name>.signature]` | `new`, `reply`, as HTML |
| `[calendar.purposes]` | `<purpose> = { account, calendar }` |
| `[sync]` | `window` (`1y`), `headers` (`all`), `older_bodies` (`on-demand`, `none`), `cache_limit` (`2GB`), `attachments` (`on-demand`, `window`) |
| `[[views]]` | `name`, `query` |
| `[[rules]]` | `name`, `account`, `run` (`server`, `local`), `enabled` (default true), `when`, `then`. Conditions and actions are checked by the rules engine later. |
| `[keys]` | `<action> = "<key>"` |
| `[agent]` | `allow` (default `["read", "draft"]`) |

## Writing back

The app edits the file through `familiar_config::ConfigDocument`, which changes only the values it sets. Comments, blank lines, key order, string styles and Windows line endings are kept, new keys go at the end of their table, and signatures are written as `'''` multi-line strings so the HTML stays readable. Saves are atomic (temporary file, then rename) and refused if the result wouldn't validate. `crates/config/tests/roundtrip.rs` checks all of this against the example config.
