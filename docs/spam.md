# Checking an email for spam or a scam

AI spam plan ([designs/AI Spam and Scam Check Plan.md](../designs/AI%20Spam%20and%20Scam%20Check%20Plan.md)),
WP S.1 to S.3, plus a background check. In the desktop app: right-click a message, Check for spam or scam.

```sh
fam mail check-spam 4182
fam mail check-spam 4182 --no-ai --json
fam mail check-spam 4182 --saved
```

Outlook has already filtered your mail by the time Familiar sees it. This
check is a second opinion on a message you're not sure about. It has two
parts:

- **Facts**, which Familiar works out itself and always shows.
- **A grade** from the AI tool you already have (Claude or Codex, as set in
  `[ai]`): spam and scam, each from 0 to 100, with reasons quoting the
  email. The tool is asked only when it is turned on (see below).

Nothing on the server is changed. To report the message, use
`fam mail report <id>` (junk), `--phishing` or `--not-junk`.

## Turning it on

The facts work for every account. The AI grade needs both of these in
`config.toml` (`fam config path` shows where):

```toml
[ai]
enabled = true

[accounts.hotmail]
# ...
spam_check = true
```

`[ai] accounts`, if set, must include the account too. `fam config validate`
warns when `spam_check` is on but the AI wouldn't be asked.

## The facts

| Fact | From | Points to |
| --- | --- | --- |
| `spf`, `dkim`, `dmarc`, `compauth` | The receiving server's `Authentication-Results`: only the topmost one counts, because a sender can add its own lower down. `-Original` and `-Untrusted` copies are ignored | `scam` on `fail` |
| `scl`, `bcl`, `category`, `delivered_to` | Outlook's verdict: spam level, bulk complaint level, Microsoft's category, and Inbox or Junk | `spam` at SCL 5+, BCL 7+, Junk; `scam` for phishing, spoofing or malware categories |
| `contact`, `written_to`, `domain_written_to`, `received_before` | Your mailbox: whether you've written to the address or its domain, whether it's a contact, how often it has written before | `safe` when you know them |
| `reply_to` | Reply-To on another domain than From | `scam` |
| `display_name` | The sender's name shows another address or domain (`"ANZ <alerts@anz.co.nz>" <x@other.example>`) | `scam` |
| `lookalike`, `punycode` | The sender's domain is within two letters of one you write to, or uses look-alike Unicode letters | `scam` |
| `link_mismatch`, `link_ip`, `link_userinfo`, `link_punycode`, `link_shortener`, `link_domains` | Links whose text names one site but go to another, to bare IP addresses, with `name@` tricks, look-alike letters, shorteners; the sites linked to | `scam` (shorteners: neither) |
| `attachment` | Attachment types used for phishing or malware (`.htm`, `.svg`, `.iso`, `.lnk`, macros, scripts) | `scam` |
| `unsubscribe` | A `List-Unsubscribe` header, as legitimate bulk mail has | neither |

Headers and attachment names are read from the server through famd. When
famd isn't running, the facts from your mailbox and the body are still
given, with a note. Links are never opened.

## The grade

The tool gets the facts, the subject, sender, Reply-To, number of
recipients, the links (text and real site), attachment names and up to
20,000 characters of text. It is told the email is untrusted and to ignore
anything it says about how it should be judged. It runs as described in
[ai.md](ai.md), with `FAMILIAR_AGENT=ai-spam`.

The answer is checked (numbers clamped to 0 to 100, at most five reasons)
and given a headline from the higher of the two numbers:

| Higher score | Headline |
| --- | --- |
| 0 to 29 | Looks fine |
| 30 to 69 | Be careful |
| 70 to 100 | Likely scam, or Likely spam (whichever is higher) |

The numbers are grades, not probabilities: AI models' stated confidence
runs high, so the headline carries the meaning.

The AI isn't asked when an agent or the MCP server runs the check (an agent
asking another agent), or with `--no-ai`. If the tool fails or times out,
you get the facts and a note.

## Checking new mail on its own

With `spam_auto = true` on an account (instead of, or as well as,
`spam_check`), famd checks the account's new mail in the background:

```toml
[accounts.hotmail]
# ...
spam_auto = true
```

Once a minute it looks for mail in Inbox and Junk Email that arrived in
the last day, has its body on this device and hasn't been checked, and
checks it one message at a time, as `fam mail check-spam` would. Each
check is one call to the AI tool. The grade is saved (below); a check
that gives no grade (the tool failed) isn't tried again until famd
restarts, and famd's log says why. The setting is read again every round,
so turning it off needs no restart.

In the desktop app, a grade stands out under the reading pane's header
only when it disagrees with where the message is: 50% or more outside
Junk Email, or under 50% in it. Otherwise it shows only while the pointer
is over the header.

## Saved results

A graded result is kept on this device in `spam_checks.json` in the data
directory, by account and the message's server ID, so it survives moves
and restarts. `--saved` shows it without checking again (exit code 6 when
there is none).
