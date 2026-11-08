# Finding events in emails with an AI tool

AI events plan, WP AI.2. `fam cal propose --from-mail <id>` (and the app's
Add to calendar) can ask an AI command-line tool you already have, Claude
Code (`claude`) or OpenAI Codex (`codex`), for the calendar events in an
email. The answer is only a proposal: nothing is added until you confirm
it. See [calendar.md](calendar.md#events-an-email-suggests) for the rest of
`fam cal propose`.

## Turning it on

It is off until you add this to `config.toml` (`fam config path` shows
where):

```toml
[ai]
enabled = true
tool = "auto"            # "auto" (Claude, else Codex), "claude", "codex", "none", or a path
# model = "sonnet"       # passed as --model
# timeout_seconds = 90
# accounts = ["hotmail"] # only these accounts' mail; default every account
```

The tool uses its own sign-in (a Claude or ChatGPT subscription, or an API
key). Familiar never sees it.

## When it is asked

Only when the email is not a meeting request and has no `.ics` attachment
with events: those are read by Familiar itself. It is never asked when an
agent or the MCP server runs `fam cal propose` (an agent asking another
agent), or with `--no-ai`. If the tool fails, times out or answers in a way
that can't be read, you get Familiar's own guess with a note saying why.

## What is sent

The subject, sender, date received and text of that one email, the text of
its PDF attachments (up to 10 MB each; scanned PDFs have no text), and the
names of other attachments. At most 20,000 characters of text. No other
mail and no calendar data.

## How it is run

From an empty temporary folder, with the prompt on stdin and a JSON schema
for the answer:

```
claude -p --output-format json --json-schema <schema> --tools "" --strict-mcp-config \
       --setting-sources "" --no-session-persistence [--model <model>]

codex exec - --sandbox read-only --skip-git-repo-check \
       --output-schema <file> --output-last-message <file> [--model <model>]
```

- Claude gets no tools and no MCP servers, and ignores your `CLAUDE.md`,
  hooks and settings for the run, so it can only answer.
- **Codex has no way to turn its tools off.** Read-only mode still lets it
  read files and run commands as you, so an email written to trick it could
  get it to read other files on this computer and send them to OpenAI. Use
  Claude unless you accept that.
- `FAMILIAR_AGENT=ai-propose` is set for the tool: if it ever ran `fam`, it
  would be an agent under `[agent]` grants, and in the audit log.

Each event the tool gives is checked: a start Familiar can read, an end
after the start (else the end is dropped), a known time zone (else this
computer's), and a repeat `fam cal add` can set. Events that fail are left
out with a note. The tool's `comment` is shown as a note.

## Checking it by hand

`cargo test -p familiar-core --lib ai::tests::live -- --ignored --nocapture`
asks the real tool on PATH (`FAMILIAR_AI_TOOL=codex` for Codex) with a
sample email and prints its answer. Claude answered it in about 8 s on
8 Nov 2026; Codex is still to be checked (WP AI.4).
