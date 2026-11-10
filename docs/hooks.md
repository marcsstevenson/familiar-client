# Hooks and `fam watch`

Familiar can run your own command when new mail arrives, and stream new
mail to a script as JSON lines.

## Hooks

Add a `[[hooks]]` entry to `config.toml` for each command:

```toml
[[hooks]]
name = "club"                 # for famd's log; defaults to hooks[<n>]
on = "new_mail"               # the only event for now
match = "from:club"           # a search, as in fam mail search; leave out for every message
run = "notify-send \"Club mail\" \"$FAMILIAR_SUBJECT\""
# enabled = true
# timeout_seconds = 60
```

famd runs the hooks. It reads `config.toml` again each time new mail comes,
so adding or changing a hook needs no restart. `fam config validate` checks
the entries.

- **What counts as new mail.** A message that arrives after famd started,
  in any folder except Sent Items, Drafts, Outbox, Deleted Items and Junk,
  received within the last day. Mail that arrived while famd was off counts
  when famd syncs it at the next start (as long as it is under a day old).
  Mail synced during an account's first sync doesn't count, and nor does a
  message you move between folders.
- **When.** famd looks every 10 seconds, once the message's body is local
  and the search index has it, so `match = "body:..."` sees the text. A
  message whose body hasn't arrived after five minutes runs its hooks
  anyway, matched on its headers.
- **`match`** is any `fam mail search` query: `from:`, `to:`, `subject:`,
  `body:`, plain words, `folder:`, `account:`, `has:attachment`,
  `is:unread` and so on, all of which must match.
- **How it runs.** The command line goes to the system shell: `sh -c` on
  macOS and Linux, `cmd /C` on Windows. Hooks run one at a time. A command
  still running after `timeout_seconds` is stopped, as is one running when
  famd stops. Its output goes to famd's log, with a line saying how it
  ended.

### What the command gets

The message is never pasted into the command line, so a subject can't run
anything. It comes two ways:

- **stdin**: one line of JSON, the same object `fam watch` prints (below).
- **environment**: `FAMILIAR_HOOK` (the hook's name), `FAMILIAR_EVENT`
  (`new_mail`), `FAMILIAR_MESSAGE_ID`, `FAMILIAR_ACCOUNT`,
  `FAMILIAR_FOLDER`, `FAMILIAR_FROM`, `FAMILIAR_FROM_NAME`,
  `FAMILIAR_SUBJECT`.

Quote the variables in your command (`"$FAMILIAR_SUBJECT"`, or
`%FAMILIAR_SUBJECT%` in `cmd`) and treat their contents as untrusted:
anyone can send you an email with any subject. `FAMILIAR_MESSAGE_ID` works
with the other commands, for example `fam mail thread $FAMILIAR_MESSAGE_ID`.

On Windows, a PowerShell script works as
`run = "powershell -NoProfile -File C:\\Users\\me\\club.ps1"`; read the
event with `$env:FAMILIAR_SUBJECT` or `[Console]::In.ReadLine() | ConvertFrom-Json`.

## `fam watch`

```
fam watch [--match <query>]
```

Prints one JSON object per new message as famd syncs it, until you stop it
(Ctrl+C) or whatever reads its output goes away (`fam watch | head -1`).
famd must be running for mail to arrive; `fam watch` reads the store
itself, so it works alongside the desktop app. What counts as new mail is
the same as for hooks, counted from when `fam watch` started.

```json
{"event":"new_mail","id":10432,"account":"hotmail","folder":"Inbox",
 "from":{"name":"Parklands Tennis","addr":"news@club.example.org"},
 "to":[{"name":"Pat","addr":"me@example.com"}],
 "subject":"Club night","received_at":1794300000,
 "preview":"Training moves to Thursday.","has_attachments":false}
```

(One line per message in real output.) `--json` is accepted and changes
nothing. `fam watch` isn't offered over MCP, because it never returns.
