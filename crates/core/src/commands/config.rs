// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam config validate` and `fam config path`.

use crate::error::{CommandError, ErrorKind};
use crate::registry::{Access, Command, CommandSpec, Context};
use familiar_config::paths;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Input shared by the config commands.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigPathInput {
    /// The config file. Defaults to FAMILIAR_CONFIG, then the platform default (see `fam config path`).
    #[schemars(extend("x-positional" = 0))]
    pub path: Option<String>,
}

fn resolve(input: &ConfigPathInput) -> Result<(PathBuf, paths::Source), CommandError> {
    paths::resolve(input.path.as_deref().map(Path::new)).ok_or_else(|| {
        CommandError::new(
            ErrorKind::Config,
            format!(
                "can't find a home or app data folder; pass a path or set {}",
                paths::ENV_VAR
            ),
        )
    })
}

/// `fam config validate`.
pub struct ConfigValidate;

/// A warning found in the config.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ConfigWarning {
    /// Dotted key path, for example `accounts.parklands.tenant`.
    pub key: String,
    /// 1-based line, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    /// 1-based column, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<usize>,
    /// What looks wrong.
    pub message: String,
}

/// An account in the config.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ConfigAccount {
    /// The account name used with `--account`.
    pub name: String,
    /// The mailbox address.
    pub email: String,
    /// The sign-in tenant.
    pub tenant: String,
    /// Whether new and reply signatures are set.
    pub signatures: SignatureState,
}

/// Which signatures an account has.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SignatureState {
    /// A signature for new messages is set.
    pub new: bool,
    /// A signature for replies is set.
    pub reply: bool,
}

/// Output of `fam config validate`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ConfigValidateOutput {
    /// The file that was checked.
    pub path: String,
    /// The config file format version.
    pub version: u32,
    /// The Microsoft app (client) ID in use.
    pub client_id: String,
    /// Accounts, by name.
    pub accounts: Vec<ConfigAccount>,
    /// Problems that don't stop the config being used.
    pub warnings: Vec<ConfigWarning>,
}

impl Command for ConfigValidate {
    type Input = ConfigPathInput;
    type Output = ConfigValidateOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["config", "validate"],
        summary: "Check config.toml for errors",
        description: "Parses and checks the config file: syntax, unknown or misspelt keys, types, references between sections (accounts named by rules and calendar purposes), formats and duplicates. Every problem is reported with its line and column. Exits with the config error code if anything is wrong; warnings alone don't fail.",
        access: Access::Read,
        errors: &[ErrorKind::Config],
    };

    fn run(
        &self,
        _ctx: &Context<'_>,
        input: ConfigPathInput,
    ) -> Result<ConfigValidateOutput, CommandError> {
        let (path, _) = resolve(&input)?;
        let shown = path.display().to_string();
        let text = std::fs::read_to_string(&path).map_err(|e| {
            let msg = if e.kind() == std::io::ErrorKind::NotFound {
                format!("no config at {shown}; start from config.example.toml in the Familiar repo")
            } else {
                format!("could not read {shown}: {e}")
            };
            CommandError::new(ErrorKind::Config, msg)
        })?;
        let report = familiar_config::check(&text);
        if !report.is_valid() {
            let lines: Vec<String> = report
                .errors()
                .map(|p| match p.line {
                    Some(_) => format!("{shown}:{p}"),
                    None => format!("{shown}: {p}"),
                })
                .collect();
            let n = lines.len();
            return Err(CommandError::new(
                ErrorKind::Config,
                format!(
                    "{n} error{} in config\n{}",
                    if n == 1 { "" } else { "s" },
                    lines.join("\n")
                ),
            ));
        }
        let warnings = report
            .warnings()
            .map(|p| ConfigWarning {
                key: p.path.clone(),
                line: p.line,
                column: p.column,
                message: p.message.clone(),
            })
            .collect();
        let config = report.config.expect("valid reports have a config");
        Ok(ConfigValidateOutput {
            path: shown,
            version: config.version,
            client_id: config.app.client_id_or_default().to_string(),
            accounts: config
                .accounts
                .iter()
                .map(|(name, a)| ConfigAccount {
                    name: name.clone(),
                    email: a.email.clone(),
                    tenant: a.tenant.clone(),
                    signatures: SignatureState {
                        new: a.signature.as_ref().is_some_and(|s| s.new.is_some()),
                        reply: a.signature.as_ref().is_some_and(|s| s.reply.is_some()),
                    },
                })
                .collect(),
            warnings,
        })
    }

    fn render_text(&self, o: &ConfigValidateOutput) -> String {
        let names: Vec<&str> = o.accounts.iter().map(|a| a.name.as_str()).collect();
        let mut s = format!(
            "{} is valid: {} account{}",
            o.path,
            names.len(),
            if names.len() == 1 { "" } else { "s" }
        );
        if !names.is_empty() {
            s.push_str(&format!(" ({})", names.join(", ")));
        }
        for w in &o.warnings {
            s.push_str(&format!("\n{}:", o.path));
            if let Some(l) = w.line {
                s.push_str(&format!("{l}:"));
                if let Some(c) = w.column {
                    s.push_str(&format!("{c}:"));
                }
            }
            s.push_str(&format!(" warning: {}: {}", w.key, w.message));
        }
        s
    }
}

/// `fam config path`.
pub struct ConfigPath;

/// Output of `fam config path`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ConfigPathOutput {
    /// The config file Familiar uses.
    pub path: String,
    /// Where the path came from.
    pub source: ConfigPathSource,
    /// Whether the file exists.
    pub exists: bool,
}

/// Where the config path came from.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConfigPathSource {
    /// Given as an argument.
    Explicit,
    /// The FAMILIAR_CONFIG environment variable.
    Env,
    /// The platform default.
    Default,
}

impl Command for ConfigPath {
    type Input = ConfigPathInput;
    type Output = ConfigPathOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["config", "path"],
        summary: "Show which config file is used",
        description: "Shows the config file path: the argument if given, else FAMILIAR_CONFIG, else the platform default (Windows %APPDATA%\\Familiar\\config.toml, macOS ~/Library/Application Support/Familiar/config.toml, Linux $XDG_CONFIG_HOME/familiar/config.toml).",
        access: Access::Read,
        errors: &[ErrorKind::Config],
    };

    fn run(
        &self,
        _ctx: &Context<'_>,
        input: ConfigPathInput,
    ) -> Result<ConfigPathOutput, CommandError> {
        let (path, source) = resolve(&input)?;
        Ok(ConfigPathOutput {
            exists: path.is_file(),
            path: path.display().to_string(),
            source: match source {
                paths::Source::Explicit => ConfigPathSource::Explicit,
                paths::Source::Env => ConfigPathSource::Env,
                paths::Source::Default => ConfigPathSource::Default,
            },
        })
    }

    fn render_text(&self, o: &ConfigPathOutput) -> String {
        if o.exists {
            o.path.clone()
        } else {
            format!("{} (does not exist yet)", o.path)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Registry;
    use crate::registry::Access;
    use serde_json::json;

    const EXAMPLE: &str = include_str!("../../../../config.example.toml");

    fn temp_file(name: &str, text: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("familiar-core-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        std::fs::write(&p, text).unwrap();
        p
    }

    #[test]
    fn validate_example() {
        let p = temp_file("ok", EXAMPLE);
        let out = Registry::builtin()
            .run("config.validate", json!({"path": p}))
            .unwrap();
        assert_eq!(out.json["accounts"].as_array().unwrap().len(), 2);
        assert_eq!(out.json["client_id"], familiar_config::DEFAULT_CLIENT_ID);
        assert!(
            out.text
                .contains("is valid: 2 accounts (hotmail, parklands)")
        );
    }

    #[test]
    fn validate_reports_every_error_with_config_kind() {
        let p = temp_file(
            "bad",
            "version = 1\n[accounts.a]\nemail = \"x\"\ntenant = \"?\"\n",
        );
        let err = Registry::builtin()
            .run("config.validate", json!({"path": p}))
            .unwrap_err();
        assert_eq!(err.kind, crate::ErrorKind::Config);
        assert_eq!(err.exit_code(), 3);
        assert!(
            err.message.starts_with("2 errors in config"),
            "{}",
            err.message
        );
        assert!(
            err.message.contains(":3:9: accounts.a.email"),
            "{}",
            err.message
        );
    }

    #[test]
    fn validate_missing_file() {
        let err = Registry::builtin()
            .run(
                "config.validate",
                json!({"path": "/no/such/familiar/config.toml"}),
            )
            .unwrap_err();
        assert_eq!(err.kind, crate::ErrorKind::Config);
        assert!(err.message.contains("no config at"));
    }

    #[test]
    fn path_explicit() {
        let out = Registry::builtin()
            .run("config.path", json!({"path": "x.toml"}))
            .unwrap();
        assert_eq!(out.json["source"], "explicit");
        assert_eq!(out.json["exists"], false);
    }

    /// `agent.allow` in config.toml accepts exactly the access levels commands declare.
    #[test]
    fn agent_access_matches_access_enum() {
        let all = [
            Access::Read,
            Access::Draft,
            Access::Write,
            Access::Send,
            Access::Delete,
            Access::Rules,
        ];
        let names: Vec<String> = all
            .iter()
            .map(|a| {
                serde_json::to_value(a)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(names, familiar_config::model::AGENT_ACCESS);
        // A new Access variant must be added to both lists.
        match Access::Read {
            Access::Read
            | Access::Draft
            | Access::Write
            | Access::Send
            | Access::Delete
            | Access::Rules => {}
        }
    }
}
