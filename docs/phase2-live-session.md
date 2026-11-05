# Phase 2 live session (Marc's PC)

One sitting that covers the live checks saved from WP 2.2 to 2.7 and the WP 2.9 gate tasks. A Claude Code session on the PC can run all of it; the steps marked **Marc** need you to look at Outlook or approve something.

Rules: write tests use the `Familiar Test` folder, and mail is sent only to marcsstevenson@hotmail.com.

## 1. Set up

1. Pull `main` and build: `cargo build --release -p familiar-cli -p familiar-daemon`.
2. Stop the old famd (`fam sync status` shows it), start the new `famd`, and wait for `fam sync status --json` to show both accounts idle. The store migrates to schema v6 (contacts) on start.
3. Edit `config.toml` (Claude Code runs `fam` as an agent, so it only gets what this grants):

   ```toml
   [agent]
   allow = ["read", "draft", "write"]

   [agent.accounts.hotmail]
   allow = ["send"]   # for step 2.3 only; remove it afterwards
   ```

   `fam config validate` should pass, and `fam agent status --json` should show `agent: true` with those grants.
4. Copy `skills/familiar/` to `~/.claude/skills/familiar/`.

## 2. Live checks

**2.2 mail and folders (Hotmail):**
- `fam folder create "Familiar Test" --account hotmail --wait --json`
- Pick an old Inbox message ID from `fam mail list --account hotmail --limit 5 --json`, then `fam mail move <id> --to "Familiar Test" --wait`, `fam mail flag <id> --wait`, `fam mail read <id> --unread --wait`.
- **Marc:** check Outlook on the web shows the message in Familiar Test, flagged and unread.
- Move it back: `fam mail move <id> --to inbox --wait`, `fam mail flag <id> --clear --wait`.

**2.3 drafts and send:**
- `fam draft create --account hotmail --to marcsstevenson@hotmail.com --subject "Familiar test 1" --body "Sent by fam." --json`, then `fam draft send <id> --wait`.
- `fam draft reply <id of that message in Inbox> --body "Reply from fam." --json` (leave it as a draft).
- **Marc:** the mail arrived with the right signature; the reply draft is in Drafts, threaded.
- Remove the `send` grant from `config.toml`.

**2.4 calendar:**
- Find a real email with an event in it (club fixture, school notice): `fam mail search '...' --json`.
- `fam cal add --from-mail <id> --purpose family --dry-run --json`, check the suggestion, then run it without `--dry-run` and with `--wait`.
- `fam cal edit <event id> --location "Familiar test" --wait`.
- **Marc:** the event appeared in Family Room in Outlook with a link to the email, and the edit followed. If it was only a test, delete it yourself with `fam cal delete <event id> --wait` in your own terminal (agents don't have `delete`), and check it goes from Outlook.

**2.5 contacts:** `fam contacts search <a name you know> --json` finds Outlook contacts and people you've emailed.

**2.6 permissions:** `fam mail delete <id> --dry-run --json` fails with `permission_denied` (exit 5); `fam audit list --source 'agent:*' --json` shows the session's writes and that refusal.

**2.7 MCP:** `claude mcp add familiar -- fam mcp serve --allow read,draft`, then in a new Claude Code session ask for the latest club email through the `mail_search` tool.

## 3. WP 2.9 gate tasks

In a fresh Claude Code session with only the skill (no extra instructions), ask:

1. "Find the thread about <topic> and summarise it."
2. "Save a draft reply to <person> saying <...>." (draft only)
3. "Create a Family Room event from the email about <...>."

**Marc:** confirm each one. Then remove `"write"` from `[agent] allow` if you'd rather agents stay read and draft day to day.

## 4. Record

Note timings and anything odd; the results go in the Development Plan's WP 2.9 row.
