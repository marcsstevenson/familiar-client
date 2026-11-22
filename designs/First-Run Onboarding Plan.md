# First-Run Onboarding Plan

19 Nov 2026 · Companion to the [Email Client Design Document](Email%20Client%20Design%20Document.md) and the [Development Plan](Development%20Plan.md)

## Summary

Today a new person who opens Familiar gets an empty window and an error. Nothing works until someone hand-writes `config.toml`, looks up a tenant ID, and runs `fam auth login` in a terminal. This plan replaces that with a short first run: sign in, then use the app straight away. Everything else is learned later, when it becomes useful.

The key decisions proposed are:

- **Sign in is the whole wizard.** The first screen offers a choice of two equal buttons: **Microsoft** (Outlook.com, Hotmail, work or school) and **Gmail**. Familiar signs in through the system browser and works out the email, display name and, for Microsoft, the tenant from the sign-in itself. Then it writes `config.toml` for the person. No forms come first.
- **Config stays the source of truth.** The GUI writes the same `config.toml` a person would write by hand, through the existing comment-preserving `ConfigDocument`. A restored config skips the wizard and goes to "Welcome back", which only asks for sign-ins.
- **Every step is a registry command.** The new commands are `fam init` and `fam account add`, so the CLI, agents and the GUI share one path. This is the project's parity rule applied to onboarding.
- **Land in the inbox in seconds, not after the sync.** famd already stores the newest Inbox page first. The app opens on the inbox while the first sync carries on, with progress shown per account in the status bar.
- **No tour.** Optional features (signatures, aliases, the Processed folder, spam check, hooks, agent access and MCP, calendar purposes, backing up the config) go in a "Get started" checklist the person can open, tick off or dismiss. Keyboard use is taught where the person already is: the palette, a `?` shortcut sheet, and a few one-time tips.
- **Empty states do the explaining.** Each empty pane says why it is empty and what to do, in one line with one action.

## Who first run is for

| Case | What they have | What they need |
| --- | --- | --- |
| **New person** | The app, a Microsoft account | Sign in and see their mail. They should never see TOML unless they want to |
| **Reinstall** (Marc, Phase 5.3 drill) | The app and a restored `config.toml` | A sign-in per account, then sync. No questions the config already answers |
| **Headless or agent user** | `fam` and `famd` in a terminal or container | `fam init`, `fam account add --device-code`, then `famd`. No GUI involved |

Marc's own install is already set up, so the first person this helps is Marc on a reinstall. After that it helps anyone he gives Familiar to, and the open-source release later.

## What happens today

These are the gaps found in the code (main at e54cf6d):

- **No config means no daemon.** famd exits with code 3 if `config.toml` is missing, can't be parsed, or has no `[accounts]` (`crates/daemon/src/main.rs`). The GUI's auto-start then has nothing to talk to.
- **The settings cog fails without a file.** `open_config` returns "there is no config file at …" instead of offering to create one (`crates/gui/src/main.rs`).
- **The list's empty state points at the terminal.** It says "no mail on this device yet: sign in with `fam auth login` and let famd sync" (`crates/gui/src/list.rs`).
- **The tenant must be typed by hand.** `tenant` is required. Working it out means knowing that Hotmail is `consumers` and a work account wants its tenant GUID, not `organizations`.
- **famd reads the config once.** It never reloads it, so adding an account means restarting famd.
- **Signatures and aliases are HTML and TOML.** Graph can't read either one (WP 0.3, and the 2026-11-06 alias check), so today they are written by hand.

The pieces onboarding needs already exist, though. `familiar-auth` signs in without the account being in the config (`--email`, `--authority`). It saves the `tid`, `preferred_username` and `name` claims. Refresh already goes to the account's own tenant. `ConfigDocument` writes the config back atomically and refuses to save an invalid result.

## What other apps do

| App | First run | What Familiar takes from it |
| --- | --- | --- |
| **Superhuman** | A 30-minute call with a person who sets up the inbox and drills shortcuts until they stick. Afterwards come tooltips for new shortcuts and one tip a day for the first month | Shortcuts are learned by doing, spread over days. Familiar can't run calls, so it copies the spacing with tips that appear once each |
| **Mimestream** | Sign in with Google and you're in. The only extra is a profile choice after the account is added | The smallest first run that works: one button, then mail |
| **Thunderbird** (Account Hub) | Ask for the email address, autoconfigure the server, then OAuth. Since version 153 it uses the system browser, not an embedded tab | The system browser is the right place for OAuth (Familiar already does this). Familiar doesn't even need the address first, because Microsoft's `common` sign-in reports who signed in |
| **HEY** | A short welcome, then one big concept (the Screener) shown at the moment it first matters: the first email from a new sender | Introduce a feature when it first applies. Example: offer the spam check the first time someone opens a Junk message |
| **Spark** | On first run, asks whether to turn on Smart Inbox and smart notifications. Both can be changed later in settings | Ask about opinionated features with one tap and a sensible default. Never bury them or force them on |
| **Linear** | Sign-in, workspace, theme, then the command menu, before any data. `?` opens a searchable shortcut sheet, right-click menus show shortcuts, and the welcome tutorial can be rerun from the command menu | Show Ctrl+K early, as the keyboard-first core. Add a searchable `?` sheet. Let "Get started" be reopened from the palette |
| **VS Code** | A Welcome page with walkthroughs: checklists that track progress and can be opened again from the palette. Whether they should pop up by themselves was argued over at length | A checklist the person drives, with progress taken from the real state (the config), and no automatic pop-ups |
| **Raycast** | A few screens that introduce the core features and set the global hotkey | Keep the screen count small and make each one change something real |

General guidance points the same way: people often skip tutorials, and contextual help and empty states teach better than a tour up front (NN/g on tutorials; progressive disclosure).

**Lessons for Familiar:** get to real mail as fast as possible. Ask only what can't be inferred. Teach features when they first apply, not on day one. Keep everything re-openable. Make the config the record of what was chosen.

## The proposed first run

### 0. Work out where the person is

When the app starts, it sorts the install into one of five states and opens the matching screen:

| State | How it's detected | Screen |
| --- | --- | --- |
| No config file | `paths::resolve` gives a path that doesn't exist | Welcome |
| Config is invalid | `familiar_config::check` reports errors | "Your config has problems": each error with line and column, plus **Open config** and **Check again**. The file is never overwritten |
| Config with no accounts | It loads but `accounts` is empty | Welcome, with the existing file kept |
| Accounts with no sign-in | `auth.status` shows missing or expired tokens | Welcome back (below) |
| Ready | At least one account is signed in | The normal app. Accounts that still need a sign-in get a banner |

The same check runs again when the window gets focus. A person who fixes the config in Notepad then sees the change without restarting.

### 1. Welcome

One screen, shown inside the main window rather than a separate one:

- The kea icon, "Familiar", and one line: "Fast, local-first mail and calendar. Your mail stays on this computer and goes straight to Microsoft, with no server in between."
- "Add your first account", with two buttons of equal weight side by side:
  - **Microsoft**: Outlook.com, Hotmail, Live, or a work or school account.
  - **Gmail**: a personal Google account (see Gmail below).
- Under them, one line helps anyone unsure which to pick: "Your address ends in @gmail.com? Choose Gmail. Hotmail, Outlook or a work address? Choose Microsoft." When the person has typed nothing, Familiar can't guess, so it doesn't preselect.
- Smaller links: **I already have a config.toml** (choose a file to copy into place, or show where Familiar looks), and **Set up from the terminal**, which shows `fam init` and `fam account add`.

There is no theme picker, no feature tour and no marketing. The theme follows the system and can be changed later.

### 2. Sign in

When **Microsoft** is pressed:

1. Familiar starts the existing PKCE flow in the system browser with authority `common` and `prompt=select_account`. That way one button works for both personal and work accounts, and anyone signed in to several accounts can pick one.
2. The window shows "Finish signing in in your browser…" with **Cancel** and **Use a code instead**. The second option is the device-code flow, which helps when the browser won't hand back to the loopback port.
3. On return, Familiar reads the ID token:
   - `preferred_username` becomes `email`.
   - `name` becomes `display_name`.
   - `tid` becomes `tenant`: `consumers` when it is the personal-account tenant `9188040d-6c67-4c5b-b112-36a304b66dad`, otherwise the tenant GUID. That is what refresh needs anyway, and it avoids the `organizations` warning.
4. It suggests an account name from the address: `hotmail`, `outlook` or `live` for personal domains, otherwise the first label of the domain (`parklandscricket`). A number is added if the name is taken. The name is shown in an editable field with one line of explanation ("used in searches, like `account:parklands`"), and **Continue** accepts it.
5. It writes the account to `config.toml`. If the file doesn't exist, it creates it from a short commented template (see Config and data folder). Then it saves the tokens under that name and starts or restarts famd.

Failures get a plain message and a next step:

| What went wrong | What Familiar says and offers |
| --- | --- |
| Admin approval needed (work tenant blocks user consent, as at iM3) | "Your organisation needs an administrator to approve Familiar." Offers **Copy a request for your IT admin** (text from `docs/im3-admin-consent-request.md` with the admin-consent link filled in) and **Use another account** |
| Browser closed or timed out | "Sign-in didn't finish." Offers **Try again** and **Use a code instead** |
| Offline | "Can't reach Microsoft. Check your connection." Offers **Try again** |
| Account already added | "That account is already in Familiar as `hotmail`." Offers **Sign in again** if its tokens had expired |
| Config write refused (it wouldn't validate) | Shows the validator's message. Nothing is saved and the tokens are dropped |

### 3. That's it: open the inbox

After the first account, the window goes straight to the unified inbox. It doesn't ask "add another account?" as a step of its own. Instead, a small **Add another account** link sits under the account list in the folder pane, and the same action is in the palette (`fam account add`). Signatures, aliases and the rest go to the Get started checklist rather than holding up the first run.

### Welcome back (restored config)

When the config has accounts but no tokens, the screen lists each account from the config ("hotmail · you@hotmail.com"), each with **Sign in**. Each sign-in uses that account's configured tenant and email hint, and the existing check refuses a sign-in as someone else. Once one account is signed in, **Open Familiar** appears. Accounts left unsigned stay listed on a banner. This is the reinstall drill (WP 5.3) with no terminal involved.

## First sync

famd already does the right thing: Inbox first, with one page of its newest headers stored before the full round, then the other folders, then a year of bodies, newest first. Onboarding only has to show it:

- **The inbox fills within seconds.** While the first sync runs, the empty list says "Downloading your mail. The newest messages appear first." It doesn't say "No messages".
- **Progress in the status bar,** per account, from `sync.status`, for example: "hotmail: downloading messages, Inbox done, 18 of 46 folders, about 4,100 of 9,500". The total is an estimate from each folder's `totalItemCount`, which the folder-tree sync already fetches. If the store doesn't keep that count yet, this work keeps it. After the first sync the count goes away. The existing sync popover keeps the detail.
- **Search says when it is incomplete.** During the first sync, search results carry "Still downloading: results may be incomplete. Try **Server too**."
- **Leaving is safe.** Closing the app mid-sync is fine: rounds resume from their saved `nextLink`. The status line says so the first time ("You can close Familiar. Sync picks up where it left off.").
- **Calendar.** The calendar's empty state says "Calendar is syncing" until the first calendar round finishes. Calendar rounds are fast: under 10 s on Hotmail.

## Get started checklist

Get started is a checklist panel, opened from a **Get started** item at the bottom of the folder pane and from the palette. The folder-pane item shows a count such as "3 of 8" until the panel is dismissed. Each item's tick comes from real state, so editing the config by hand ticks it too. Each item opens the thing it describes. None of them is required.

| Item | Done when | What it opens |
| --- | --- | --- |
| **Learn the keys** | The `?` sheet has been opened and Ctrl+K used once | The `?` shortcut sheet, with a note that Ctrl+K reaches every command |
| **Add your signature** | `[accounts.<name>.signature]` has `new` | The signature editor (below) |
| **Add the addresses you send from** | Optional. Ticked when aliases are set or the item is skipped | A list editor for `aliases`. It explains that Microsoft doesn't tell Familiar about aliases |
| **Set up a Processed folder** | `processed_folder` is set | A folder picker that writes the key and explains the tick |
| **Check mail for spam and scams** | `spam_check` is set on any account | Shown only when `claude` or `codex` is found. Explains that a message is sent to that tool, then turns it on per account |
| **Let AI agents use your mail** | `[agent]` has been looked at, or MCP set up | Explains the default (read and draft only), shows `claude mcp add familiar -- fam mcp serve` with a copy button, and links `docs/agent.md` |
| **Run a command on new mail** | `[[hooks]]` exists | Opens the config at a commented hooks example. This one is for people who script, so it stays plain |
| **Back up your config** | Can't be detected, so it is ticked by hand | Explains that `config.toml` plus a sign-in rebuilds the install, and that the file belongs in a private repo because it holds signatures |

The checklist's own state (dismissed, manual ticks, tips shown) is UI state, not a setting. It lives in the data folder next to `folders.json` as `onboarding.json`, so the "config is text" principle isn't muddied with "has seen tip 3".

### Signature editor

Signatures are the one optional step most people will want. The editor reuses the compose window's editor in a small dialog, with tabs for **New messages** and **Replies**, a **Same for both** box, and a hint: "In Outlook, open a new email, select your signature, copy it, and paste it here." Pasted HTML goes through the same sanitiser as compose. Small images are embedded, as the design doc says. It saves with `ConfigDocument::set_signature`. This overlaps WP 4.6's "guided one-time signature copy", so it can carry that part of 4.6.

## Teaching the keyboard

Familiar is keyboard-first, but first-run people come from Outlook and use the mouse. Superhuman and Linear both teach in place, without a lesson:

- **`?` opens a searchable shortcut sheet,** grouped by area, showing the current bindings from `[keys]`. It is also in the palette and in the Get started checklist.
- **Ctrl+K appears in the first empty states** ("Press Ctrl+K for every command"). The palette already shows each command's key and its `fam` line, which is what teaches the CLI.
- **One-time tips.** The first three times someone does with the mouse something that has a single key, a quiet notice says "Tip: press **e** to archive." That covers archive, delete, reply and next/previous message. Each tip shows at most three times, and a "Don't show tips" link turns them all off. Tips are never shown while compose has focus.
- **Context menus show keys** next to each item where one exists, as Linear does (some may already; this is a check, not a feature).

## Empty states

Each one gets one line of why, one of what to do, and at most one button. No illustration is needed beyond the existing kea in the Welcome screen.

| Where | Today | Proposed |
| --- | --- | --- |
| Whole app, no accounts | Error notice | The Welcome screen |
| Message list during first sync | "no mail on this device yet: sign in with `fam auth login`…" | "Downloading your mail. The newest messages appear first." |
| Folder with no messages | Blank | "Nothing in Archive." For Inbox, after the first sync: "Inbox is empty." |
| Search with no results | Blank | "No messages match `…` on this computer." Offers **Search the server too** (the existing Server too option) |
| Reading pane, nothing selected | Blank | "Select a message. **j** and **k** move through the list, Ctrl+K does everything else." |
| Calendar, no events in range | Blank | "Nothing on." While syncing: "Calendar is syncing." |
| Sync status, no accounts | "No accounts have synced on this device." | "No accounts yet." Offers **Add an account** |
| famd not running or crashed | Daemon state notice | "Familiar's background sync isn't running." Offers **Start it**, plus the last lines of `famd.log` if it exited with an error |
| Account needs sign-in again | Status bar "Sign-in problem" | Banner on that account: "hotmail needs you to sign in again." Offers **Sign in** |
| Compose with no identities | `Config: …` warning | "Add an account before writing mail." Offers **Add an account** |

All copy goes through one pass with the project's UX-copy style (plain, sentence case, no blame) when it is built.

## Config and data folder

- **Created by the first account, not before.** Writing a config with no accounts would just give famd an error to report. The data folder (`%USERPROFILE%\.familiar` on Windows) and the config are created together when the first account is added.
- **A short template.** The new file has `version = 1`, the account, and commented-out examples of `[sync]`, `[keys]`, `[agent]` and `[[hooks]]`, each with one line pointing to `docs/config.md`. It does not include `[app] client_id`. That key already defaults to Familiar's own ID, and leaving it out means a future ID change reaches new installs. The template lives in `crates/config` beside `config.example.toml`, and a test runs it through `check()`.
- **Never clobber.** If a config exists, Familiar only adds to it with `ConfigDocument`, which keeps comments and refuses invalid results. An invalid existing file is shown with its errors, never replaced.
- **The settings cog without a file** offers **Create config.toml** instead of an error.
- **famd and config changes.** famd should start, and stay up, with no accounts: it logs "no accounts yet" and idles instead of exiting 3. After the GUI or `fam account add` changes the accounts, famd is restarted the same way the GUI auto-starts it today. Queued ops are durable and rounds resume, so a restart costs a few seconds. Live config reload (a file watch or a socket call) is left until a restart is shown to be a real problem.

## The CLI path

Two new registry commands, so the GUI, `fam`, MCP and the palette all get them:

- **`fam init`** creates the data folder and the template config if neither exists, prints the path, and suggests `fam account add` next. If a config exists it says so and changes nothing. It is idempotent.
- **`fam account add [--provider microsoft|gmail] [--name <n>] [--device-code] [--email <hint>]`** (provider defaults to `microsoft`; `--device-code` is Microsoft only, because Google's device flow doesn't allow Gmail scopes) runs the sign-in described above, writes the account to the config, saves the tokens, and asks a running famd to restart. With `--json` it returns the account it added. An agent session can't run it, because it needs a person in a browser, so agent sources get a clear refusal.
- **`fam account remove <name>`** is a later item. It signs out and removes the table from the config. Deleting that account's local mail is a separate, confirmed step, because it is destructive.

`docs/sign-in.md` and `docs/config.md` change their first steps to `fam init`, then `fam account add`. Hand-writing the config stays documented for people who prefer it.

## Gmail

Gmail sits beside Microsoft on the Welcome screen, and in **Add another account**, so the person chooses. Gmail accounts already sync (Phase G: `provider = "gmail"`, Google client from `google-client.json` beside the config, or `[app.google] client_file`).

When **Gmail** is pressed:

1. Familiar finds a Google client (see the choice below). Then it opens Google's sign-in in the system browser: PKCE, a loopback redirect, `prompt=select_account consent`, and every Gmail, Calendar and People scope at once, as `familiar-auth` already does.
2. The waiting screen warns about the one surprise first: "Google will say it hasn't verified Familiar. Choose **Advanced**, then **Go to Familiar**. Familiar is open source and talks only to Google, from this computer." Without that line people stop at Google's warning page.
3. On return, the ID token's `email` and `name` become `email` and `display_name`. The account is written with `provider = "gmail"` and no tenant. The suggested name is `gmail` (`gmail2` and so on if it's taken), or the first label of the domain for a Google Workspace address. Send-as aliases come from Gmail itself, so the alias checklist item is ticked already for Gmail accounts.
4. Then famd restarts and the inbox opens, as for Microsoft.

Gmail's failures:

| What went wrong | What Familiar says and offers |
| --- | --- |
| Person stopped at "Google hasn't verified this app" | "Sign-in didn't finish." Offers **Try again**, with the Advanced, then Go to Familiar steps shown again |
| Some permission boxes left unticked on Google's consent page | "Familiar needs every box ticked to read and send your mail." Offers **Try again** (Google shows the boxes again because of `prompt=consent`) |
| Workspace admin blocks the app | "Your organisation's Google admin needs to allow Familiar." Offers **Copy a request for your admin**, with the client ID to allow |
| No Google client available | The bring-your-own page: what's needed, a link to `docs/gmail-setup.md`, and **Choose google-client.json**, which copies the file beside the config |

**Whose Google client.** Microsoft needs no setup because Familiar ships its client ID. Gmail can't offer a real choice unless it does the same. The Gmail Plan recommended against shipping one, but that was before new users were in scope. Now there are two ways:

- **Ship Familiar's own Google client** (Marc's unverified production project) built in, with `google-client.json` or `client_file` as an override. Gmail then needs no setup. The costs: the unverified warning on every sign-in, a cap of 100 users over the project's lifetime, and new users see Marc's project name on the consent screen. Google treats a desktop app's client secret as not secret, so shipping it is allowed. Verification with a CASA assessment lifts the cap and the warning if Familiar ever grows past that.
- **Bring your own project** for everyone except Marc. Every new Gmail user spends ten minutes in Google Cloud console first, so Gmail is a real choice only for technical people.

This plan recommends shipping the client for now, since the audience is a small group of new users, and counting sign-ins against the cap in the release notes. The bring-your-own page stays for anyone who wants their own.

## Testing

- **Mock identity:** the mock already follows Entra's tenant rules. It needs a `common` authority that returns the consumer `tid` for personal mock users and the tenant GUID for work ones, and an "admin approval required" error for a tenant marked as blocking consent.
- **Unit and CLI tests:** `fam init` on an empty home, `fam account add` against the mock for a personal and a work user (checking `tenant`, the suggested name and the config's validity), refusal under an agent source, no overwrite of an invalid config, and famd starting with no accounts.
- **GUI:** the onboarding screens are driven through the existing Playwright and IPC-stub harness, or WebView2 remote debugging on Windows. Per Marc's rule, tests never drive his mouse or keyboard.
- **Live drill on Marc's PC:** run the app under a fresh `FAMILIAR_DATA_DIR` and `FAMILIAR_CONFIG` with a separate token store (`FAMILIAR_TOKEN_STORE=file`), so his real install isn't touched. Add Hotmail and Parklands through the Welcome screen and confirm the written config matches his hand-made one, apart from comments. This is also a rehearsal of WP 5.3.

## Work packages

| WP | Work | Depends on | Route | Done when |
| --- | --- | --- | --- | --- |
| OB.1 | **famd with no accounts, restart on account change.** famd idles instead of exiting; GUI and CLI restart it after an account change | — | PR (daemon) | Fresh install: GUI starts famd with no config, adds an account, sync begins with no manual step |
| OB.2 | **`fam init` and `fam account add`.** Template config; `--provider microsoft\|gmail`; Microsoft `common` sign-in with `tid` mapped to `tenant`; Gmail sign-in writing `provider = "gmail"`; the built-in Google client if Marc agrees; name suggestion; config write; mock `common` support | OB.1 | PR (auth and config writes) | Mock Microsoft and Gmail users get valid configs; live Hotmail and Gmail adds match Marc's config |
| OB.3 | **Welcome, sign-in and Welcome back screens.** State detection; Microsoft and Gmail side by side; both error tables; bring-your-own Google client page; settings cog creates the config | OB.2 | PR (touches sign-in) | A new Windows profile goes from first launch to inbox with no terminal, for a Microsoft account and for a Gmail one |
| OB.4 | **First-sync progress.** Folder totals kept; status bar estimate; first-sync copy in list, search and calendar | OB.3 | Direct to main if no schema change, otherwise PR | Progress counts move during a mock first sync and disappear after it |
| OB.5 | **Empty states.** Every row in the empty-states table | OB.3 | Direct to main (UI copy) | Each state checked in the browser harness |
| OB.6 | **Get started checklist and keyboard teaching.** Panel, `onboarding.json`, `?` sheet, one-time tips, context-menu keys | OB.3 | Direct to main (UI) | Items tick from config edits; tips stop after three showings and when turned off |
| OB.7 | **Signature and alias editors.** Dialog over `ConfigDocument`; carries the signature part of WP 4.6 | OB.6 | Direct to main (writes only config through the existing validated path) | A pasted Outlook signature round-trips to config and appears in compose |
| OB.8 | **Fresh-profile drill on Marc's PC** | OB.1–OB.7 | — | Marc's two accounts set up from nothing through the GUI in under five minutes of his time |

OB.1 to OB.3 are the core. They are worth doing before the open-source release (Phase 5) and right before the WP 5.3 reinstall drill. OB.4 to OB.7 can run in parallel threads once OB.3 is in.

## Decisions

Answered by Marc on 23 Nov 2026:

1. **Audience:** build for new people who want to use Familiar now, not only Marc's reinstall. OB.1 to OB.7 get built, starting with the core (OB.1 to OB.3).
2. **Client ID:** shipping Familiar's own Microsoft client ID to other people is fine until publisher verification (WP 5.2). Work tenants that block consent get the "ask your IT admin" path.
3. **famd restart:** restarting famd after an account change is accepted. No live config reload.
4. **Signatures:** they stay in the Get started checklist, not in the first run.
5. **Keyboard tips:** on by default, with a "Don't show tips" switch.
6. **Gmail in onboarding** (Marc, 23 Nov): the first run offers Gmail beside Microsoft so the person chooses. See Gmail above.

Still open: whether Familiar ships Marc's Google client so Gmail needs no setup (recommended above) or Gmail users bring their own project, and whether importing server rules and folder favourites (the rest of WP 4.6) also becomes Get started items once WP 4.3 is done.

## Sources

- Superhuman, [The fastest way to Inbox Zero? A single coaching session](https://blog.superhuman.com/the-fastest-way-to-inbox-zero-a-single-coaching-session/), and [5 lessons from Superhuman's onboarding](https://www.gainsight.com/blog/5-lessons-i-learned-from-superhumans-onboarding/)
- Mimestream, [Get started with Mimestream](https://mimestream.com/help/getting-started)
- Mozilla, [Account Hub for Thunderbird Desktop](https://support.mozilla.org/it/kb/account-hub-thunderbird-desktop) and [Thunderbird autoconfiguration](https://wiki.mozilla.org/Thunderbird:Autoconfiguration)
- ProductLed, [How HEY onboards new users](https://productled.com/hey-onboarding-teardown/); Spark, [Customize your Smart Inbox](https://support.readdle.com/spark/personalization/customize-your-smart-inbox)
- Linear, [First time user experience (2019)](https://linear.app/changelog/2019-05-09-first-time-user-experience), [Keyboard shortcuts help (2021)](https://linear.app/changelog/2021-03-25-keyboard-shortcuts-help), [Navigating Linear](https://linear.app/enablement/guide/navigating-linear), and Supademo's [Linear onboarding teardown](https://supademo.com/user-flow-examples/linear)
- Visual Studio Code, [Get started](https://code.visualstudio.com/docs/getstarted/overview?os=macos); Raycast, [Quick intro](https://manual.raycast.com/windows/getting-started)
- Nielsen Norman Group, [Examples of tutorials in apps](https://www.nngroup.com/articles/onboarding-tutorials/)
- Microsoft, [ID token claims reference](https://learn.microsoft.com/hr-HR/azure/active-directory/develop/id-token-claims-reference) (the `tid` for personal accounts is `9188040d-6c67-4c5b-b112-36a304b66dad`)
