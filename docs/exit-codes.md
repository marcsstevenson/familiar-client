# fam exit codes

Generated from `familiar_core::ErrorKind`; do not edit by hand. Regenerate with
`FAMILIAR_BLESS=1 cargo test -p familiar-cli --test parity`.

These codes are stable. A code never changes meaning once published.

| Code | Kind | Meaning |
| --- | --- | --- |
| 0 | | Success. |
| 1 | `internal` | Unexpected failure inside Familiar (a bug). |
| 2 | `usage` | Invalid command line or input. |
| 3 | `config` | config.toml is missing or invalid. |
| 4 | `auth_required` | The account needs to sign in: fam auth login. |
| 5 | `permission_denied` | Not granted to this caller (agent permissions). |
| 6 | `not_found` | The item or account does not exist. |
| 7 | `offline` | The server is unreachable and the command needs it. |
| 8 | `throttled` | The server asked to slow down; retry later. |
| 9 | `conflict` | The item changed since it was read. |
| 10 | `confirmation_required` | Needs --yes when not run interactively. |
| 11 | `daemon_unavailable` | famd is not running or not reachable. |
| 12 | `server` | The server returned an unexplained error. |
