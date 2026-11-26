// SPDX-License-Identifier: MIT OR Apache-2.0
//! Folder colours, pins and ordering in config.toml, keyed by account and provider ID.

use crate::{CommandError, ErrorKind};
use familiar_config::{ConfigDocument, edit::EditError, model::FolderPref};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// The colours supported by the desktop folder pane.
pub const COLOURS: &[&str] = &[
    "red", "orange", "yellow", "green", "teal", "blue", "purple", "pink", "grey",
];

type Folders = BTreeMap<String, BTreeMap<String, FolderPref>>;

/// The active config used by desktop and command-line preferences.
pub fn config_path() -> Result<PathBuf, CommandError> {
    familiar_config::paths::resolve(None)
        .map(|(path, _)| path)
        .ok_or_else(|| CommandError::new(ErrorKind::Config, "no config path"))
}

/// Imports an old JSON preferences file once. Explicit TOML settings take precedence.
pub fn import_legacy<T: serde::de::DeserializeOwned>(
    config: &Path,
    legacy: &Path,
    apply: impl FnOnce(&mut ConfigDocument, T) -> Result<(), EditError>,
) -> Result<(), String> {
    let bytes = match std::fs::read(legacy) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("{}: {e}", legacy.display())),
    };
    let old = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", legacy.display()))?;
    ConfigDocument::update(config, |doc| apply(doc, old)).map_err(|e| e.to_string())?;
    std::fs::rename(legacy, legacy.with_extension("json.migrated")).or_else(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(e.to_string())
        }
    })
}

/// Reads the folder settings, importing a previous install's colours, pins and order.
pub fn load(config: &Path, data: &Path) -> Result<Folders, CommandError> {
    if let Err(e) = import_legacy(config, &data.join("folders.json"), |doc, old: Folders| {
        for (account, folders) in old {
            for (id, pref) in folders {
                let path = ["folders", &account, &id];
                if !doc.contains(&path) {
                    doc.set_serialized(&path, &pref)?;
                }
            }
        }
        Ok(())
    }) {
        eprintln!("familiar: importing folder preferences: {e}");
    }
    match familiar_config::load(config) {
        Ok(c) => Ok(c.folders),
        Err(familiar_config::LoadError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(Folders::new())
        }
        Err(e) => Err(CommandError::new(ErrorKind::Config, e.to_string())),
    }
}

/// Changes one folder, retaining every other setting and removing cleared values.
pub fn change(
    doc: &mut ConfigDocument,
    account: &str,
    id: &str,
    f: impl FnOnce(&mut FolderPref),
) -> Result<(), EditError> {
    let path = ["folders", account, id];
    let mut pref = doc.get(&path)?.unwrap_or_default();
    f(&mut pref);
    if pref == FolderPref::default() {
        doc.remove(&path);
    } else {
        doc.set_serialized(&path, &pref)?;
    }
    Ok(())
}
