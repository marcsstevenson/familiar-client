// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam`, the command line. Subcommands and flags are generated from the
//! command registry in `familiar-core`: each command's path becomes nested
//! subcommands and each input field becomes a flag (or a positional argument
//! when the field's schema has `x-positional`).
//!
//! Mapping from input schema to arguments:
//!
//! | Input field | CLI |
//! | --- | --- |
//! | `bool` | `--flag` |
//! | string, integer, number | `--name <value>` |
//! | string enum | `--name <a|b|c>` |
//! | `Vec<_>` of the above | `--name <value>`, repeatable |
//! | `Option<_>` or `#[serde(default)]` | optional; otherwise required |
//!
//! `--json` is global and prints the output as JSON matching `fam schema --json`.

use clap::{Arg, ArgAction, ArgMatches};
use familiar_core::{CommandError, ErrorKind, Registry};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::Write;

/// The value type of one argument.
#[derive(Debug, Clone, PartialEq)]
pub enum ArgKind {
    /// `--flag`.
    Bool,
    /// Whole number.
    Integer,
    /// Any number.
    Number,
    /// Free text.
    String,
    /// One of a fixed set of strings.
    Enum(Vec<String>),
}

/// One CLI argument derived from an input field.
#[derive(Debug, Clone, PartialEq)]
pub struct ArgDef {
    /// The JSON field name.
    pub field: String,
    /// The long flag, kebab-case.
    pub long: String,
    /// The value type.
    pub kind: ArgKind,
    /// Repeatable (the field is an array).
    pub list: bool,
    /// Must be given.
    pub required: bool,
    /// Position, if positional.
    pub position: Option<usize>,
    /// Help text.
    pub help: Option<String>,
}

/// Derives the CLI arguments for an input schema, or explains why it can't.
pub fn args_for(schema: &Value) -> Result<Vec<ArgDef>, String> {
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return Err("input must be a JSON object (a struct)".into());
    }
    let defs = schema.get("$defs").and_then(Value::as_object);
    let required: BTreeSet<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let empty = Map::new();
    let props = schema
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let mut out = Vec::new();
    for (field, prop) in props {
        if field == "json" {
            return Err("field `json` clashes with the global --json flag".into());
        }
        let (prop, nullable) = strip_null(resolve(prop, defs));
        let (kind, list) = match prop.get("type").and_then(Value::as_str) {
            Some("array") => {
                let items = prop
                    .get("items")
                    .ok_or_else(|| format!("`{field}`: array without items"))?;
                let (items, _) = strip_null(resolve(items, defs));
                (
                    scalar(&items)
                        .ok_or_else(|| format!("`{field}`: unsupported array item type"))?,
                    true,
                )
            }
            _ => (
                scalar(&prop)
                    .ok_or_else(|| format!("`{field}`: unsupported type for a CLI argument"))?,
                false,
            ),
        };
        if kind == ArgKind::Bool && (list || prop.get("default") == Some(&Value::Bool(true))) {
            return Err(format!("`{field}`: boolean flags must default to false"));
        }
        let position = prop
            .get("x-positional")
            .and_then(Value::as_u64)
            .map(|p| p as usize);
        out.push(ArgDef {
            field: field.clone(),
            long: field.replace('_', "-"),
            required: required.contains(field.as_str()) && !nullable && kind != ArgKind::Bool,
            kind,
            list,
            position,
            help: prop
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }
    Ok(out)
}

fn resolve<'a>(schema: &'a Value, defs: Option<&'a Map<String, Value>>) -> &'a Value {
    if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
        if let Some(def) = r.strip_prefix("#/$defs/").and_then(|n| defs?.get(n)) {
            return def;
        }
    }
    schema
}

/// Removes a `null` alternative; returns the remaining schema and whether null was allowed.
fn strip_null(schema: &Value) -> (Value, bool) {
    let mut s = schema.clone();
    if let Some(Value::Array(types)) = s.get("type") {
        let rest: Vec<Value> = types.iter().filter(|t| *t != "null").cloned().collect();
        let nullable = rest.len() != types.len();
        if rest.len() == 1 {
            s["type"] = rest[0].clone();
        }
        return (s, nullable);
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(Value::Array(vs)) = s.get(key) {
            let rest: Vec<&Value> = vs
                .iter()
                .filter(|v| v.get("type") != Some(&json!("null")))
                .collect();
            if rest.len() == 1 && vs.len() == 2 {
                return (rest[0].clone(), true);
            }
        }
    }
    (s, false)
}

fn scalar(schema: &Value) -> Option<ArgKind> {
    if let Some(values) = string_enum(schema) {
        return Some(ArgKind::Enum(values));
    }
    match schema.get("type")?.as_str()? {
        "boolean" => Some(ArgKind::Bool),
        "integer" => Some(ArgKind::Integer),
        "number" => Some(ArgKind::Number),
        "string" => Some(ArgKind::String),
        _ => None,
    }
}

fn string_enum(schema: &Value) -> Option<Vec<String>> {
    if let Some(e) = schema.get("enum").and_then(Value::as_array) {
        return e.iter().map(|v| v.as_str().map(str::to_string)).collect();
    }
    let variants = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))?
        .as_array()?;
    let mut values = Vec::new();
    for v in variants {
        if let Some(c) = v.get("const").and_then(Value::as_str) {
            values.push(c.to_string());
        } else {
            values.extend(string_enum(v)?);
        }
    }
    Some(values)
}

struct Node {
    leaf: Option<(String, String, Vec<ArgDef>)>,
    children: BTreeMap<String, Node>,
}

/// Builds the `fam` command tree from a registry.
pub fn build_cli(registry: &Registry) -> Result<clap::Command, String> {
    let mut root = Node {
        leaf: None,
        children: BTreeMap::new(),
    };
    for c in registry.commands() {
        let spec = c.spec();
        let args =
            args_for(&c.input_schema().to_value()).map_err(|e| format!("{}: {e}", spec.name()))?;
        let mut node = &mut root;
        for seg in spec.path {
            node = node
                .children
                .entry(seg.to_string())
                .or_insert_with(|| Node {
                    leaf: None,
                    children: BTreeMap::new(),
                });
        }
        node.leaf = Some((spec.summary.to_string(), spec.description.to_string(), args));
    }
    let cmd = clap::Command::new("fam")
        .about("Familiar: local-first mail and calendar")
        .version(env!("CARGO_PKG_VERSION"))
        .subcommand_required(true)
        .arg_required_else_help(true)
        .arg(
            Arg::new("json")
                .long("json")
                .global(true)
                .action(ArgAction::SetTrue)
                .help("Print output as JSON matching `fam schema --json`"),
        );
    Ok(add_children(cmd, root.children))
}

fn add_children(mut cmd: clap::Command, children: BTreeMap<String, Node>) -> clap::Command {
    for (name, node) in children {
        let mut sub = clap::Command::new(name);
        if let Some((summary, description, args)) = node.leaf {
            sub = sub.about(summary);
            if !description.is_empty() {
                sub = sub.long_about(description);
            }
            for a in args {
                sub = sub.arg(to_clap(&a));
            }
        } else {
            sub = sub.subcommand_required(true).arg_required_else_help(true);
        }
        cmd = cmd.subcommand(add_children(sub, node.children));
    }
    cmd
}

fn to_clap(a: &ArgDef) -> Arg {
    let mut arg = Arg::new(a.field.clone()).required(a.required);
    arg = match a.position {
        Some(p) => arg.index(p + 1),
        None => arg.long(a.long.clone()),
    };
    if let Some(h) = &a.help {
        arg = arg.help(h.clone());
    }
    arg = match &a.kind {
        ArgKind::Bool => arg.action(ArgAction::SetTrue),
        ArgKind::Integer => arg.value_parser(clap::value_parser!(i64)),
        ArgKind::Number => arg.value_parser(clap::value_parser!(f64)),
        ArgKind::String => arg,
        ArgKind::Enum(values) => {
            arg.value_parser(clap::builder::PossibleValuesParser::new(values.clone()))
        }
    };
    if a.list {
        arg = arg.action(ArgAction::Append);
    }
    arg
}

/// The dotted names of every runnable command in a `fam` tree.
pub fn cli_command_names(cmd: &clap::Command) -> BTreeSet<String> {
    fn walk(cmd: &clap::Command, prefix: &mut Vec<String>, out: &mut BTreeSet<String>) {
        for sub in cmd.get_subcommands() {
            prefix.push(sub.get_name().to_string());
            if sub.get_subcommands().next().is_none() {
                out.insert(prefix.join("."));
            } else {
                walk(sub, prefix, out);
            }
            prefix.pop();
        }
    }
    let mut out = BTreeSet::new();
    walk(cmd, &mut Vec::new(), &mut out);
    out
}

/// Turns parsed arguments back into the command's JSON input.
fn input_from(matches: &ArgMatches, args: &[ArgDef]) -> Value {
    let mut obj = Map::new();
    for a in args {
        let id = a.field.as_str();
        let value = match (&a.kind, a.list) {
            (ArgKind::Bool, _) => matches.get_flag(id).then_some(Value::Bool(true)),
            (ArgKind::Integer, false) => matches.get_one::<i64>(id).map(|v| json!(v)),
            (ArgKind::Number, false) => matches.get_one::<f64>(id).map(|v| json!(v)),
            (ArgKind::String | ArgKind::Enum(_), false) => {
                matches.get_one::<String>(id).map(|v| json!(v))
            }
            (ArgKind::Integer, true) => matches
                .get_many::<i64>(id)
                .map(|v| json!(v.collect::<Vec<_>>())),
            (ArgKind::Number, true) => matches
                .get_many::<f64>(id)
                .map(|v| json!(v.collect::<Vec<_>>())),
            (ArgKind::String | ArgKind::Enum(_), true) => matches
                .get_many::<String>(id)
                .map(|v| json!(v.collect::<Vec<_>>())),
        };
        if let Some(v) = value {
            obj.insert(a.field.clone(), v);
        }
    }
    Value::Object(obj)
}

fn error_json(e: &CommandError) -> String {
    serde_json::to_string_pretty(&json!({
        "error": { "kind": e.kind, "message": e.message, "exit_code": e.exit_code() }
    }))
    .unwrap_or_default()
}

/// Runs `fam` with the given arguments (including the program name) and returns the exit code.
pub fn main_with<I, T>(args: I) -> u8
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let want_json = args.iter().any(|a| a == "--json");
    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();
    let fail = |stderr: &mut dyn Write, e: CommandError| {
        if want_json {
            let _ = writeln!(stderr, "{}", error_json(&e));
        } else {
            let _ = writeln!(stderr, "fam: {}", e.message);
        }
        e.exit_code()
    };

    let registry = Registry::builtin();
    let cli = match build_cli(&registry) {
        Ok(c) => c,
        Err(e) => return fail(&mut stderr, CommandError::internal(e)),
    };
    let matches = match cli.try_get_matches_from(&args) {
        Ok(m) => m,
        Err(e) => {
            use clap::error::ErrorKind as K;
            return match e.kind() {
                K::DisplayHelp | K::DisplayVersion => {
                    let _ = e.print();
                    0
                }
                _ if want_json => fail(
                    &mut stderr,
                    CommandError::usage(e.render().to_string().trim()),
                ),
                _ => {
                    let _ = e.print();
                    ErrorKind::Usage.exit_code()
                }
            };
        }
    };
    let json_out = matches.get_flag("json");

    let mut path = Vec::new();
    let mut m = &matches;
    while let Some((name, sub)) = m.subcommand() {
        path.push(name.to_string());
        m = sub;
    }
    let name = path.join(".");
    let Some(cmd) = registry.get(&name) else {
        return fail(
            &mut stderr,
            CommandError::usage(format!("unknown command `{name}`")),
        );
    };
    let args = match args_for(&cmd.input_schema().to_value()) {
        Ok(a) => a,
        Err(e) => return fail(&mut stderr, CommandError::internal(e)),
    };
    match registry.run(&name, input_from(m, &args)) {
        Ok(out) => {
            if json_out {
                let _ = writeln!(
                    stdout,
                    "{}",
                    serde_json::to_string_pretty(&out.json).unwrap_or_default()
                );
            } else {
                let _ = writeln!(stdout, "{}", out.text);
            }
            0
        }
        Err(e) => fail(&mut stderr, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_flags() {
        let r = Registry::builtin();
        let args = args_for(&r.get("version").unwrap().input_schema().to_value()).unwrap();
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].long, "verbose");
        assert_eq!(args[0].kind, ArgKind::Bool);
        assert!(!args[0].required);
    }

    #[test]
    fn schema_format_is_an_enum() {
        let r = Registry::builtin();
        let args = args_for(&r.get("schema").unwrap().input_schema().to_value()).unwrap();
        assert_eq!(
            args[0].kind,
            ArgKind::Enum(vec!["commands".into(), "mcp".into(), "typescript".into()])
        );
    }

    #[test]
    fn cli_tree_is_valid() {
        build_cli(&Registry::builtin()).unwrap().debug_assert();
    }
}
