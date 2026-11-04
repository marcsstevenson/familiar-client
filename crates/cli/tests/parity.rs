// SPDX-License-Identifier: MIT OR Apache-2.0
//! The parity test (WP 1.2): every registered command must appear on every
//! surface. CI runs this; a command missing from the CLI, `fam schema --json`,
//! the MCP tool list or the checked-in TypeScript bindings fails the build.
//!
//! The checked-in generated files (`bindings/commands.ts`, `docs/exit-codes.md`)
//! are regenerated with `FAMILIAR_BLESS=1 cargo test -p familiar-cli --test parity`.

use familiar_cli::{build_cli, cli_command_names};
use familiar_core::parity::{self, Mismatch, Surface};
use familiar_core::registry::{Access, Command, CommandSpec, Context};
use familiar_core::{CommandError, Registry, surfaces};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command as Process;

fn repo_file(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn fam(args: &[&str]) -> (i32, String, String) {
    let out = Process::new(env!("CARGO_BIN_EXE_fam"))
        .args(args)
        .output()
        .expect("run fam");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn fam_json(args: &[&str]) -> Value {
    let (code, stdout, stderr) = fam(args);
    assert_eq!(code, 0, "fam {args:?} failed: {stderr}");
    serde_json::from_str(&stdout).expect("fam printed JSON")
}

/// Command names listed in a TypeScript bindings file's `COMMAND_NAMES`.
fn ts_command_names(source: &str) -> BTreeSet<String> {
    let start = source
        .find("export const COMMAND_NAMES = [")
        .expect("COMMAND_NAMES in bindings");
    let rest = &source[start..];
    let end = rest.find("]").expect("end of COMMAND_NAMES");
    rest[..end]
        .lines()
        .skip(1)
        .filter_map(|l| {
            l.trim()
                .strip_suffix(',')
                .map(|l| l.trim_matches('"').to_string())
        })
        .collect()
}

/// Maps MCP tool names back to dotted command names; unknown tools keep their own name.
fn mcp_names(registry: &Registry, tools: &Value) -> BTreeSet<String> {
    let by_tool: BTreeMap<String, String> = registry
        .commands()
        .map(|c| (c.spec().mcp_name(), c.spec().name()))
        .collect();
    tools["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| {
            let n = t["name"].as_str().expect("tool name");
            by_tool.get(n).cloned().unwrap_or_else(|| n.to_string())
        })
        .collect()
}

/// Reads each surface as it is actually shipped: the CLI tree, the `fam`
/// binary's schema and MCP output, and the checked-in TS file.
fn shipped_surfaces(registry: &Registry) -> Vec<(Surface, BTreeSet<String>)> {
    let cli = cli_command_names(&build_cli(registry).expect("CLI builds"));
    let schema: BTreeSet<String> = fam_json(&["schema", "--json"])["commands"]
        .as_array()
        .expect("commands array")
        .iter()
        .map(|c| c["name"].as_str().expect("name").to_string())
        .collect();
    let mcp = mcp_names(
        registry,
        &fam_json(&["schema", "--format", "mcp", "--json"]),
    );
    let ts = ts_command_names(&read(&repo_file("bindings/commands.ts")));
    vec![
        (Surface::Cli, cli),
        (Surface::JsonSchema, schema),
        (Surface::Mcp, mcp),
        (Surface::TypeScript, ts),
    ]
}

fn read(path: &PathBuf) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
        .replace("\r\n", "\n")
}

fn report(problems: &[Mismatch]) -> String {
    problems.iter().map(|p| format!("  - {p}\n")).collect()
}

#[test]
fn every_command_is_on_every_surface() {
    let registry = Registry::builtin();
    if let Err(problems) = parity::check(&registry, &shipped_surfaces(&registry)) {
        panic!(
            "command surfaces are out of step with the registry:\n{}\
             If you added a command, regenerate with FAMILIAR_BLESS=1 cargo test -p familiar-cli --test parity",
            report(&problems)
        );
    }
}

/// Generated files in the repo must match the generator exactly.
#[test]
fn generated_files_are_current() {
    let registry = Registry::builtin();
    let files = [
        (
            "bindings/commands.ts",
            surfaces::typescript::bindings(&registry),
        ),
        (
            "docs/exit-codes.md",
            familiar_core::error::exit_codes_markdown(),
        ),
    ];
    let bless = std::env::var_os("FAMILIAR_BLESS").is_some();
    for (rel, want) in files {
        let path = repo_file(rel);
        if bless {
            std::fs::write(&path, &want).unwrap();
            continue;
        }
        let have = read(&path);
        assert!(
            have == want,
            "{rel} is stale. Regenerate with FAMILIAR_BLESS=1 cargo test -p familiar-cli --test parity"
        );
    }
}

/// The sample command works on the CLI, in the schema and as an MCP tool.
#[test]
fn sample_command_on_every_surface() {
    let out = fam_json(&["version", "--json"]);
    assert_eq!(out["version"], env!("CARGO_PKG_VERSION"));

    let doc = fam_json(&["schema", "--json"]);
    let version = doc["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "version")
        .unwrap();
    assert_eq!(version["cli"], "fam version");
    assert_eq!(version["input"]["properties"]["verbose"]["type"], "boolean");
    assert!(
        doc["exit_codes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["code"] == 2 && c["kind"] == "usage")
    );

    let mcp = fam_json(&["schema", "--format", "mcp", "--json"]);
    let tool = mcp["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "version")
        .unwrap();
    assert_eq!(tool["annotations"]["readOnlyHint"], true);
    assert_eq!(tool["inputSchema"]["type"], "object");

    let ts = read(&repo_file("bindings/commands.ts"));
    assert!(ts.contains("\"version\": { input: VersionInput; output: VersionOutput };"));
}

// --- The parity test catches a deliberately missing command ---------------

/// Dropping any command from any one surface is reported as missing there.
#[test]
fn catches_a_command_missing_from_each_surface() {
    let registry = Registry::builtin();
    let shipped = shipped_surfaces(&registry);
    for surface in Surface::ALL {
        let mut broken = shipped.clone();
        broken
            .iter_mut()
            .find(|(s, _)| s == surface)
            .unwrap()
            .1
            .remove("version");
        let problems = parity::check(&registry, &broken).unwrap_err();
        assert_eq!(
            problems,
            vec![Mismatch::Missing {
                surface: *surface,
                command: "version".into()
            }]
        );
    }
    // And a surface nobody checked is a failure too.
    let problems = parity::check(&registry, &shipped[1..]).unwrap_err();
    assert_eq!(
        problems,
        vec![Mismatch::NotChecked {
            surface: Surface::Cli
        }]
    );
}

/// A command added to the registry without regenerating the bindings is caught.
#[test]
fn catches_a_new_command_with_stale_bindings() {
    struct Ping;
    #[derive(serde::Deserialize, schemars::JsonSchema)]
    struct PingInput {
        /// Times to ping.
        count: i64,
    }
    impl Command for Ping {
        type Input = PingInput;
        type Output = i64;
        const SPEC: CommandSpec = CommandSpec {
            path: &["debug", "ping"],
            summary: "Test-only command",
            description: "",
            access: Access::Read,
            errors: &[],
        };
        fn run(&self, _: &Context<'_>, i: PingInput) -> Result<i64, CommandError> {
            Ok(i.count)
        }
    }

    let mut registry = Registry::builtin();
    registry.register(Ping);

    // Every generated surface picks the new command up automatically...
    let cli = cli_command_names(&build_cli(&registry).unwrap());
    assert!(cli.contains("debug.ping"));
    let mcp: BTreeSet<String> = surfaces::mcp::tool_list(&registry)
        .tools
        .iter()
        .map(|t| t.name.replace('_', "."))
        .collect();
    let schema: BTreeSet<String> = surfaces::schema_document(&registry)
        .commands
        .into_iter()
        .map(|c| c.name)
        .collect();
    // ...but the checked-in bindings still lack it, and parity says so.
    let ts = ts_command_names(&read(&repo_file("bindings/commands.ts")));
    let problems = parity::check(
        &registry,
        &[
            (Surface::Cli, cli),
            (Surface::JsonSchema, schema),
            (Surface::Mcp, mcp),
            (Surface::TypeScript, ts),
        ],
    )
    .unwrap_err();
    assert_eq!(
        problems,
        vec![Mismatch::Missing {
            surface: Surface::TypeScript,
            command: "debug.ping".into()
        }]
    );
}

/// A command whose input can't be expressed as CLI flags fails the CLI build
/// instead of being silently left out.
#[test]
fn unrepresentable_input_fails_the_cli_build() {
    struct Nested;
    #[derive(serde::Deserialize, schemars::JsonSchema)]
    struct Inner {
        a: i64,
    }
    #[derive(serde::Deserialize, schemars::JsonSchema)]
    struct NestedInput {
        inner: Inner,
    }
    impl Command for Nested {
        type Input = NestedInput;
        type Output = ();
        const SPEC: CommandSpec = CommandSpec {
            path: &["debug", "nested"],
            summary: "Test-only command",
            description: "",
            access: Access::Read,
            errors: &[],
        };
        fn run(&self, _: &Context<'_>, i: NestedInput) -> Result<(), CommandError> {
            let _ = i.inner.a;
            Ok(())
        }
    }
    let mut registry = Registry::builtin();
    registry.register(Nested);
    let err = build_cli(&registry).unwrap_err();
    assert!(err.contains("debug.nested"), "{err}");
}

// --- Exit codes through the real binary -------------------------------------

#[test]
fn exit_codes_from_the_binary() {
    assert_eq!(fam(&["version"]).0, 0);
    assert_eq!(fam(&["version", "--bogus"]).0, 2);
    assert_eq!(fam(&["no-such-command"]).0, 2);
    let (code, _, stderr) = fam(&["version", "--bogus", "--json"]);
    assert_eq!(code, 2);
    let err: Value = serde_json::from_str(&stderr).unwrap();
    assert_eq!(err["error"]["kind"], "usage");
    assert_eq!(err["error"]["exit_code"], 2);
}
