# Adding a Gmail account

Gmail accounts sign in with Familiar's own Google client, so there is nothing to set up: `fam account add --provider gmail` (or **Gmail** on the desktop app's Welcome screen) signs in and writes the account to config.toml. The client is an unverified Google project capped at 100 users over its lifetime, so Google shows a "hasn't verified this app" page at sign-in. Its ID and secret aren't in the repo: a build includes them only when `FAMILIAR_GOOGLE_CLIENT_ID` and `FAMILIAR_GOOGLE_CLIENT_SECRET` are set while it compiles (Familiar's release builds set them; see [releasing.md](releasing.md)). A build without them, or anyone who prefers their own Google project, uses a `google-client.json` next to `config.toml` or `[app.google] client_file` (steps 1 and 2 below), which also takes the place of the built-in client. famd syncs Gmail mail with labels as folders, bodies for the sync window, changes through Gmail's history, and server search. Read, flag, archive, move, delete (to Trash), report, folder changes, drafts and sending go back to Gmail; see [writes.md](writes.md#gmail-accounts). Send from an alias only once Gmail has verified it (Settings, Accounts, Send mail as), and list it in `aliases = [...]` so the compose window offers it.

## 1. Make your own Google client (optional)

Follow Part 1 of [google-sign-in-test-guide.md](google-sign-in-test-guide.md): create a Google Cloud project, turn on the Gmail, Calendar and People APIs, publish the consent screen to production (don't submit it for verification), and create a **Desktop app** client. Save its JSON as `google-client.json` next to your `config.toml`, or point `[app.google] client_file` at it.

## 2. Add the account to config.toml

```toml
[accounts.gmail]
provider = "gmail"
email = "you@gmail.com"
# No tenant. Optional, if the client file isn't next to config.toml:
# [app.google]
# client_file = "C:\\Users\\you\\.familiar\\google-client.json"
```

`fam config validate` refuses a `tenant` on a Gmail account and requires one on a Microsoft account.

## 3. Sign in

```sh
fam auth login gmail
```

The browser opens at Google. Google says the app isn't verified: choose **Advanced**, then **Go to Familiar (unsafe)**, and allow everything. `fam auth status --check gmail` proves the sign-in by reading your Gmail profile.

There is no `--device-code` for Gmail: Google doesn't offer device code sign-in for Gmail. On a machine without a browser, sign in on one with a browser and copy the encrypted token file across (`FAMILIAR_TOKEN_STORE=file`, see [sign-in.md](sign-in.md)).

## 4. View Google calendars

Open Calendar in Familiar. The calendar picker groups your personal, shared, and subscribed calendars under the Gmail account. Use its checkboxes to choose which calendars appear together. Shared calendars retain their Google access permissions.

To check sync from the CLI, run `fam cal calendars --account gmail` and `fam cal list --account gmail --days 30`. The daemon refreshes the calendar list and the events in the configured calendar window every five minutes.
