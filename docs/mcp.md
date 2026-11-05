# MCP server and local JSON-RPC API

WP 2.7. `fam mcp serve` offers every `fam` command as an MCP tool over stdio, for agents and tools that can't run a shell. It is generated from the same registry as the CLI, `fam schema --json` and the TypeScript bindings, so the parity test fails if a command is missing from any of them.

## Using it from Claude Code

```
claude mcp add familiar -- fam mcp serve --allow read,draft
```

Or in a project's `.mcp.json`:

```json
{ "mcpServers": { "familiar": { "command": "fam", "args": ["mcp", "serve", "--allow", "read,draft"] } } }
```

Tools are named after the command path with `_`: `mail_list`, `mail_thread`, `draft_create`, `cal_add`, `contacts_search`. Each tool's input schema is the command's input (the same names as the CLI flags, with `-` as `_`), and its result carries the command's `--json` output both as `structuredContent` and as JSON text. A failed command is a tool result with `isError: true` and the text `{"error": {"kind", "message", "exit_code"}}`, using the kinds and codes in [exit-codes.md](exit-codes.md).

## What it offers

- `--allow` takes access levels, comma-separated or repeated: `read`, `draft`, `write`, `send`, `delete`, `rules`. Only commands at those levels are listed and callable. The default is `read,draft`; `read` is always included.
- `auth login`, `auth logout` and `mcp serve` are never offered.
- Writes are made as source `mcp`, so famd also checks each one against `[agent]` in `config.toml` (see [agent.md](agent.md)) and logs it in the audit log. Offering `send` here doesn't grant it: the account needs `[agent.accounts.<name>] allow = ["send"]` too.

## The local JSON-RPC API

The same server answers each command by its dotted name as a plain JSON-RPC 2.0 method, one message per line:

```
{"jsonrpc":"2.0","id":1,"method":"mail.list","params":{"limit":5}}
{"jsonrpc":"2.0","id":1,"result":{"folders":[...],"messages":[...]}}
```

`params` is the command's input and `result` its `--json` output. A command error is a JSON-RPC error with code `-32000`, the message, and `data: {"kind", "message", "exit_code"}`. A command outside `--allow` is code `-32001` with kind `permission_denied`; an unknown method is `-32601`.

MCP's own methods are `initialize`, `ping`, `tools/list` and `tools/call` (protocol `2025-06-18`; `2025-03-26` and `2024-11-05` are answered in kind). Notifications get no answer. The server runs until stdin closes and prints nothing else to stdout; progress lines go to stderr.

## Tests

`crates/cli/tests/mcp.rs` runs the real `fam mcp serve` against famd and the mock: the tool list follows `--allow`, a draft is saved and logged as `mcp`, a write famd's `[agent]` doesn't grant comes back as `permission_denied` (exit code 5) and is logged as `denied`, and the dotted methods work.
