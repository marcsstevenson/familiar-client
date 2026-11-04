# Email Client Design Document

Nov 4, 2026 · @Marc Stevenson

## Summary

Build a local-first email and calendar client as a Rust engine with thin surfaces on top: a Tauri desktop app, a CLI, an MCP server and a local API. It targets Microsoft accounts first, and it covers mail, calendar, search and rules across your three accounts.

The key decisions are:

- **Full CLI control:** every action is one typed command, and all surfaces are generated from it, so the CLI can't fall behind the GUI. Agents use the same CLI in place of Composio and the Microsoft 365 connector.
- **Easy reinstall:** one `config.toml` holds accounts, signatures, rules, views and keybindings, while tokens stay in the OS keychain. A reinstall is config, sign-in and sync.
- **Decades of mail on demand:** the device keeps one year of full messages plus headers for all history, and fetches older bodies and body-text search results from the server when needed.
- **Headless by design:** only the desktop build needs a webview, so the engine runs on its own in a container.
- **Speed:** the UI reads only from local storage, with targets enforced in CI.

Tauri is the recommended UI because HTML email needs a web engine anyway, but it needs to prove itself against GPUI in a Phase 0 test. Phase 0 confirmed Tauri (Windows: 442 ms start, 11 ms keypress p95, 30 ms open p95).

## Goals and principles

The client replaces Outlook on the desktop for one person running several Microsoft accounts, and everything it can do is scriptable.

**Goals**

- Handle day-to-day mail and calendar work across all accounts from one app.
- Give the CLI full parity with the GUI, enforced by the architecture rather than by discipline.
- Replace Composio and the Claude Microsoft 365 connector for mail and calendar, so agents use the local CLI instead.
- Rebuild a working install from one config file plus sign-in.
- Feel instant: no spinner for anything already on the device.
- Run on Windows, macOS and Linux, and without a display in a container.
- Publish as an open source project, with personal config and mail kept out of the repo.

**Non-goals for version 1**

- Multi-user or organisation-wide administration.
- A hosted service. There is no vendor server between the client and Microsoft.
- Mobile apps. The core should not rule them out, but they are not designed here.
- Outlook features you don't use: shared mailboxes, delegates, categories, Quick Steps, Focused Inbox and PST archives.
- SharePoint and OneDrive features, beyond handling attachments.

**Principles**

1. **One engine.** GUI, CLI and any automation call the same typed commands.
2. **Local first.** Reads come from the local store and writes are queued, so the network never blocks the interface.
3. **Config is text.** Anything a person sets lives in a readable file, except secrets.
4. **Safe by default.** Destructive actions can be previewed and undone wherever the server allows it.

## How Outlook is used today

This profile comes from past conversations, your answers to the design questions and a read-only check of the Hotmail mailbox on 4 November 2026.

| What has been observed | What it means for the design |
| --- | --- |
| New Outlook on Windows, with three Microsoft accounts: personal Hotmail, Parklands Cricket Club and iM3 Dental | Multi-account is the core model, and consumer and business sign-in must both work |
| Hotmail goes back to May 2006 and holds about 9,500 visible messages in 46 folders | Headers for all history are small enough to keep locally, while full messages stay limited to one year |
| `$search` on Hotmail messages works and reached mail from 2011 when tested, though an earlier chat recorded a search failure | Older message text can be searched on the server, and the local index gives instant results for recent mail and all headers |
| A complete thread needed separate Inbox and Sent Items queries | Threads are assembled locally across all folders |
| Project mail is found by subject tag, sender and attachment presence | Search needs field operators and an attachment filter |
| Signatures differ by account | Each account has its own signatures, saved in the config |
| Events are often created from an email, including school, club and coaching dates | "Create event from this email" is a first-class command |
| Family and school events go on a calendar called Family Room, not the default | Named calendars can be targeted, with a configurable default for each purpose |
| One past session found Graph handled `New Zealand Standard Time` more reliably than `Pacific/Auckland` on the consumer account, though a later event worked with the IANA name | The core stores IANA zones and maps them to Windows zone names at the API boundary |
| Outbound team mail was drafted and reviewed before sending, with a question over To versus BCC | Draft-first is the default for automation, with a warning for large visible recipient lists |
| A rule was wanted to delete Junk mail from a matching sender address | Rules need sender-contains conditions, ordering and a safer delete option |
| Local mail access through a CLI or MCP was wanted, without a cloud connector in between | The client includes a local MCP server, so assistants reach mail without a third party |

## Functional requirements

Version 1 must cover mail, calendar, search, rules and contacts lookup for all three accounts. Everything marked V1 is needed before Outlook can be retired.

| Area | Requirement | Release |
| --- | --- | --- |
| Accounts | Multiple personal and Microsoft 365 accounts, with a unified inbox and a view per account | V1 |
| Accounts | Choose the sending identity per message, with a default for each account | V1 |
| Mail | Read, reply, reply all, forward, drafts, send, archive, move, delete, flag and mark read | V1 |
| Mail | Conversation view assembled across folders, including Sent Items | V1 |
| Mail | Full folder tree: create, rename, move and delete | V1 |
| Mail | Full messages stored for a configurable window, one year by default, with headers kept for all history | V1 |
| Mail | Older messages load on demand and can be pinned for offline use | V1 |
| Mail | Attachments: preview, save, add by drag and drop, and download on demand | V1 |
| Mail | Offline reading within the window, and an outbox that sends when the connection returns | V1 |
| Mail | Undo send through a short configurable delay | V1 |
| Compose | Rich text and plain text, inline images and contact autocomplete | V1 |
| Compose | Separate signatures for new messages and replies on each account, switched automatically with the sending identity and saved in the config | V1 |
| Compose | Warn when many outside recipients are visible in To or Cc, and offer Bcc | V1 |
| Search | Instant local search across all accounts, with sender, date, folder, subject and attachment operators | V1 |
| Search | Body-text search of older mail through the server, merged with local results and labelled as online | V1 |
| Search | Saved searches shown as smart folders | V1 |
| Search | Text inside attachments | Later |
| Rules | Create, order, enable and test server rules, and manage them from config | V1 |
| Rules | Local rules for conditions the server cannot express, such as Junk-folder clean-up | V1 |
| Calendar | Day, week, month and agenda views, with multiple calendars shown together | V1 |
| Calendar | Create, edit, RSVP, recurring events, reminders and time zones | V1 |
| Calendar | Create an event from an email, keeping a link back, and suggest details from the text | V1 |
| Calendar | Target a named calendar, such as Family Room, with defaults by purpose | V1 |
| Contacts | Read and search Outlook contacts, plus a local list of past correspondents | V1 |
| Contacts | Full contact editing | Later |
| Automation | CLI, MCP server and local API with full parity | V1 |
| Migration | Import server rules and folder favourites on first run, and copy signatures across once | V1 |
| Tasks | Microsoft To Do integration | Later |
| Providers | Gmail, generic IMAP and JMAP | Later |

The design leaves room for these later items through a provider interface, but version 1 ships and tests only Microsoft Graph.

## CLI and configuration

The CLI gets full control because the GUI has no private route into the engine. Every user action is a typed command, and both surfaces call the same command registry. The command is called `fam`, and the project is Familiar.

### How full control is guaranteed

- Each command is defined once in Rust, with typed input, output and errors.
- The CLI arguments, JSON schema, MCP tools and GUI bindings are generated from that definition.
- A CI test fails if a registered command is missing from any surface.
- `fam api` calls Microsoft Graph directly with the account's token, as an escape hatch for anything not yet modelled.
- GUI actions such as opening a message or a view are commands too, through `fam ui`.

### Command model

```bash
fam mail list --account parklands --unread --json
fam mail search 'from:club subject:"AGM" has:attachment after:2026-01-01'
fam mail search 'invoice before:2015-01-01' --scope all
fam mail thread <id>
fam mail fetch <id>
fam mail pin <id>
fam mail draft --to a@example.com --subject "Team update" --body-file note.md
fam mail send <draft-id> --yes
fam mail move <id> --folder Archive

fam signature list
fam signature show --account parklands --type reply

fam cal list --calendar "Family Room" --from today --days 14
fam cal add --purpose family --title "AGM" --start "2026-09-22 19:00" --duration 1h
fam cal add --from-mail <id> --purpose family

fam rules plan
fam rules apply
fam rules test --message <id>

fam auth login --account hotmail --device-code
fam sync status
fam config validate
fam config export > config.toml
fam schema --json
fam mcp serve --allow read,draft
```

Every command follows the same contract:

- `--json` gives output matching a published schema, and tables are for people only.
- `--dry-run` shows the effect of any write.
- Sends, deletes and rule changes require `--yes` when not run interactively.
- Exit codes are stable and documented: for example, usage error, auth required, offline, throttled and not found.
- Queued commands return an operation ID, and `--wait` blocks until the server confirms.

### Configuration file

One TOML file holds everything a person sets, so it can sit in a private Git repo. Changes made in the GUI are written back to it with comments and ordering preserved.

```toml
version = 1

[app]
client_id = "00000000-0000-0000-0000-000000000000"
theme = "system"
send_delay_seconds = 10

[accounts.hotmail]
email = "you@hotmail.com"
tenant = "consumers"

[accounts.hotmail.signature]
new = """
<p>Your Name<br>Your details</p>
"""
reply = """
<p>Your Name</p>
"""

[accounts.parklands]
email = "you@parklandscricket.co.nz"
tenant = "organizations"

[accounts.parklands.signature]
new = """
<p>Your Name<br>Your club role</p>
"""
reply = """
<p>Your Name<br>Your club role</p>
"""

[calendar.purposes]
family = { account = "hotmail", calendar = "Family Room" }

[sync]
window = "1y"
headers = "all"
older_bodies = "on-demand"
cache_limit = "2GB"
attachments = "on-demand"

[[views]]
name = "Club, unread"
query = "account:parklands is:unread"

[[rules]]
name = "Delete firebaseapp spam"
account = "hotmail"
run = "server"
when = { sender_contains = ["firebaseapp"] }
then = { delete = true, stop = true }

[keys]
archive = "e"
reply = "r"

[agent]
allow = ["read", "draft", "calendar-write"]
```

The file never holds passwords, tokens or message data. A Microsoft client ID identifies the app and is not a secret. Signatures contain personal details, so your config belongs in its own private repo, separate from the public code.

### Signatures

Each account has separate signatures for new messages and replies, and the right one follows the sending identity as it changes. They are written as HTML inside `config.toml`, so restoring that one file restores them. Small images are embedded in the file and sent as inline attachments.

[Microsoft Graph has been reported as having no signature API](https://learn.microsoft.com/en-us/answers/a/1841062), so the client can't read them from new Outlook or sync them back. You copy each signature across once during migration, and the config is then the source of truth. Phase 0 rechecks this.

### Agent use

An agent uses the same CLI as you, with no separate connector. For mail and calendar, this replaces Composio and the Claude Microsoft 365 connector, as long as each account can sign in.

- **Discovery:** `fam schema --json` lists every command with its inputs, outputs and errors, and the project ships an agent skill explaining common workflows.
- **Predictable use:** JSON output, stable exit codes, `--dry-run` and no interactive prompts when run by an agent.
- **Access:** `[agent] allow` sets what agent sessions may do, with read and draft as the baseline. Sends stay as drafts for you to approve unless you grant more.
- **MCP is optional:** the MCP server exposes the same commands for tools that can't run a shell.

The CLI runs where the client is installed, so an agent needs a route to that machine: Claude Code, the Claude desktop app, a linked computer or a container you host. From a phone with your computer off there is no route, and that's accepted: agents don't need mail access while your computer is off. Phase 2 passes only when past Composio tasks work through the CLI: finding a thread, saving a draft and creating a Family Room event from an email.

### Rules as code

`fam rules plan` compares the config with the rules on the server and shows what would be created, changed or removed. `fam rules apply` then makes those changes. Server rules [run on the Inbox and support conditions such as sender-contains](https://learn.microsoft.com/en-us/graph/api/mailfolder-post-messagerules), and they keep working when the client is closed. Rules the server cannot express run locally and are labelled `run = "local"`, so it's clear they only fire while the client or daemon is running.

### Reinstalling

1. Install the app with one command, such as `winget`, `brew`, a Linux package or a container image.
2. Restore `config.toml`, usually with `git clone`. Accounts, signatures, rules, views and keybindings all come back with it.
3. Run `fam config validate`.
4. Run `fam auth login` once per account, or sign in when the GUI prompts.
5. Run `fam sync`. Headers for all history arrive first, then a year of message bodies, newest first.

No other backup is needed for server-held mail. Anything only held locally, such as an unsynced outbox item, is flagged before uninstalling.

## Architecture

A single Rust core owns sync, storage, search and rules, and the GUI, CLI, MCP server and local API are thin surfaces over it. Removing the GUI leaves a complete headless client.

&#91;embedded content: client architecture · four surfaces, one core engine\]

The four surfaces call the same command registry, so none has abilities the others lack. Only the desktop build links a webview, which keeps the container image small.

### Processes

- A background daemon, `famd`, owns the sync engine and is the only process that writes to the store.
- The GUI and CLI read SQLite directly for speed and send writes to the daemon over a local socket or named pipe.
- The GUI starts the daemon automatically. The CLI starts it if needed, or runs a one-shot sync with `--no-daemon`.
- In a container, `famd` is the main process and `fam` runs with `docker exec`.

### Sync

The sync engine keeps a local copy of each mailbox and treats the server as the source of truth.

1. **First sync:** download the folder tree and headers for all history, then bodies for the sync window, newest first. Attachments download on demand.
2. **Incremental sync:** [delta queries](https://learn.microsoft.com/en-us/graph/api/message-delta) return only what changed in each folder, using a saved link per folder. They run across all history without bodies, so moves and read states stay correct for old mail too.
3. **Local writes:** an action updates the store at once and joins a durable queue, which retries with idempotency protection and survives restarts.
4. **Moves:** a move [appears as a removal from the old folder](https://learn.microsoft.com/en-us/graph/api/message-delta), so messages are matched by immutable ID and Internet message ID to keep local data and avoid downloading them again.
5. **Threads:** conversations are built locally from `conversationId` across all folders, which solves the split between Inbox and Sent Items.
6. **Calendars:** events sync by [delta query](https://unified.to/blog/how_to_integrate_with_microsoft_outlook.md) for a rolling window, and recurring series are shown as the server expands them.

### Recent and older mail

The device keeps one year of full messages, and older mail stays one click away. Keeping headers for everything goes slightly beyond your one-year idea, but at about 9,500 Hotmail messages it should take very little space and makes old mail instantly findable by sender, subject and date. Setting `headers = "window"` turns it off.

|  | Within the window | Older |
| --- | --- | --- |
| Stored locally | Headers and bodies | Headers and a short preview |
| Opening a message | Instant, also offline | Header shows at once and the body loads from the server, then stays in a size-limited cache |
| Search by sender, subject, date or folder | Instant, also offline | Instant, also offline |
| Search of message text | Instant, also offline | Through the server when online |
| Attachments | On demand | On demand |

Server search uses `$search` on messages, which worked on Hotmail on 4 November, including a test limited to mail before 2015. It [returns up to 1,000 results](https://learn.microsoft.com/pt-pt/graph/search-query-parameter) and can't be combined with `$filter` or `$orderby`, so results are refined locally. When offline, the client says that older message text wasn't searched.

Messages leave the window as they age, keeping their headers but losing their local bodies unless pinned. Changing the window to `all` would keep everything if you later prefer that.

### Polling and limits

The client polls because Graph webhooks need an [HTTPS endpoint Microsoft can call](https://learn.microsoft.com/graph/api/resources/subscription), which a desktop app doesn't have. Microsoft's Outlook limits are [10,000 requests per 10 minutes per app and mailbox](https://help.lawvu.com/en/articles/4213065-microsoft-outlook-throttling), [4 concurrent requests and 15 MB of uploads every 30 seconds](https://devblogs.microsoft.com/microsoft365dev/?p=4523), though [Microsoft says limits can change](https://learn.microsoft.com/en-ca/graph/throttling-limits).

Polling the Inbox every 30 seconds and all 46 Hotmail folders every 5 minutes uses about 112 requests per 10 minutes, or about 1% of that limit. The scheduler caps each mailbox at 4 concurrent requests, honours `Retry-After` and puts user actions ahead of background work.

### Code structure

The Rust workspace separates `core` (commands and types), `provider-graph`, `store`, `search`, `rules`, `daemon`, `cli` and `gui`. Only `gui` depends on Tauri, and CI builds the others on a machine with no display libraries to enforce that.

## Technology choices

The recommendation is a Rust core with a Tauri 2 desktop shell and a strict TypeScript interface. The deciding factor is that HTML email needs a web engine, so a webview is required in almost any design.

This matters more than raw drawing speed. GPU-native toolkits draw the app very fast but [have no HTML or CSS runtime](https://docs.rs/gpui-ce/0.3.0/gpui/_documentation/comparison/index.html), so they still need an embedded webview for the message body. Tauri already pairs a [web UI with a Rust backend](https://docs.rs/gpui-ce/0.3.0/gpui/_documentation/comparison/index.html), which uses one engine for the whole window.

### Options against your criteria

| Option | Mac, Windows, Linux | Headless in a container | Speed | Type safety | Polished UI | HTML email |
| --- | --- | --- | --- | --- | --- | --- |
| **Rust + Tauri 2 + TypeScript (recommended)** | Yes, but [each OS has a different webview](https://www.youngju.dev/blog/culture/2026-05-14-tauri-2-rust-webview-desktop-mobile-electron-alternative-deep-dive-2026.en) | Yes, the core has no UI dependency | Fast core; the UI depends on the webview, and Linux is weakest | Strong, if bindings are generated | Excellent, with the widest choice of UI tools | Built in |
| Rust + GPUI | Yes, [Zed 1.0 runs on all three](https://news.lavx.hu/article/rust-written-zed-1-0-code-editor-released-a-performance-analysis) | Yes | Best, [designed around 120 fps](https://zed.dev/blog/videogame) | Strongest, Rust throughout | Excellent, but most components must be built | Needs an embedded webview |
| Rust + Slint | Yes | Yes | Very good | Strong, [compiled `.slint` components](https://docs.rs/gpui-ce/0.3.0/gpui/_documentation/comparison/index.html) | Good, but less suited to dense custom UI | Needs an embedded webview |
| .NET + Avalonia | Yes, [rendered the same with Skia](https://startdebugging.net/de/2026/05/maui-vs-avalonia-vs-uno-in-2026/) | Yes, with Native AOT | Very good; [AOT improves startup, but some controls are incompatible](https://v11.docs.avaloniaui.net/docs/deployment/native-aot) | Strong, C# | Very good | [Embedded views always sit on top, so app controls can't overlap them](https://docs.avaloniaui.net/docs/app-development/native-interop) |
| TypeScript + Electron | Yes, [the same Chromium everywhere](https://www.youngju.dev/transcribe/culture/2026-05-14-tauri-2-rust-webview-desktop-mobile-electron-alternative-deep-dive-2026.en) | Node core only, without the UI | Weakest, heavy start and memory | Good, but weaker than Rust | Excellent | Built in |

Flutter and Qt were not researched for this document. Published comparisons report Tauri idling at [30 to 80 MB and starting in 0.2 to 0.8 seconds, against 200 to 400 MB and 1 to 3 seconds for Electron](https://www.youngju.dev/transcribe/culture/2026-05-14-tauri-2-rust-webview-desktop-mobile-electron-alternative-deep-dive-2026.en). These are secondary figures and not email-specific, so Phase 0 measures them directly.

### Why this combination

- **Precedent:** [Pebble](https://github.com/Tardis07/Pebble) and [RAVN](https://github.com/ravnmail/ravn) already combine Tauri, Rust, SQLite and Tantivy for local-first mail. None of those I checked advertises full CLI parity, and Pebble lists its Outlook support as experimental. Pebble's AGPL licence would apply to anything reused from it, so this project shouldn't copy its code under a permissive licence.
- **Headless:** the Rust core compiles into a binary with no webview or GUI library, so the CLI and daemon fit a small container image.
- **Type safety:** commands are defined in Rust and TypeScript types are generated from them, so a mismatch fails the build.
- **An exit if webviews disappoint:** Tauri 3 is in alpha with a [selectable Chromium (CEF) runtime](https://github.com/tauri-apps/tauri/releases/tag/@tauri-apps%2Fcli-v3.0.0-alpha.0), which would address differences between webviews at the cost of a bigger download. The UI is also a thin client, so GPUI could replace it later without changing the core.

Rust is confirmed, so Avalonia and Electron drop out. GPUI remains the main challenger, and Phase 0 tests it against Tauri. Phase 0 confirmed Tauri (Windows: 442 ms start, 11 ms keypress p95, 30 ms open p95). Because Claude is building this, Tauri also has a practical edge: its web interface is easier for Claude to run, screenshot and test automatically.

### Proposed stack

| Concern | Choice | Reason |
| --- | --- | --- |
| Core language | Rust on Tokio | No garbage-collection pauses, one binary, and strong types for commands and errors |
| Desktop shell | Tauri 2 | Small install, and web rendering for both the app and HTML mail |
| Interface | TypeScript in strict mode with SolidJS | Fine-grained updates suit long lists, and the framework adds little runtime weight |
| Metadata store | SQLite in WAL mode | Used by the comparable clients I checked, and readers don't block the sync writer |
| Search | Tantivy | Fast full-text search with field operators, and it can always be rebuilt |
| Message bodies | Compressed files keyed by content hash | Keeps the database small and removes duplicates |
| Mail service | Microsoft Graph | One API for [mail](https://learn.microsoft.com/en-us/graph/api/message-delta), calendars and rules, on both account types |
| CLI | Rust, generated from the command registry | Parity comes from the build rather than from discipline |
| Automation | Local MCP server and JSON-RPC over a socket or named pipe | Assistants and scripts reach mail without a cloud connector |
| Packaging | winget, Homebrew, deb/rpm/AppImage and an OCI image | One-command installs make reinstalling simple |

## Performance and interface

"Fast" is defined by the targets below and tested in CI, so a slowdown fails the build. These are proposed targets, not measurements. Hotmail holds about 9,500 messages, so Phase 0 tests with a synthetic mailbox of 500,000 to leave plenty of margin.

| Action | Target at the 95th percentile |
| --- | --- |
| Keypress to visible response | Under 16 ms |
| Cold start to usable inbox | Under 500 ms |
| Open a message that's already on the device | Under 50 ms |
| Search results, first page | Under 100 ms |
| Scrolling a list of 500,000 messages | 60 fps, with no dropped frames |
| `fam mail list --json` with the daemon running | Under 50 ms |
| Archive, move or flag, as seen in the UI | Immediate; the server call follows in the background |
| Idle memory: GUI / headless daemon | Under 400 MB private, including the webview / under 50 MB |
| Open an older message, online | Under 1 second |

The GUI memory target was raised from 150 MB after the Phase 0 bake-off: every design that can render HTML mail needs a web engine, and WebView2 alone uses most of that (see spikes/ui-bakeoff/RESULTS.md).

### How the speed is achieved

- The UI never waits for the network, because it reads from the local store. Older mail shows its header at once, and its body loads in place.
- Sorting and searching happen in Rust, and the interface receives only the rows on screen.
- Lists are virtualised, so row count doesn't affect scrolling.
- HTML for the next likely messages is prepared ahead of time.
- Sync runs at low priority and pauses briefly while you're typing or scrolling.

### Interface direction

The look is calm, dense and keyboard-first, closer to a modern developer tool than to the Office ribbon.

- **Layout:** three panes, with folders, message list and reading pane. The list can collapse, and the calendar is one keystroke away.
- **Command palette:** every registered command is reachable from one shortcut and shows its keybinding and CLI equivalent, which also teaches the CLI.
- **Keyboard:** single-key actions by default, all rebindable in the config.
- **Reading:** clean typography, with HTML mail shown in an isolated frame so it can't break the app's styling.
- **Calendar links:** a side panel shows that day's events while you create an event from an email, so clashes are visible straight away.
- **Appearance:** light, dark and system themes defined as design tokens, with a consistent custom title bar on each OS.
- **Accessibility:** full keyboard use, screen reader labels and WCAG 2.1 AA contrast from the first release.

## Security and sign-in

The client talks directly to Microsoft with no vendor server, so the main risks are local token storage, hostile email content and over-broad automation.

### Sign-in

Microsoft's [basic authentication is retired](https://cdn.jsdelivr.net/gh/pimalaya/himalaya@master/README.md), so every account uses OAuth 2.0 through a public client registration with no client secret.

| Context | Flow | Where the token lives |
| --- | --- | --- |
| Desktop GUI and CLI | Authorisation code with PKCE, through the system browser | OS keychain |
| Headless or over SSH | [Device code, entered on another device](https://github.com/JeremyTBradshaw/MSGraphPSEssentials/wiki/Get-an-access-token-via-Device-Code-flow) | OS keychain where one exists |
| Container | Device code on first run, or an imported token | Encrypted file on a mounted volume, with the key supplied as a container secret |

[Requesting `offline_access` is what produces a refresh token](https://www.unipile.com/microsoft-graph-oauth-email/), so sign-in survives between sessions.

The app registration must accept both [organisational and personal accounts](https://www.unipile.com/microsoft-graph-oauth-email/). The client ID is public and ships with the project, and anyone can replace it with their own in the config.

Hotmail needs no admin, but the two organisation accounts might. [Microsoft's recommended consent policy doesn't let users approve mail or calendar permissions themselves](https://learn.microsoft.com/en-us/entra/identity/enterprise-apps/manage-app-consent-policies), apart from a separate allowance for popular mail apps. You are an admin on the Parklands tenant, so you can approve the app there yourself. You aren't an admin on iM3 Dental, so that account only works if its tenant already lets users approve the app. Phase 0 tests this first, because an account that fails can't drop its current connector.

### Permissions

| Delegated permission | Needed for |
| --- | --- |
| `User.Read`, `offline_access` | Identity and staying signed in |
| `Mail.ReadWrite`, `Mail.Send` | Reading, organising and sending mail |
| `Calendars.ReadWrite` | Calendar views and events |
| `Contacts.Read` | Address autocomplete |
| [`MailboxSettings.ReadWrite`](https://learn.microsoft.com/en-us/graph/api/mailfolder-post-messagerules) | Managing server rules |

Permissions are requested per feature where possible, so an account used only for reading doesn't need send access.

### Email content

- HTML is sanitised in Rust and then shown in a sandboxed frame with scripts disabled and a strict content security policy.
- Remote images are blocked by default, with per-message and per-sender overrides, because they can act as read trackers.
- The real destination of a link is shown before it opens.
- Attachments never open automatically, and risky file types need an extra confirmation.

### Automation

Mail content is untrusted input, including when an assistant reads it through MCP. A message could contain instructions aimed at the assistant.

- Agent sessions, through the CLI or MCP, default to read and draft only, which matches how drafts are reviewed today.
- Send, delete, rule changes and forwarding need an explicit grant in the config, per account.
- The daemon enforces these limits and records every automated write in a local audit log with its source.
- The local API uses a per-user socket or named pipe and isn't exposed on the network unless configured.

These limits stop mistakes and injected instructions from casually sending mail, but they aren't a hard security boundary. An agent with an unrestricted shell under your user account could change the config or use your keychain. For stronger isolation, run the agent as a separate user or in a container that holds only read and draft permissions.

### Data on the device

The store relies on full-disk encryption and per-user file permissions. Database encryption at rest is a later option. There is no telemetry, and crash reports are opt-in with message content removed.

## Delivery, risks and open questions

Build the headless engine first, because it carries the riskiest work and gives you a useful CLI before any UI exists. Outlook stays in place until the parallel run passes.

&#91;embedded content: delivery roadmap · six phases, each with a gate; phases are not drawn to scale\]

Phases aren't dated. Claude builds under your direction, so the pace depends mainly on your reviews at each gate.

### How it gets built

- **Claude:** writes the Rust and TypeScript, tests, CI and documentation.
- **You:** set direction, approve each gate, register the Microsoft app and do every account sign-in.
- **Live testing:** the cloud workspace can't normally reach Microsoft, so tests against real accounts run on your computer. Everything else uses a mock Graph server and synthetic mail.
- **Safety:** real accounts stay read-only until Phase 2. Write tests then use a dedicated test folder and send only to you.

### Open source

- **Licence:** dual MIT or Apache-2.0, the usual choice for Rust projects, with `LICENSE-MIT` and `LICENSE-APACHE` in the repo.
- **Repo contents:** no real mail, tokens or personal config, with secret scanning in CI.
- **Distribution:** releases need Windows and macOS code signing, and Microsoft publisher verification may be worth doing for other users' tenants.

### Risks

| Risk | Effect | Response |
| --- | --- | --- |
| iM3 Dental [can't sign in without admin approval, which won't be given](https://learn.microsoft.com/en-us/entra/identity/enterprise-apps/manage-app-consent-policies) | That account can't use the client and keeps its current connector | Test in Phase 0 before building further. If blocked, the client still ships for the accounts that work, and that tenant stays on Outlook on the web |
| A tenant [blocks device code sign-in, as the CIS benchmark recommends](https://hub.powerpipe.io/mods/turbot/steampipe-mod-microsoft365-compliance/controls/control.cis_v600_5_2_2_12) | Headless login fails for that account | Sign in on a desktop and import the encrypted token |
| The CLI is only reachable where it's installed | Agents can't reach mail from a phone when your computer is off, which Composio can do today | Accepted: off-computer access isn't needed. A container on an always-on host remains possible later |
| Webview differences, especially [WebKitGTK on Linux](https://dev.to/devtoolpicks/tauri-vs-electron-for-indie-hackers-in-2026-honest-comparison-o0e) | Slow or inconsistent Linux UI | Measure in Phase 0, with Tauri's CEF runtime or GPUI as fallbacks |
| Calendar complexity, including recurrence, time zones and invites | Late, subtle bugs | Let the server expand recurrences and handle invites, and test against real calendars |
| Sync errors affecting real mail | Lost or misfiled messages | Read-only until Phase 2, destructive actions move to Deleted Items first, and a store-versus-server consistency check runs regularly |
| HTML email rendering varies widely | Broken messages or security flaws | Build a public test set from newsletters and synthetic mail, and keep sanitising and sandboxing separate |
| Overall scope, since a mail and calendar client is a large build | The project stalls before Outlook is replaced | Each phase is useful alone, starting with the headless CLI |

### Decisions and open questions

| Question | Decision |
| --- | --- |
| Current Outlook | New Outlook, so there's no classic add-in or VBA migration |
| Local history | One year locally, with older mail on demand |
| Other Outlook features | None needed, but per-account signatures must be saved in the config |
| SharePoint and OneDrive | Not needed |
| Admin approval for a custom app | You are an admin on Parklands and can approve it there; not available from iM3 Dental |
| Project model | Open source |
| Builder and language | Claude under your direction, in Rust |
| Connectors | The CLI replaces Composio and the Microsoft 365 connector for mail and calendar |
| Licence | Dual MIT or Apache-2.0 |
| Name | The project is Familiar, the CLI is `fam` and the daemon is `famd` |
| Mail access with your computer off | Not needed, so there's no always-on host |

Still open:

- [ ] Can iM3 Dental sign in without admin approval? Phase 0 answers this.

### Sources

Checked on 4 November 2026. Published performance figures are secondary sources, and several comparisons come from authors with a product interest.

- [Microsoft Graph throttling limits](https://learn.microsoft.com/en-ca/graph/throttling-limits), read in part, and [Outlook limits update](https://devblogs.microsoft.com/microsoft365dev/?p=4523)
- [Graph message delta](https://learn.microsoft.com/en-us/graph/api/message-delta), [create message rule](https://learn.microsoft.com/en-us/graph/api/mailfolder-post-messagerules), [subscriptions](https://learn.microsoft.com/graph/api/resources/subscription) and [`$search` parameter](https://learn.microsoft.com/pt-pt/graph/search-query-parameter)
- [Entra app consent policies](https://learn.microsoft.com/en-us/entra/identity/enterprise-apps/manage-app-consent-policies) and [Graph signature support answer](https://learn.microsoft.com/en-us/answers/a/1841062)
- [GPUI Community Edition toolkit comparison](https://docs.rs/gpui-ce/0.3.0/gpui/_documentation/comparison/index.html) and [Zed's GPUI rendering post](https://zed.dev/blog/videogame)
- [Tauri 3.0 alpha release notes](https://github.com/tauri-apps/tauri/releases/tag/@tauri-apps%2Fcli-v3.0.0-alpha.0) and [Tauri and Electron comparison](https://www.youngju.dev/transcribe/culture/2026-05-14-tauri-2-rust-webview-desktop-mobile-electron-alternative-deep-dive-2026.en)
- [Avalonia native interop](https://docs.avaloniaui.net/docs/app-development/native-interop) and [Native AOT](https://v11.docs.avaloniaui.net/docs/deployment/native-aot)
- [Pebble](https://github.com/Tardis07/Pebble) and [RAVN](https://github.com/ravnmail/ravn)
