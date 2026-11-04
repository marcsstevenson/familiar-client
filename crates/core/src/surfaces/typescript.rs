// SPDX-License-Identifier: MIT OR Apache-2.0
//! TypeScript bindings for the desktop app, generated from the registry's JSON
//! schemas. The checked-in copy is `bindings/commands.ts`; the parity test
//! fails if it is stale.

use crate::error::ErrorKind;
use crate::registry::Registry;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Renders the bindings for every command in the registry.
pub fn bindings(registry: &Registry) -> String {
    let mut named: BTreeMap<String, String> = BTreeMap::new();
    let mut commands = Vec::new();

    for c in registry.commands() {
        let spec = c.spec();
        let base = pascal(&spec.path.join("_"));
        let input = add_root(
            &mut named,
            c.input_schema().to_value(),
            &format!("{base}Input"),
        );
        let output = add_root(
            &mut named,
            c.output_schema().to_value(),
            &format!("{base}Output"),
        );
        commands.push((spec.name(), spec.summary, input, output));
    }

    let mut out = String::from(
        "// SPDX-License-Identifier: MIT OR Apache-2.0\n\
         // Generated from the Familiar command registry by `fam schema --format typescript`.\n\
         // Do not edit. Regenerate with: FAMILIAR_BLESS=1 cargo test -p familiar-cli --test parity\n\n",
    );

    // Errors and exit codes.
    let kinds: Vec<String> = ErrorKind::ALL
        .iter()
        .map(|k| format!("\"{}\"", k.as_str()))
        .collect();
    out.push_str("/** Stable error categories. */\n");
    out.push_str(&format!(
        "export type ErrorKind =\n  | {};\n\n",
        kinds.join("\n  | ")
    ));
    out.push_str("/** Process exit code for each error kind. 0 is success. */\n");
    out.push_str("export const EXIT_CODES: Readonly<Record<ErrorKind, number>> = {\n");
    for k in ErrorKind::ALL {
        out.push_str(&format!("  {}: {},\n", k.as_str(), k.exit_code()));
    }
    out.push_str("};\n\n");
    out.push_str("/** An error returned by a command. */\nexport type CommandError = {\n  kind: ErrorKind;\n  message: string;\n};\n\n");

    for (name, body) in &named {
        out.push_str(&format!("export type {name} = {body};\n\n"));
    }

    out.push_str("/** Every command, by dotted name, with its input and output types. */\nexport interface Commands {\n");
    for (name, summary, input, output) in &commands {
        out.push_str(&format!(
            "  /** {summary} */\n  \"{name}\": {{ input: {input}; output: {output} }};\n"
        ));
    }
    out.push_str("}\n\n/** A command name. */\nexport type CommandName = keyof Commands;\n\n");
    out.push_str("/** Every command name, in order. */\nexport const COMMAND_NAMES = [\n");
    for (name, ..) in &commands {
        out.push_str(&format!("  \"{name}\",\n"));
    }
    out.push_str("] as const satisfies readonly CommandName[];\n");
    out
}

/// Types the generator writes itself, so `$defs` copies are skipped.
const HAND_WRITTEN: &[&str] = &["ErrorKind", "CommandError"];

/// Registers a root schema and its `$defs`; returns the root type's name.
fn add_root(named: &mut BTreeMap<String, String>, schema: Value, fallback: &str) -> String {
    if let Some(defs) = schema.get("$defs").and_then(Value::as_object) {
        for (name, def) in defs {
            // Written by hand above the generated types.
            if HAND_WRITTEN.contains(&name.as_str()) {
                continue;
            }
            insert(named, name.clone(), ts_type(def, 0));
        }
    }
    let name = schema
        .get("title")
        .and_then(Value::as_str)
        .filter(|t| is_ident(t))
        .unwrap_or(fallback)
        .to_string();
    insert(named, name.clone(), ts_type(&schema, 0));
    name
}

fn insert(named: &mut BTreeMap<String, String>, name: String, body: String) {
    if let Some(existing) = named.get(&name) {
        assert_eq!(
            existing, &body,
            "two different types are both named `{name}`"
        );
    }
    named.insert(name, body);
}

/// Converts a JSON schema (the subset schemars produces) to a TS type expression.
fn ts_type(schema: &Value, indent: usize) -> String {
    let obj = match schema {
        Value::Bool(true) => return "unknown".into(),
        Value::Bool(false) => return "never".into(),
        Value::Object(o) => o,
        _ => return "unknown".into(),
    };
    if let Some(r) = obj.get("$ref").and_then(Value::as_str) {
        return r.rsplit('/').next().unwrap_or("unknown").to_string();
    }
    if let Some(c) = obj.get("const") {
        return c.to_string();
    }
    if let Some(e) = obj.get("enum").and_then(Value::as_array) {
        return e
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join(" | ");
    }
    for key in ["oneOf", "anyOf"] {
        if let Some(variants) = obj.get(key).and_then(Value::as_array) {
            return variants
                .iter()
                .map(|v| ts_type(v, indent))
                .collect::<Vec<_>>()
                .join(" | ");
        }
    }
    if let Some(all) = obj.get("allOf").and_then(Value::as_array) {
        return all
            .iter()
            .map(|v| ts_type(v, indent))
            .collect::<Vec<_>>()
            .join(" & ");
    }
    match obj.get("type") {
        Some(Value::Array(types)) => types
            .iter()
            .map(|t| {
                let mut one = obj.clone();
                one.insert("type".into(), t.clone());
                ts_type(&Value::Object(one), indent)
            })
            .collect::<Vec<_>>()
            .join(" | "),
        Some(Value::String(t)) => match t.as_str() {
            "string" => "string".into(),
            "integer" | "number" => "number".into(),
            "boolean" => "boolean".into(),
            "null" => "null".into(),
            "array" => {
                let item = obj
                    .get("items")
                    .map(|i| ts_type(i, indent))
                    .unwrap_or("unknown".into());
                format!("Array<{item}>")
            }
            "object" => ts_object(obj, indent),
            _ => "unknown".into(),
        },
        _ => "unknown".into(),
    }
}

fn ts_object(obj: &Map<String, Value>, indent: usize) -> String {
    let extra = match obj.get("additionalProperties") {
        Some(Value::Bool(false)) => None,
        Some(v @ Value::Object(_)) => Some(ts_type(v, indent)),
        _ => Some("unknown".to_string()),
    };
    let props = match obj.get("properties").and_then(Value::as_object) {
        Some(p) if !p.is_empty() => p,
        _ => return format!("Record<string, {}>", extra.unwrap_or("never".into())),
    };
    let required: Vec<&str> = obj
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let pad = "  ".repeat(indent + 1);
    let mut s = String::from("{\n");
    for (name, prop) in props {
        if let Some(d) = prop.get("description").and_then(Value::as_str) {
            s.push_str(&format!(
                "{pad}/** {} */\n",
                d.replace("*/", "* /").replace('\n', " ")
            ));
        }
        let key = if is_ident(name) {
            name.clone()
        } else {
            format!("\"{name}\"")
        };
        let opt = if required.contains(&name.as_str()) {
            ""
        } else {
            "?"
        };
        s.push_str(&format!(
            "{pad}{key}{opt}: {};\n",
            ts_type(prop, indent + 1)
        ));
    }
    s.push_str(&"  ".repeat(indent));
    s.push('}');
    s
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn pascal(s: &str) -> String {
    s.split(['_', '-', '.'])
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            c.next()
                .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
                .unwrap_or_default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn converts_common_shapes() {
        assert_eq!(
            ts_type(&json!({"type": ["string", "null"]}), 0),
            "string | null"
        );
        assert_eq!(
            ts_type(&json!({"type": "array", "items": {"type": "integer"}}), 0),
            "Array<number>"
        );
        assert_eq!(ts_type(&json!({"enum": ["a", "b"]}), 0), "\"a\" | \"b\"");
        assert_eq!(ts_type(&json!({"$ref": "#/$defs/Thing"}), 0), "Thing");
        assert_eq!(
            ts_type(
                &json!({"type": "object", "properties": {"a": {"type": "boolean"}}, "required": ["a"]}),
                0
            ),
            "{\n  a: boolean;\n}"
        );
    }

    #[test]
    fn pascal_case() {
        assert_eq!(pascal("mail_list"), "MailList");
    }
}
