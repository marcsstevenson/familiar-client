# Command registry

Every user action in Familiar is a command defined once in `familiar-core`. The CLI, `fam schema --json`, the MCP tool list and the desktop app's TypeScript bindings are all generated from that definition, so no surface can do something the others can't.

## Adding a command

1. Write the command in `crates/core/src/commands/`:

   ```rust
   pub struct MailList;

   #[derive(Deserialize, JsonSchema)]
   #[serde(deny_unknown_fields)]
   pub struct MailListInput {
       /// Account name from config.toml.
       pub account: Option<String>,
       /// Only unread messages.
       #[serde(default)]
       pub unread: bool,
   }

   impl Command for MailList {
       type Input = MailListInput;
       type Output = Vec<MessageSummary>;
       const SPEC: CommandSpec = CommandSpec {
           path: &["mail", "list"],          // fam mail list, MCP tool mail_list
           summary: "List messages",
           description: "",
           access: Access::Read,
           errors: &[ErrorKind::NotFound, ErrorKind::DaemonUnavailable],
       };
       fn run(&self, ctx: &Context<'_>, input: MailListInput) -> Result<Self::Output, CommandError> { … }
   }
   ```

2. Register it in `Registry::builtin()` (`crates/core/src/registry.rs`).
3. Regenerate the checked-in files: `FAMILIAR_BLESS=1 cargo test -p familiar-cli --test parity`.
4. Commit the updated `bindings/commands.ts`.

Doc comments on input fields become the CLI help, the MCP argument descriptions and the TS comments.

## Input fields and CLI flags

| Input field | CLI |
| --- | --- |
| `bool` (defaults to false) | `--flag` |
| string, integer, number | `--name <value>` |
| unit-variant enum | `--name <a\|b\|c>` |
| `Vec<_>` of the above | `--name <value>`, repeatable |
| `Option<_>` or `#[serde(default)]` | optional; otherwise required |
| `#[schemars(extend("x-positional" = 0))]` | positional argument 0 |

Field names are snake_case in JSON and kebab-case on the command line. Nested structs are not allowed in inputs; the CLI build fails if a command's input can't be expressed as flags. `--json` is global.

## Output and errors

- `--json` prints the output as JSON matching the command's output schema in `fam schema --json`. Without it the command's `render_text` is printed.
- Errors exit with the stable codes in [exit-codes.md](exit-codes.md). With `--json` the error is printed to stderr as `{"error": {"kind", "message", "exit_code"}}`.

## The parity test

`crates/cli/tests/parity.rs` reads each surface as shipped (the CLI tree, the `fam` binary's schema and MCP output, the checked-in TS file) and fails if any registered command is missing from one, if a surface lists a command that isn't registered, or if a generated file is stale.
