<!-- Generated from the command registry; regenerate with FAMILIAR_BLESS=1 cargo test -p familiar-cli --test parity -->
# fam command reference

Every command takes `--json`. Access is what `[agent]` must allow (see SKILL.md). `fam schema --json` has the full input and output schemas.

## agent.status

Show what agents may do. Access: `read`.

```
fam agent status
```

Says whether this session counts as an agent (FAMILIAR_AGENT is set, or it runs inside Claude Code) and lists, per account, the access agents have from [agent] in config.toml. A write that needs more fails with permission_denied (exit 5).

## api

Call Microsoft Graph directly (GET only). Access: `read`.

```
fam api <PATH>
```

The escape hatch for anything Familiar doesn't model yet: GETs a Graph path with an account's token and prints Graph's JSON response unchanged. Goes through famd, which holds the tokens and paces requests, so famd must be running. Only GET is allowed until writes arrive in Phase 2; any other --method is refused with exit code 5 (permission_denied). Example: fam api '/me/mailFolders/inbox/messages?$top=3&$select=subject' --account hotmail

- `--account <account>`: The account whose token to use (its name in config.toml). May be left out when famd syncs only one account.
- `--method <method>`: HTTP method. Only GET is allowed until writes arrive in Phase 2.
- `PATH` (required): A Graph path under https://graph.microsoft.com/v1.0, such as '/me/mailFolders?$top=5', or a full link Graph returned, such as an @odata.nextLink.

## audit.list

List the audit log of writes. Access: `read`.

```
fam audit list
```

Lists writes with their source (cli, gui, mcp or agent:<name>) and outcome: queued, done, failed, or denied when an agent lacked the grant. Newest first.

- `--limit <limit>`: Most rows (default 50).
- `--outcome <outcome>`: Only this outcome: queued, denied, done or failed.
- `--since <since>`: Only this far back: a period like 7d, 4w or 6m. Default all.
- `--source <source>`: Only this source, e.g. mcp or agent:claude-code; a trailing * matches the start (agent:*).

## auth.login

Sign in to a Microsoft account. Access: `write`.

```
fam auth login <ACCOUNT>
```

Signs in with the system browser (or a device code with --device-code) and keeps the tokens in the OS keychain, or in an encrypted file where there is none (set FAMILIAR_TOKEN_PASSPHRASE). Signing in again replaces the stored tokens.

- `ACCOUNT` (required): The account name, for example `hotmail` (lowercase letters, digits, - and _).
- `--authority <authority>`: Where to sign in: `consumers` for Hotmail and Outlook.com, `organizations` or the tenant ID or domain for a work account, or `common`. Defaults to the account's `tenant` in config.toml.
- `--client-id <client-id>`: The app (client) ID. Defaults to `[app] client_id` in config.toml, then Familiar's.
- `--device-code`: Use a device code (enter it at microsoft.com/devicelogin on any device) instead of opening a browser here.
- `--email <email>`: The address to sign in as. Defaults to the account's `email` in config.toml. Sent as a hint to the sign-in page; a sign-in as anyone else is refused.

## auth.logout

Sign out: forget an account's tokens. Access: `write`.

```
fam auth logout [ACCOUNT]
```

Removes the account's tokens from this machine. Microsoft offers no sign-out for apps like Familiar; to revoke access everywhere, remove Familiar at myaccount.microsoft.com (personal) or myapps.microsoft.com (work).

- `ACCOUNT`: The account to sign out.
- `--all`: Sign out every account.

## auth.status

Show which accounts are signed in. Access: `read`.

```
fam auth status [ACCOUNT]
```

Lists the accounts in config.toml and any others signed in on this machine. With --check, each sign-in is proved by refreshing if needed and calling Microsoft Graph.

- `ACCOUNT`: Only this account. With --check, a failed check is an error (exit code 4 or 7).
- `--check`: Prove each sign-in works: refresh the token if needed and call Graph /me.

## cal.add

Add an event. Access: `write`.

```
fam cal add [SUBJECT]
```

Adds an event to a calendar on this device at once and queues it for the server. --start takes "2026-11-21 18:00", "sat 6pm" or a date alone (all day), in --tz or this computer's zone; --end or --minutes (default 60) set the end. --calendar takes a calendar name or a purpose from [calendar.purposes]; --purpose takes a purpose (family). --repeat daily|weekly|monthly|yearly with --every, --on, --until or --count makes a series the server expands. --attendee sends invitations. --from-mail <id> suggests the subject, date, time and place from an email and links the email in the event.

- `--account <account>`: Account (its name in config.toml), when --calendar doesn't settle it.
- `--all-day`: All-day event (also when --start is a date alone).
- `--attendee <attendee>` (repeatable): Invite someone ("Name <addr>" or an address); repeat or separate with commas.
- `--body <body>`: Notes (plain text).
- `--calendar <calendar>`: Calendar name ("Family Room") or a purpose from [calendar.purposes] ("family"). Default: the account's main calendar.
- `--category <category>` (repeatable): Outlook category; repeat for more.
- `--count <count>`: Or: repeat this many times.
- `--days <days>`: All-day: how many days when there's no --end (default 1).
- `--dry-run`: Only show what would happen.
- `--end <end>`: End: a date and time, or a time on the start's day. All-day: the last day.
- `--every <every>`: Repeat every this many days, weeks, months or years (default 1).
- `--from-mail <from-mail>`: Make it from this email (local message ID): suggests the subject, date, time and place, and links the email in the event.
- `--location <location>`: Location.
- `--minutes <minutes>`: Length in minutes when there's no --end (default 60).
- `--no-reminder`: No reminder.
- `--on <on>` (repeatable): Weekly: the days, such as mon,wed (default the start's day).
- `--optional <optional>` (repeatable): Invite someone as optional.
- `--purpose <purpose>`: A purpose from [calendar.purposes], such as family.
- `--reminder <reminder>`: Reminder, minutes before the start (default 15).
- `--repeat <repeat>`: Repeat: daily, weekly, monthly or yearly.
- `--show-as <show-as>`: Show as free, tentative, busy (the default), oof or workingElsewhere.
- `--start <start>`: Start: "2026-11-21 18:00", "sat 6pm", "tomorrow 9:30am", or a date alone for an all-day event. With --from-mail, found in the email when left out.
- `SUBJECT`: Subject. With --from-mail, the email's subject when left out.
- `--tz <tz>`: Time zone for the times (IANA or Windows). Default: this computer's.
- `--until <until>`: Repeat until this date.
- `--wait`: Wait until the server has it (needs famd).
- `--wait-secs <wait-secs>`: Seconds --wait waits at most (default 60).
- `--yes`: Go ahead with more than 10 invitees.

## cal.calendars

List the calendars on this device. Access: `read`.

```
fam cal calendars
```

Lists each account's calendars with how many events are on this device, the synced window, the last sync and any error, and the purposes from [calendar.purposes] that point to each.

- `--account <account>`: Only this account (its name in config.toml).

## cal.delete

Delete an event, or cancel a meeting you organised. Access: `delete`.

```
fam cal delete <EVENT>
```

Takes an event off this device's calendar at once and queues the delete for the server. --series deletes the whole recurring series. A meeting you organised with attendees is cancelled, which tells them (needs --yes; --comment adds a note). To turn down someone else's meeting, use fam cal rsvp decline.

- `--comment <comment>`: For a meeting you organised: a note in the cancellation attendees get.
- `--dry-run`: Only show what would happen.
- `EVENT` (required): Local event ID (from fam cal list).
- `--series`: Delete the whole recurring series.
- `--wait`: Wait until the server has it (needs famd).
- `--wait-secs <wait-secs>`: Seconds --wait waits at most (default 60).
- `--yes`: Go ahead with cancelling a meeting you organised (attendees are told).

## cal.edit

Change an event. Access: `write`.

```
fam cal edit <EVENT>
```

Changes an event on this device at once and queues the change for the server. Times are read as fam cal add reads them; a new --start keeps the event's length unless --end or --minutes is given. --series changes every event of a recurring series (one occurrence otherwise). Only the organiser can change the subject, times, place or attendees; attendees with changes get an update.

- `--attendee <attendee>` (repeatable): New attendee list (replaces it); repeat for more.
- `--body <body>`: New notes (plain text).
- `--category <category>` (repeatable): New categories (replaces them); repeat for more.
- `--dry-run`: Only show what would change.
- `--end <end>`: New end.
- `EVENT` (required): Local event ID (from fam cal list).
- `--location <location>`: New location.
- `--minutes <minutes>`: New length in minutes.
- `--no-reminder`: Turn the reminder off.
- `--reminder <reminder>`: New reminder, minutes before the start.
- `--series`: Change every event in its series, not just this one.
- `--show-as <show-as>`: Show as free, tentative, busy, oof or workingElsewhere.
- `--start <start>`: New start (as fam cal add takes it). The length stays unless --end or --minutes.
- `--subject <subject>`: New subject.
- `--tz <tz>`: Time zone for the times (IANA or Windows). Default: this computer's.
- `--wait`: Wait until the server has the change (needs famd).
- `--wait-secs <wait-secs>`: Seconds --wait waits at most (default 60).
- `--yes`: Go ahead with more than 10 attendees.

## cal.list

List events in a date range. Access: `read`.

```
fam cal list
```

Lists events from the calendars on this device for --days days (default 7) from --from (default today), in --tz or this computer's time zone. --calendar takes a calendar name or a purpose from [calendar.purposes]. Reads the store, so it works without famd; famd keeps events for the window set by [calendar] past and future.

- `--account <account>`: Only this account (its name in config.toml).
- `--calendar <calendar>`: A calendar's name (such as "Family Room") or a purpose from `[calendar.purposes]` (such as "family"). All calendars if left out.
- `--days <days>`: How many days to list, from 1 to 366. Default 7.
- `--from <from>`: First day: `today` (the default), `tomorrow`, `yesterday` or a date like 2026-11-05.
- `--tz <tz>`: Time zone to show times in, IANA (Pacific/Auckland) or Windows (New Zealand Standard Time). Default: this computer's.

## cal.propose

Suggest calendar events from an email, without adding them. Access: `read`.

```
fam cal propose
```

Reads an email and says which calendar events it suggests, for you to check before adding them with fam cal add. A meeting request comes back as an invitation to answer with fam cal rsvp, not as a new event. Otherwise the events in its .ics attachments; else, with [ai] enabled in config.toml, the events the AI tool (Claude or Codex) finds in its text and PDFs; else the date, time and place found in its text. Each event says whether it is already in your calendar. Nothing is changed. Attachments and invitations are read through famd; without famd only the text is used.

- `--from-mail <from-mail>` (required): The email (local message ID).
- `--no-ai`: Don't ask the AI tool, even when [ai] is enabled.
- `--tz <tz>`: Time zone to give times in (IANA or Windows). Default: this computer's.

## cal.rsvp

Answer a meeting invitation. Access: `write`.

```
fam cal rsvp <EVENT> <RESPONSE>
```

Accepts, tentatively accepts or declines a meeting on this device at once and queues the answer for the server, which tells the organiser unless --no-reply. A declined meeting leaves the calendar. --series answers for the whole series.

- `--comment <comment>`: A note to the organiser.
- `--dry-run`: Only show what would happen.
- `EVENT` (required): Local event ID (from fam cal list).
- `--no-reply`: Don't tell the organiser.
- `RESPONSE` (required): accept, tentative or decline.
- `--series`: Answer for the whole recurring series.
- `--wait`: Wait until the server has it (needs famd).
- `--wait-secs <wait-secs>`: Seconds --wait waits at most (default 60).

## config.path

Show which config file is used. Access: `read`.

```
fam config path [PATH]
```

Shows the config file path: the argument if given, else FAMILIAR_CONFIG, else the platform default (Windows %USERPROFILE%\.familiar\config.toml, macOS ~/Library/Application Support/Familiar/config.toml, Linux $XDG_CONFIG_HOME/familiar/config.toml).

- `PATH`: The config file. Defaults to FAMILIAR_CONFIG, then the platform default (see `fam config path`).

## config.validate

Check config.toml for errors. Access: `read`.

```
fam config validate [PATH]
```

Parses and checks the config file: syntax, unknown or misspelt keys, types, references between sections (accounts named by rules and calendar purposes), formats and duplicates. Every problem is reported with its line and column. Exits with the config error code if anything is wrong; warnings alone don't fail.

- `PATH`: The config file. Defaults to FAMILIAR_CONFIG, then the platform default (see `fam config path`).

## contacts.search

Find people to write to. Access: `read`.

```
fam contacts search [QUERY]
```

Searches Outlook contacts and everyone mail came from or went to, by the start of a name, address or company, best first: contacts, then people written to most, then heard from. Reads the store, so it works without famd. Each result's mailbox field is ready for --to.

- `--account <account>`: Only this account (its name in config.toml).
- `--limit <limit>`: How many to show, 1 to 200. Default 20.
- `QUERY`: The start of a name, address or company (or of any word in a name). Empty lists the people written to most.

## draft.create

Save a new draft. Access: `draft`.

```
fam draft create
```

Saves a new message in Drafts, with the account's new-message signature, and queues it for the server. Send it later with fam draft send.

- `--account <account>`: The account to write from (config name or address); needed with more than one.
- `--attach <attach>` (repeatable): Attach a file (up to 3 MB); repeat for more.
- `--bcc <bcc>` (repeatable): Bcc.
- `--body <body>`: Body text (plain text unless --html).
- `--body-file <body-file>`: Read the body from a file.
- `--cc <cc>` (repeatable): Cc.
- `--dry-run`: Only show what would happen.
- `--from <from>`: Send from this alias of the account (listed in `aliases` in config.toml).
- `--html`: The body is HTML.
- `--importance <importance>`: low, normal or high.
- `--inline <inline>` (repeatable): Add an inline image, referenced in an HTML body as cid:<file name>.
- `--no-signature`: Leave the account's signature off.
- `--subject <subject>`: Subject.
- `--to <to>` (repeatable): To: an address or `Name <address>`; repeat, or separate with commas.
- `--wait`: Wait until the server has the draft (needs famd).
- `--wait-secs <wait-secs>`: Seconds --wait waits at most (default 60).

## draft.forward

Save a forward as a draft. Access: `draft`.

```
fam draft forward <MESSAGE>
```

Saves a forward of a message (with its attachments) in Drafts, with your text and the reply signature above it.

- `--attach <attach>` (repeatable): Attach another file; repeat for more.
- `--bcc <bcc>` (repeatable): Bcc.
- `--body <body>`: Body text, above the forwarded message.
- `--body-file <body-file>`: Read the body from a file.
- `--cc <cc>` (repeatable): Cc.
- `--dry-run`: Only show what would happen.
- `--from <from>`: Send from this alias of the account; defaults to the alias the original was sent to.
- `--html`: The body is HTML.
- `MESSAGE` (required): Local ID of the message to forward.
- `--no-signature`: Leave the account's reply signature off.
- `--to <to>` (repeatable): To.
- `--wait`: Wait until the server has the draft (needs famd).
- `--wait-secs <wait-secs>`: Seconds --wait waits at most (default 60).

## draft.reply

Save a reply as a draft. Access: `draft`.

```
fam draft reply <MESSAGE>
```

Saves a reply (or reply all with --all) in Drafts, threaded with the original, with your text and the reply signature above the quoted message.

- `--all`: Reply to everyone on it, not just the sender.
- `--attach <attach>` (repeatable): Attach a file; repeat for more.
- `--bcc <bcc>` (repeatable): Bcc.
- `--body <body>`: Body text, above the quoted original (plain text unless --html).
- `--body-file <body-file>`: Read the body from a file.
- `--cc <cc>` (repeatable): Cc (replaces the default with --all).
- `--dry-run`: Only show what would happen.
- `--from <from>`: Send from this alias of the account; defaults to the alias the original was sent to.
- `--html`: The body is HTML.
- `--inline <inline>` (repeatable): Add an inline image.
- `MESSAGE` (required): Local ID of the message to reply to.
- `--no-signature`: Leave the account's reply signature off.
- `--to <to>` (repeatable): Replace the recipients (default: the sender, or everyone with --all).
- `--wait`: Wait until the server has the draft (needs famd).
- `--wait-secs <wait-secs>`: Seconds --wait waits at most (default 60).

## draft.send

Send a draft. Access: `send`.

```
fam draft send <DRAFT>
```

Queues a draft to send after the undo-send delay ([app] send_delay_seconds; --now skips it). Undo with fam ops cancel <op> before then. Offline, it waits in the queue until it can go.

- `DRAFT` (required): Local ID of the draft.
- `--dry-run`: Only show what would happen.
- `--now`: Skip the undo-send delay.
- `--wait`: Wait until it has gone (needs famd; includes the undo-send delay).
- `--wait-secs <wait-secs>`: Seconds --wait waits at most (default 60).
- `--yes`: Send even to many outside recipients who would see each other.

## draft.update

Change a draft. Access: `draft`.

```
fam draft update <DRAFT>
```

Changes a draft's recipients, subject or body, or adds and removes attachments. Recipient flags replace that line; --body replaces the whole body, and inline images it no longer shows are taken off.

- `--attach <attach>` (repeatable): Attach a file; repeat for more.
- `--bcc <bcc>` (repeatable): Replace Bcc (`""` empties it).
- `--body <body>`: Replace the whole body (plain text unless --html; no signature is added).
- `--body-file <body-file>`: Replace the whole body from a file.
- `--cc <cc>` (repeatable): Replace Cc (`""` empties it).
- `DRAFT` (required): Local ID of the draft.
- `--dry-run`: Only show what would happen.
- `--from <from>`: Send from this alias, or the account's own address.
- `--html`: The body is HTML.
- `--inline <inline>` (repeatable): Add an inline image.
- `--remove-attachment <remove-attachment>` (repeatable): Take an attachment off, by file name (or Graph attachment ID); repeat for more.
- `--subject <subject>`: New subject.
- `--to <to>` (repeatable): Replace To (`""` empties it).
- `--wait`: Wait until the server has the change (needs famd).
- `--wait-secs <wait-secs>`: Seconds --wait waits at most (default 60).

## folder.color

Colour a mail folder in the folder pane. Access: `write`.

```
fam folder color <FOLDER> <COLOR>
```

Gives a folder a colour in the desktop app's folder pane, or none to clear it. Kept on this device only (folders.json in the data directory), not on the server; it follows the folder through a rename or move.

- `--account <account>`: The account, when more than one has the folder.
- `COLOR` (required): red, orange, yellow, green, teal, blue, purple, pink, grey, or none.
- `FOLDER` (required): The folder (path, or name).

## folder.create

Create a mail folder. Access: `write`.

```
fam folder create <NAME>
```

Creates a folder on this device at once (messages can be moved into it straight away) and queues it for the server.

- `--account <account>`: The account (config name or address); needed with more than one.
- `--dry-run`: Only show what would change.
- `NAME` (required): The new folder's name.
- `--parent <parent>`: Parent folder (path, well-known name or name). Default: the top level.
- `--wait`: Wait until the server has the change (needs famd).
- `--wait-secs <wait-secs>`: Seconds --wait waits at most (default 60).

## folder.delete

Delete a mail folder (to Deleted Items). Access: `delete`.

```
fam folder delete <FOLDER>
```

Moves a folder and everything in it into Deleted Items, as Outlook does. With --permanent (and --yes), deletes a folder already in Deleted Items for good.

- `--account <account>`: The account; needed with more than one.
- `--dry-run`: Only show what would change.
- `FOLDER` (required): The folder (path, or name).
- `--permanent`: Delete for good. Only for a folder already in Deleted Items; needs --yes.
- `--wait`: Wait until the server has the change (needs famd).
- `--wait-secs <wait-secs>`: Seconds --wait waits at most (default 60).
- `--yes`: Confirm a permanent delete when not interactive.

## folder.list

List mail folders. Access: `read`.

```
fam folder list
```

Lists every folder on this device by account, as a tree, with message and unread counts. Reads the local store, so it works offline and without famd. The path column is what fam mail list --folder takes.

- `--account <account>`: Only this account (its name in config.toml).

## folder.move

Move a mail folder under another, or to the top level. Access: `write`.

```
fam folder move <FOLDER>
```

Moves a folder (with everything in it) under --to, or to the top level with --top, on this device at once and queued for the server.

- `--account <account>`: The account; needed with more than one.
- `--dry-run`: Only show what would change.
- `FOLDER` (required): The folder (path, or name).
- `--to <to>`: The new parent folder.
- `--top`: Move it to the top level instead.
- `--wait`: Wait until the server has the change (needs famd).
- `--wait-secs <wait-secs>`: Seconds --wait waits at most (default 60).

## folder.pin

Pin a mail folder to the top of the folder pane. Access: `write`.

```
fam folder pin <FOLDER>
```

Lists a folder below All inboxes in the desktop app's folder pane. Kept on this device only (folders.json in the data directory), not on the server.

- `--account <account>`: The account, when more than one has the folder.
- `FOLDER` (required): The folder (path, or name).

## folder.rename

Rename a mail folder. Access: `write`.

```
fam folder rename <FOLDER> <NAME>
```

Renames a folder on this device at once and queues it for the server. Built-in folders can't be renamed.

- `--account <account>`: The account; needed with more than one.
- `--dry-run`: Only show what would change.
- `FOLDER` (required): The folder (path, or name).
- `NAME` (required): Its new name.
- `--wait`: Wait until the server has the change (needs famd).
- `--wait-secs <wait-secs>`: Seconds --wait waits at most (default 60).

## folder.unpin

Unpin a mail folder. Access: `write`.

```
fam folder unpin <FOLDER>
```

Stops listing a pinned folder below All inboxes in the desktop app's folder pane.

- `--account <account>`: The account, when more than one has the folder.
- `FOLDER` (required): The folder (path, or name).

## mail.archive

Move messages to Archive. Access: `write`.

```
fam mail archive <MESSAGES>...
```

Moves messages to their account's Archive folder, on this device at once and queued for the server.

- `--dry-run`: Only show what would change.
- `MESSAGES` (required, repeatable): Local message IDs.
- `--wait`: Wait until the server has the change (needs famd).
- `--wait-secs <wait-secs>`: Seconds --wait waits at most (default 60).

## mail.attachments

List or save a message's attachments. Access: `read`.

```
fam mail attachments <MESSAGE>
```

Lists a message's attachments with their names, types and sizes, read from the server through famd (attachments aren't kept on this device). --save DIR downloads them into a folder, never replacing a file that is already there; --name picks one. Inline images are left out unless --inline. Attached Outlook items and cloud links are listed but can't be saved.

- `--inline`: Include inline images (the ones shown inside the body).
- `MESSAGE` (required): The message's local ID.
- `--name <name>`: Only the attachment with this name or ID (with --save).
- `--save <save>`: Save the attachments into this folder. A file that already exists is kept and the new one gets a number, like `report (2).pdf`.

## mail.check-spam

Check how likely an email is to be spam or a scam. Access: `read`.

```
fam mail check-spam <MESSAGE>
```

Shows what the mail server recorded about the email (SPF, DKIM, DMARC, Outlook's spam level and folder), how well you know the sender, and what its links and attachments look like. With spam_check = true on the account and [ai] enabled in config.toml, the AI tool (Claude or Codex) also grades it: spam and scam from 0 to 100, with reasons. Nothing on the server is changed; the graded result is kept on this device (--saved shows it again). Report it with fam mail report. Headers and attachments are read through famd.

- `MESSAGE` (required): The message's local ID.
- `--no-ai`: Only the facts: don't send the email to the AI tool.
- `--saved`: Don't check: show the result saved by an earlier check, if any.

## mail.delete

Move messages to Deleted Items. Access: `delete`.

```
fam mail delete <MESSAGES>...
```

Moves messages to their account's Deleted Items folder, on this device at once and queued for the server. With --permanent (and --yes), deletes messages already in Deleted Items for good.

- `--dry-run`: Only show what would change.
- `MESSAGES` (required, repeatable): Local message IDs.
- `--permanent`: Delete for good. Only for messages already in Deleted Items; needs --yes.
- `--wait`: Wait until the server has the change (needs famd).
- `--wait-secs <wait-secs>`: Seconds --wait waits at most (default 60).
- `--yes`: Confirm a permanent delete when not interactive.

## mail.fetch

Show a message with its text. Access: `read`.

```
fam mail fetch <MESSAGE>
```

Shows a message's header and body. Bodies in the sync window are already on this device; an older one is downloaded through famd into the size-limited cache (sync.cache_limit). --local never downloads. --format html returns the body as stored instead of plain text.

- `--format <text|html>`: How to return a body.
- `--local`: Don't download: show the body only if it is already on this device.
- `MESSAGE` (required): The message's local ID.

## mail.flag

Flag messages for follow-up, or clear the flag. Access: `write`.

```
fam mail flag <MESSAGES>...
```

Flags messages (or clears their flags with --clear) on this device at once and queues the change for the server.

- `--clear`: Clear the flag instead.
- `--dry-run`: Only show what would change.
- `MESSAGES` (required, repeatable): Local message IDs.
- `--wait`: Wait until the server has the change (needs famd).
- `--wait-secs <wait-secs>`: Seconds --wait waits at most (default 60).

## mail.list

List messages in a folder. Access: `read`.

```
fam mail list
```

Lists a folder's messages newest first, from this device, so it works offline and without famd. Without --account it lists that folder in every account, merged. --folder takes a name, a path such as Inbox/Club, or a well-known name such as sentitems; the default is the Inbox. --after takes the next-page token.

- `--account <account>`: Only this account (its name in config.toml, or its address). Default: every account.
- `--after <after>`: The next-page token from a previous list.
- `--flagged`: Only flagged messages.
- `--folder <folder>`: Folder name, path (Inbox/Club) or well-known name (inbox, sentitems, drafts, deleteditems, archive, junkemail). Default: inbox.
- `--limit <limit>`: Most messages per page (default 50, at most 500).
- `--unread`: Only unread messages.

## mail.move

Move messages to another folder. Access: `write`.

```
fam mail move <MESSAGES>...
```

Moves messages to a folder of the same account on this device at once and queues the move for the server.

- `--dry-run`: Only show what would change.
- `MESSAGES` (required, repeatable): Local message IDs.
- `--to <to>` (required): Destination folder: path (Inbox/Club), well-known name or name, in each message's own account.
- `--wait`: Wait until the server has the change (needs famd).
- `--wait-secs <wait-secs>`: Seconds --wait waits at most (default 60).

## mail.pin

Keep a message's body on this device. Access: `read`.

```
fam mail pin <MESSAGE>
```

Pins a message so its body stays on this device even outside the sync window, downloading it first if needed. --unpin lets it age out again. Needs famd.

- `MESSAGE` (required): The message's local ID.
- `--unpin`: Unpin instead: the body becomes an ordinary cached body again.

## mail.process

Move messages to their account's processed folder. Access: `write`.

```
fam mail process <MESSAGES>...
```

Moves messages to the folder set as processed_folder for their account in config.toml, on this device at once and queued for the server. The folder is created at the top level if the account doesn't have it yet.

- `--dry-run`: Only show what would change.
- `MESSAGES` (required, repeatable): Local message IDs.
- `--wait`: Wait until the server has the change (needs famd).
- `--wait-secs <wait-secs>`: Seconds --wait waits at most (default 60).

## mail.read

Mark messages read or unread. Access: `write`.

```
fam mail read <MESSAGES>...
```

Marks messages read (or unread with --unread) on this device at once and queues the change for the server. Works offline; famd sends queued changes when it can.

- `--dry-run`: Only show what would change.
- `MESSAGES` (required, repeatable): Local message IDs (from fam mail list).
- `--unread`: Mark unread instead.
- `--wait`: Wait until the server has the change (needs famd).
- `--wait-secs <wait-secs>`: Seconds --wait waits at most (default 60).

## mail.report

Report messages as junk, phishing or not junk. Access: `write`.

```
fam mail report <MESSAGES>...
```

Reports messages to the mail provider as junk (the default), phishing (--phishing) or not junk (--not-junk), which trains its filter. Junk and phishing go to Junk Email; not junk goes back to the Inbox. The move shows on this device at once and the report is queued for the server.

- `--dry-run`: Only show what would change.
- `MESSAGES` (required, repeatable): Local message IDs.
- `--not-junk`: Report them as not junk, moving them from Junk Email to the Inbox.
- `--phishing`: Report them as phishing (a scam) instead of junk.
- `--wait`: Wait until the server has the report (needs famd).
- `--wait-secs <wait-secs>`: Seconds --wait waits at most (default 60).

## mail.search

Search mail across all accounts. Access: `read`.

```
fam mail search <QUERY>
```

Searches every account on this device instantly, offline included: every header, and message text for the sync window. Operators: from: to: subject: body: has:attachment before: after: (YYYY-MM-DD, YYYY-MM, YYYY or an age such as 30d) folder: account: is:unread is:read is:flagged, "phrases" and -exclusions. With --scope all the words are also searched on the server (through famd), so older message text is found too; those results are labelled online. Results are newest first; --after takes the next-page token.

- `--after <after>`: The next-page token from a previous search.
- `--limit <limit>`: Most results per page (default 50, at most 500).
- `QUERY` (required): The query, for example 'from:club subject:"AGM" has:attachment after:2026-01-01'. Operators: from: to: subject: body: has:attachment before: after: folder: account: is:unread is:read is:flagged, "phrases" and -exclusions.
- `--scope <local|all>`: Where to search.

## mail.send

Write and send a message. Access: `send`.

```
fam mail send
```

Saves a new message as a draft with the account's signature and queues it to send after the undo-send delay (--now skips it). Undo with fam ops cancel <op> before then.

- `--account <account>`: The account to send from (config name or address); needed with more than one.
- `--attach <attach>` (repeatable): Attach a file (up to 3 MB); repeat for more.
- `--bcc <bcc>` (repeatable): Bcc.
- `--body <body>`: Body text (plain text unless --html).
- `--body-file <body-file>`: Read the body from a file.
- `--cc <cc>` (repeatable): Cc.
- `--dry-run`: Only show what would happen.
- `--from <from>`: Send from this alias of the account (listed in `aliases` in config.toml).
- `--html`: The body is HTML.
- `--importance <importance>`: low, normal or high.
- `--inline <inline>` (repeatable): Add an inline image.
- `--no-signature`: Leave the account's signature off.
- `--now`: Skip the undo-send delay.
- `--subject <subject>`: Subject.
- `--to <to>` (repeatable): To: an address or `Name <address>`; repeat, or separate with commas.
- `--wait`: Wait until it has gone (needs famd; includes the undo-send delay).
- `--wait-secs <wait-secs>`: Seconds --wait waits at most (default 60).
- `--yes`: Send even to many outside recipients who would see each other.

## mail.thread

Show a whole conversation. Access: `read`.

```
fam mail thread <MESSAGE>
```

Shows the conversation a message belongs to, oldest first, across every folder of its account: replies in Sent Items, drafts and moved messages included, with copies of one message shown once. Reads this device only. Use fam mail fetch for a message's text.

- `MESSAGE` (required): The local ID of any message in the conversation.
- `--no-trash`: Leave out messages in Deleted Items and Junk Email.

## mcp.serve

Serve the commands to an MCP client over stdio. Access: `read`.

```
fam mcp serve
```

Speaks MCP (JSON-RPC, one message per line) on stdin and stdout until stdin closes. Each command is a tool named like mail_list; the same server also answers each command by its dotted name (mail.list) as a plain JSON-RPC method. Only commands within --allow are offered (default read,draft); writes are made as source mcp and checked by famd against [agent] in config.toml, and logged in the audit log.

- `--allow <allow>` (repeatable): Access levels whose commands are offered, comma-separated or repeated: read, draft, write, send, delete, rules. Default read,draft. famd still checks each write against [agent] in config.toml.

## ops.cancel

Cancel a change that hasn't reached the server. Access: `write`.

```
fam ops cancel <ID>
```

Removes a queued or failed change and undoes it on this device. A change already on its way or done can't be cancelled.

- `ID` (required): The op's ID (from fam ops list).

## ops.list

List changes waiting for the server. Access: `read`.

```
fam ops list
```

Lists queued changes that haven't reached the server yet, and ones that failed. --all adds those done in the last hour.

- `--all`: Also show changes the server got in the last hour.
- `--limit <limit>`: Most rows (default 100).

## ops.retry

Try a failed change again. Access: `write`.

```
fam ops retry <ID>
```

Queues a failed change again, making its local change again first.

- `ID` (required): The op's ID (from fam ops list).

## schema

Describe every command, its input, output and errors. Access: `read`.

```
fam schema
```

Prints the command schema that the CLI, MCP tools and GUI bindings are generated from. Use --json for machine-readable output.

- `--format <commands|mcp|typescript>`: Which description to produce.

## sync.check

Compare the store with the server and report drift. Access: `read`.

```
fam sync check
```

Lists every folder and message on the server and compares them with this device's store: folders, which folder each message is in, read state, flags, received times, bodies against the sync window, and the store's own counters. Differences are re-checked after a sync round so changes still arriving are not reported. Nothing is fixed. Lists every message, so it takes a while on a large mailbox. Needs famd. Exit code 0 either way; `clean` says whether drift was found.

- `--account <account>`: Only this account (its name in config.toml).
- `--limit <limit>`: Most differences listed per account (default 50); counts are always complete.
- `--no-settle`: Compare at once, without first waiting for a sync round. Changes still syncing then show as drift.

## sync.status

Show how far each account's sync has got. Access: `read`.

```
fam sync status
```

Shows, per account, what famd is doing (first sync, bodies, idle), how many headers and bodies are on this device, what is still to fetch for the sync window, pinned and cached bodies, and any folder errors. Counts are read from the store, so they show even when famd is not running.

- `--account <account>`: Only this account (its name in config.toml).

## ui.folder

Show a folder in the app. Access: `read`.

```
fam ui folder [FOLDER]
```

Shows a folder's messages in the running desktop app and brings the window to the front. With no folder, shows every account's Inbox together.

- `--account <account>`: The account the folder is in (its name in config.toml). Needed when a folder is given and there is more than one account.
- `FOLDER`: Folder path (Inbox/Club) or well-known name (inbox, sentitems, drafts, archive). Default: every account's Inbox together.

## ui.open

Open a message in the app. Access: `read`.

```
fam ui open <MESSAGE>
```

Shows the message in the running desktop app: its folder in the list and the message in the reading pane, and brings the window to the front.

- `MESSAGE` (required): Local ID of the message to open.

## version

Show the Familiar version. Access: `read`.

```
fam version
```

Shows the Familiar version and the command schema version.

- `--verbose`: Also report the operating system and CPU architecture.
