// SPDX-License-Identifier: MIT OR Apache-2.0
//! `config.toml`: the one file that holds everything a person sets in Familiar.
//!
//! - [`check`] parses and validates a file and reports every problem with its line and column.
//! - [`load`] reads a file and returns the typed [`Config`], or the problems.
//! - [`ConfigDocument`] edits a file in place, keeping comments, blank lines and ordering,
//!   so changes made in the app can be written back to a file people also edit by hand.
//!
//! The file never holds passwords, tokens or mail. See `config.example.toml` at the repo root.

pub mod edit;
pub mod model;
pub mod paths;
pub mod validate;

pub use edit::{ConfigDocument, SignatureKind};
pub use model::{Account, Config, Hook, Signature};
pub use validate::{Problem, Severity};

use std::ops::Range;
use std::path::Path;

/// Familiar's own Microsoft app (client) ID, registered for personal and
/// organisational accounts. Public, not a secret; `[app] client_id` overrides it.
pub const DEFAULT_CLIENT_ID: &str = "c6057b6a-a692-47dd-b34e-12629372e32a";

impl model::App {
    /// The configured client ID, or Familiar's own.
    pub fn client_id_or_default(&self) -> &str {
        self.client_id.as_deref().unwrap_or(DEFAULT_CLIENT_ID)
    }
}

/// The result of checking a config file.
#[derive(Debug, Clone)]
pub struct Report {
    /// The parsed config, when the file has the right shape. It may still have errors in `problems`.
    pub config: Option<Config>,
    /// Errors and warnings, in file order where locations are known.
    pub problems: Vec<Problem>,
}

impl Report {
    /// True when there are no errors (warnings are allowed).
    pub fn is_valid(&self) -> bool {
        self.config.is_some() && self.errors().next().is_none()
    }

    /// The errors.
    pub fn errors(&self) -> impl Iterator<Item = &Problem> {
        self.problems
            .iter()
            .filter(|p| p.severity == Severity::Error)
    }

    /// The warnings.
    pub fn warnings(&self) -> impl Iterator<Item = &Problem> {
        self.problems
            .iter()
            .filter(|p| p.severity == Severity::Warning)
    }
}

/// Parses and validates config text, collecting every problem found.
pub fn check(text: &str) -> Report {
    // TOML syntax first; spans from this document locate later problems.
    let doc = match toml_edit::Document::parse(text) {
        Ok(d) => d,
        Err(e) => {
            let mut p = Problem::error("", e.message().trim_end().to_string());
            locate(&mut p, text, e.span());
            return Report {
                config: None,
                problems: vec![p],
            };
        }
    };
    // Then the shape: types, required keys, unknown keys.
    let config: Config = match toml::from_str(text) {
        Ok(c) => c,
        Err(e) => {
            let mut msg = e.message().trim_end().to_string();
            if msg.starts_with("unknown field `allow`") {
                msg.push_str(
                    "; agent access goes under [agent] (read, draft, write) or [agent.accounts.<name>] (send, delete, rules), not under [accounts.<name>]",
                );
            }
            let mut p = Problem::error("", msg);
            locate(&mut p, text, e.span());
            return Report {
                config: None,
                problems: vec![p],
            };
        }
    };
    // Then the meaning.
    let mut problems = validate::check(&config);
    for p in &mut problems {
        let span = span_of(doc.as_item(), &p.path);
        locate(p, text, span);
    }
    problems.sort_by_key(|p| (p.line.unwrap_or(0), p.column.unwrap_or(0)));
    Report {
        config: Some(config),
        problems,
    }
}

/// An error reading or validating a config file.
#[derive(Debug)]
pub enum LoadError {
    /// The file could not be read.
    Io(std::io::Error),
    /// The file has errors.
    Invalid(Vec<Problem>),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io(e) => write!(f, "could not read config: {e}"),
            LoadError::Invalid(p) => {
                for (i, p) in p.iter().enumerate() {
                    if i > 0 {
                        f.write_str("\n")?;
                    }
                    write!(f, "{p}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for LoadError {}

/// Reads and validates a config file. Warnings are dropped; use [`check`] to see them.
pub fn load(path: &Path) -> Result<Config, LoadError> {
    let text = std::fs::read_to_string(path).map_err(LoadError::Io)?;
    let report = check(&text);
    if report.is_valid() {
        Ok(report.config.expect("valid reports have a config"))
    } else {
        Err(LoadError::Invalid(report.errors().cloned().collect()))
    }
}

fn locate(p: &mut Problem, text: &str, span: Option<Range<usize>>) {
    if let Some(span) = span {
        let (line, column) = line_col(text, span.start);
        p.line = Some(line);
        p.column = Some(column);
    }
}

/// 1-based line and column (in characters) of a byte offset.
fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let before = &text[..offset];
    let line = before.matches('\n').count() + 1;
    let start = before.rfind('\n').map_or(0, |i| i + 1);
    (line, before[start..].chars().count() + 1)
}

/// Finds the span of a dotted path such as `accounts.hotmail.email` or
/// `views[1].name`, falling back to the nearest parent with a span.
fn span_of(root: &toml_edit::Item, path: &str) -> Option<Range<usize>> {
    use toml_edit::{Item, Value};
    let mut table: &dyn toml_edit::TableLike = root.as_table_like()?;
    let mut best = None;
    let segments: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
    for (n, seg) in segments.iter().enumerate() {
        let last = n + 1 == segments.len();
        let (key, index) = match seg.split_once('[') {
            Some((k, rest)) => (k, rest.trim_end_matches(']').parse::<usize>().ok()),
            None => (*seg, None),
        };
        let Some((k, item)) = table.get_key_value(key) else {
            break;
        };
        best = k.span().or(best);
        let next: Option<&dyn toml_edit::TableLike> = match (item, index) {
            (Item::ArrayOfTables(a), Some(i)) => match a.get(i) {
                Some(t) => {
                    best = t.span().or(best);
                    Some(t as &dyn toml_edit::TableLike)
                }
                None => None,
            },
            (Item::Value(Value::Array(a)), Some(i)) => {
                if let Some(v) = a.get(i) {
                    best = v.span().or(best);
                }
                v_table(a.get(i))
            }
            (Item::Value(v), None) => {
                if last {
                    best = v.span().or(best);
                }
                v.as_inline_table().map(|t| t as &dyn toml_edit::TableLike)
            }
            (Item::Table(t), None) => Some(t as &dyn toml_edit::TableLike),
            _ => None,
        };
        match next {
            Some(t) => table = t,
            None => break,
        }
    }
    best
}

fn v_table(v: Option<&toml_edit::Value>) -> Option<&dyn toml_edit::TableLike> {
    v?.as_inline_table().map(|t| t as &dyn toml_edit::TableLike)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_counts_from_one() {
        let t = "a = 1\nbb = 2\n";
        assert_eq!(line_col(t, 0), (1, 1));
        assert_eq!(line_col(t, 6), (2, 1));
        assert_eq!(line_col(t, 9), (2, 4));
    }
}
