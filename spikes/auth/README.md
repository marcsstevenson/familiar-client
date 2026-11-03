# auth spike (WP 0.2 step B)

Throwaway. Signs in to one account, calls `/me`, `/me/mailFolders`, `/me/messages`, `/me/calendars` and `/me/mailboxSettings`, then refreshes the token once and calls `/me` again. Each line prints `ok` or `failed` with the HTTP status and the Graph or AADSTS error code. Tokens stay in memory: they are never printed or written to disk. For `mailboxSettings` only the field names are printed, not their values.

Run from this folder in PowerShell, with the Familiar app's Application (client) ID:

```powershell
$env:FAMILIAR_CLIENT_ID = "<client id>"
cargo run -- --account hotmail --flow pkce
cargo run -- --account hotmail --flow device
cargo run -- --account parklands --flow pkce
cargo run -- --account parklands --flow device
```

- `--account hotmail` uses the `consumers` authority; `--account parklands` uses `organizations`.
- `--flow pkce` opens the system browser and listens on a loopback port; `--flow device` prints a code to enter at <https://microsoft.com/devicelogin>.
- `--read-only` asks for `User.Read offline_access Mail.Read Calendars.Read` instead of the full set.
- `--client-id <id>` can be used instead of the environment variable.

Sign in with the account the run is for; use a private window if the browser picks the wrong one. Paste the output lines into the WP 0.2 results table.
