# Agent permissions and the audit log

WP 2.6. Agents use the same `fam` commands as you. What they may change is set in `config.toml`, checked before anything changes, and every write is written to an audit log with who asked.

## Who counts as an agent

Each write carries a source:

| Source | When |
| --- | --- |
| `cli` | `fam` run by a person |
| `agent:<name>` | `fam` with `FAMILIAR_AGENT=<name>` set (`FAMILIAR_AGENT=1` gives `agent:agent`), or run inside Claude Code, which sets `CLAUDECODE=1` (`agent:claude-code`) |
| `mcp` | the MCP server (WP 2.7) |
| `gui` | the app (Phase 3) |

Writes from `agent:*` and `mcp` are checked. Writes from people aren't. `fam agent status` says which this session is and what agents may do on each account.

## Grants

```toml
[agent]
allow = ["read", "draft", "write"]   # every account

[agent.accounts.hotmail]
allow = ["send", "delete"]           # this account only
```

The default is `["read", "draft"]`: agents can read everything and save drafts for you to send. `write` covers marking read, flagging, moving, archiving, folders, and calendar events. `send`, `delete` and `rules` are only granted per account; validation rejects them in `agent.allow`.

What each op needs:

| Op | Needs |
| --- | --- |
| mark read, flag, move, archive, create/rename/move folder | `write` |
| delete a message (also into Deleted Items), delete a folder | `delete` |
| create or change a draft (new, reply, forward) | `draft` |
| send a draft (so forwarding needs `send` too) | `send` |
| create or change an event | `write`, plus `send` when it has attendees, or it is a meeting you organised that has attendees |
| answer an invitation | `write`, plus `send` when the organiser is told |
| delete an event | `delete`, plus `send` when it cancels a meeting you organised |

A write is checked whole: if any op it plans isn't granted, nothing changes. It fails with `permission_denied` (exit 5) and a message naming the account, the missing level and the config line that would grant it. `--dry-run` is checked the same way, so an agent can find out first.

famd reads `[agent]` again for each agent write, so a change to `config.toml` applies at once. Without famd, `fam` reads it itself.

## No prompts

`fam` never prompts. Anything that needs confirming (a permanent delete, cancelling a meeting with attendees) fails with `confirmation_required` (exit 10) until it's run again with `--yes`. `--yes` only confirms: it never grants what `[agent]` doesn't.

## The audit log

Every write is logged with its source: `queued` when it's accepted, then `done` or `failed` when the server answers. Writes refused for want of a grant are logged as `denied` (dry runs aren't). `fam audit list` reads it:

```
fam audit list --source 'agent:*' --since 7d
fam audit list --outcome denied --json
```

## Limits

These stop mistakes and instructions injected through mail content. They aren't a security boundary: an agent with a shell under your account can edit `config.toml` or unset `CLAUDECODE`. For real isolation, run the agent as another user or in a container that only has read and draft.
