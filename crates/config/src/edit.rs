// SPDX-License-Identifier: MIT OR Apache-2.0
//! Comment-preserving edits. The app changes one setting at a time and writes
//! the file back; everything it didn't touch stays byte-for-byte the same,
//! including comments, blank lines, key order and string styles.

use crate::{Report, check};
use std::path::Path;
use toml_edit::{DocumentMut, Item, Table, Value};

/// Which signature to set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureKind {
    /// For new messages.
    New,
    /// For replies and forwards.
    Reply,
}

impl SignatureKind {
    fn key(self) -> &'static str {
        match self {
            SignatureKind::New => "new",
            SignatureKind::Reply => "reply",
        }
    }
}

/// A config file open for editing.
#[derive(Debug, Clone)]
pub struct ConfigDocument {
    doc: DocumentMut,
    /// The file used Windows line endings; write them back the same way.
    crlf: bool,
}

/// An edit that can't be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditError(pub String);

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EditError {}

impl ConfigDocument {
    /// Parses config text. Only TOML syntax is checked here; call [`Self::check`] to validate.
    pub fn parse(text: &str) -> Result<Self, EditError> {
        text.parse::<DocumentMut>()
            .map(|doc| Self {
                doc,
                crlf: text.contains("\r\n"),
            })
            .map_err(|e| EditError(e.message().trim_end().to_string()))
    }

    /// Reads a config file for editing.
    pub fn open(path: &Path) -> Result<Self, EditError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| EditError(format!("could not read {}: {e}", path.display())))?;
        Self::parse(&text)
    }

    /// Validates the document as it stands now.
    pub fn check(&self) -> Report {
        check(&self.to_string())
    }

    /// Sets a value, creating parent tables as needed. `path` is a list of
    /// keys, for example `["app", "theme"]`. An existing value keeps its place
    /// and its comments; a new key goes at the end of its table.
    pub fn set(&mut self, path: &[&str], value: impl Into<Value>) -> Result<(), EditError> {
        let (last, parents) = path
            .split_last()
            .ok_or_else(|| EditError("empty path".into()))?;
        let table = self.table_mut(parents)?;
        let mut value: Value = value.into();
        match table.get_mut(last) {
            Some(Item::Value(old)) => {
                // Keep the comment and spacing around the old value.
                *value.decor_mut() = old.decor().clone();
                *old = value;
            }
            Some(_) => {
                return Err(EditError(format!(
                    "`{}` is a table, not a value",
                    path.join(".")
                )));
            }
            None => {
                table.insert(last, Item::Value(value));
            }
        }
        Ok(())
    }

    /// Sets an account's signature, written as a multi-line literal string so
    /// the HTML stays readable and needs no escaping.
    pub fn set_signature(
        &mut self,
        account: &str,
        kind: SignatureKind,
        html: &str,
    ) -> Result<(), EditError> {
        if self
            .doc
            .get("accounts")
            .and_then(|a| a.get(account))
            .is_none()
        {
            return Err(EditError(format!("no account called `{account}`")));
        }
        self.set(
            &["accounts", account, "signature", kind.key()],
            multiline_string(html),
        )
    }

    /// Removes a value or table. Returns false if it wasn't there.
    pub fn remove(&mut self, path: &[&str]) -> bool {
        let Some((last, parents)) = path.split_last() else {
            return false;
        };
        let mut item = self.doc.as_item_mut();
        for p in parents {
            match item.get_mut(p) {
                Some(i) => item = i,
                None => return false,
            }
        }
        match item.as_table_like_mut() {
            Some(t) => t.remove(last).is_some(),
            None => false,
        }
    }

    /// The underlying document, for edits the helpers above don't cover
    /// (for example `[[views]]` entries).
    pub fn document_mut(&mut self) -> &mut DocumentMut {
        &mut self.doc
    }

    /// Writes the file atomically (a temporary file in the same folder, then
    /// a rename), so a crash never leaves half a config. Refuses to write a
    /// document that fails validation.
    pub fn save(&self, path: &Path) -> Result<(), EditError> {
        let text = self.to_string();
        let report = check(&text);
        if let Some(e) = report.errors().next() {
            return Err(EditError(format!("not saved, the result is invalid: {e}")));
        }
        let name = path
            .file_name()
            .ok_or_else(|| EditError(format!("{} is not a file path", path.display())))?;
        let mut tmp_name = name.to_os_string();
        tmp_name.push(".tmp");
        let tmp = path.with_file_name(tmp_name);
        let io = |e: std::io::Error| EditError(format!("could not write {}: {e}", path.display()));
        std::fs::write(&tmp, text.as_bytes()).map_err(io)?;
        std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            io(e)
        })
    }

    fn table_mut(&mut self, path: &[&str]) -> Result<&mut Table, EditError> {
        let mut table = self.doc.as_table_mut();
        for (i, key) in path.iter().enumerate() {
            if !table.contains_key(key) {
                let mut t = Table::new();
                // A table that only holds tables needs no header of its own.
                t.set_implicit(i + 1 < path.len());
                table.insert(key, Item::Table(t));
            }
            table = match table.get_mut(key) {
                Some(Item::Table(t)) => t,
                _ => {
                    return Err(EditError(format!(
                        "`{}` is not a table",
                        path[..=i].join(".")
                    )));
                }
            };
        }
        Ok(table)
    }
}

impl std::fmt::Display for ConfigDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = self.doc.to_string();
        if self.crlf {
            // toml_edit writes `\n` between items; restore the file's endings.
            f.write_str(&text.replace("\r\n", "\n").replace('\n', "\r\n"))
        } else {
            f.write_str(&text)
        }
    }
}

/// A string value written as `'''…'''` when possible, otherwise as a normal
/// escaped string.
fn multiline_string(s: &str) -> Value {
    let literal_ok = !s.contains("'''")
        && !s.ends_with('\'')
        && s.chars()
            .all(|c| c == '\n' || c == '\t' || c == '\r' || !c.is_control());
    if literal_ok {
        let body = if s.ends_with('\n') {
            s.to_string()
        } else {
            format!("{s}\n")
        };
        // Parse the raw form so the value keeps that representation. The
        // newline straight after the opening quotes is not part of the value.
        if let Ok(v) = format!("'''\n{body}'''").parse::<Value>()
            && v.as_str() == Some(body.as_str())
        {
            return v;
        }
    }
    Value::from(s)
}
