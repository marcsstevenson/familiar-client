# Signing in

`fam auth` signs Familiar in to each Microsoft account in `config.toml` and keeps the tokens on this machine.

On a new install, `fam account add` does it all in one step: it signs in at `common` (any personal or work Microsoft account), then writes the account to `config.toml` (creating the file if needed) with the email, display name and tenant from the sign-in: `consumers` for personal accounts, the tenant ID for work ones. `--provider gmail` adds a Gmail account the same way ([gmail-setup.md](gmail-setup.md)). The name defaults to one made from the address (`hotmail`, `gmail`, or the first part of a work domain; `--name` sets it), a running famd is restarted so it syncs the account, and agents can't run it. `fam init` only creates the commented starting `config.toml`.

```sh
fam auth login hotmail                  # opens the browser
fam auth login parklands --device-code  # or: enter a code at microsoft.com/devicelogin
fam auth status                         # what is signed in (no network)
fam auth status --check                 # prove each sign-in works: refresh if needed, call Graph /me
fam auth logout parklands               # forget an account's tokens (or --all)
```

- **Which account and tenant.** `fam auth login <name>` reads `email` and `tenant` from `[accounts.<name>]`. The email is sent as a hint to the sign-in page, and a sign-in as anyone else is refused without saving anything. `--authority`, `--email` and `--client-id` override the config, and let an account sign in before it is in the config.
- **Refresh.** Access tokens are refreshed five minutes before they expire. Personal accounts (Hotmail, Outlook.com) refresh against `consumers`. Work accounts refresh against their own tenant ID from the ID token, never `organizations` or `common`: Parklands' refresh failed there in WP 0.2 (AADSTS65001). Entra rotates refresh tokens and the new one is saved each time.
- **Exit codes.** A sign-in that is missing, revoked or expired exits 4 (`auth_required`); no network exits 7 (`offline`). See [exit-codes.md](exit-codes.md).
- **Sign-out** is local. Microsoft has no token revocation for apps like Familiar. To cut access everywhere, remove Familiar at myaccount.microsoft.com (personal) or myapps.microsoft.com (work).

## Where tokens are kept

| `FAMILIAR_TOKEN_STORE` | Store |
| --- | --- |
| `auto` (default) | The OS keychain (Windows Credential Manager, macOS Keychain, Secret Service on Linux) if one answers, otherwise the encrypted file if a passphrase is set |
| `keychain` | The OS keychain only |
| `file` | The encrypted file only |

Keychain entries are under the service `familiar`, one per account (`familiar` / `hotmail`), split into 2 KB chunks (`hotmail#1`, ...) because Windows Credential Manager holds at most 2,560 bytes per entry, plus `#index` listing the accounts.

The encrypted file is for containers and other machines without a keychain. It is `tokens.enc` in `FAMILIAR_DATA_DIR` (default: the platform's local data folder, then `Familiar`), sealed with XChaCha20-Poly1305 under a key derived with Argon2id from `FAMILIAR_TOKEN_PASSPHRASE`, or from the file named by `FAMILIAR_TOKEN_PASSPHRASE_FILE` (a container secret). On Unix it is created with mode 0600.

## Testing against the mock

`familiar-mock-graph` also mocks the identity platform on the same port, with Entra's tenant rules (including the AADSTS65001 refresh failure). Point Familiar at it with `FAMILIAR_LOGIN_URL=<mock.login_url()>` and `FAMILIAR_GRAPH_URL=<mock.base_url()>`. See `crates/auth/tests/mock_flows.rs` and `crates/cli/tests/auth.rs`.

The keychain itself is not touched by CI. On a desktop, `cargo test -p familiar-auth -- --ignored keychain` round-trips records through the real keychain under the service `familiar-test`.
