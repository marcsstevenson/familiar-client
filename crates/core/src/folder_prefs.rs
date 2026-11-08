// SPDX-License-Identifier: MIT OR Apache-2.0
//! Folder colours and pins: settings kept on this device only, in
//! `folders.json` in the data directory. Keyed by account and Graph folder ID,
//! so they follow a folder through a rename or a move.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The file name in the data directory.
pub const FILE: &str = "folders.json";

/// The colours a folder can have. The desktop app has a light and a dark
/// shade of each.
pub const COLOURS: &[&str] = &[
    "red", "orange", "yellow", "green", "teal", "blue", "purple", "pink", "grey",
];

/// One folder's settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderPref {
    /// One of [`COLOURS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Listed at the top of the folder pane.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
}

impl FolderPref {
    fn is_empty(&self) -> bool {
        self.color.is_none() && !self.pinned
    }
}

/// Every folder's settings: account key, then Graph folder ID.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderPrefs(pub BTreeMap<String, BTreeMap<String, FolderPref>>);

impl FolderPrefs {
    /// Reads `folders.json` from `data_dir`. A missing file is empty; an
    /// unreadable one is reported on stderr and treated as empty.
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join(FILE);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                eprintln!("familiar: ignoring an unreadable {}: {e}", path.display());
                Self::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                eprintln!("familiar: couldn't read {}: {e}", path.display());
                Self::default()
            }
        }
    }

    /// Writes `folders.json` to `data_dir` (through a temporary file, so a
    /// reader never sees half of it).
    pub fn save(&self, data_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let path = data_dir.join(FILE);
        let tmp = data_dir.join(format!("{FILE}.tmp"));
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &path)
    }

    /// The settings of one folder.
    pub fn get(&self, account: &str, remote_id: &str) -> FolderPref {
        self.0
            .get(account)
            .and_then(|a| a.get(remote_id))
            .cloned()
            .unwrap_or_default()
    }

    /// Changes one folder's settings, dropping entries left empty.
    pub fn update(&mut self, account: &str, remote_id: &str, f: impl FnOnce(&mut FolderPref)) {
        let folders = self.0.entry(account.to_string()).or_default();
        let pref = folders.entry(remote_id.to_string()).or_default();
        f(pref);
        if pref.is_empty() {
            folders.remove(remote_id);
        }
        if folders.is_empty() {
            self.0.remove(account);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_empty_entries_dropped() {
        let dir = std::env::temp_dir().join(format!("fam-prefs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(FolderPrefs::load(&dir), FolderPrefs::default());

        let mut p = FolderPrefs::default();
        p.update("hotmail", "AAA", |f| f.pinned = true);
        p.update("hotmail", "BBB", |f| f.color = Some("red".into()));
        p.save(&dir).unwrap();
        let back = FolderPrefs::load(&dir);
        assert!(back.get("hotmail", "AAA").pinned);
        assert_eq!(back.get("hotmail", "BBB").color.as_deref(), Some("red"));

        let mut p = back;
        p.update("hotmail", "AAA", |f| f.pinned = false);
        p.update("hotmail", "BBB", |f| f.color = None);
        assert!(p.0.is_empty());

        std::fs::write(dir.join(FILE), b"not json").unwrap();
        assert_eq!(FolderPrefs::load(&dir), FolderPrefs::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
