// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam folder list` (WP 1.10): every folder on this device with its counts,
//! and `fam folder color` / `pin` / `unpin`: settings kept on this device only
//! ([`crate::folder_prefs`]).
//!
//! Reads the store directly, so it works offline and without `famd`.

use std::collections::HashMap;

use crate::commands::mail::{open_store, pick_accounts, store_err};
use crate::daemon::DaemonPaths;
use crate::error::{CommandError, ErrorKind};
use crate::folder_prefs::{COLOURS, FolderPref, FolderPrefs};
use crate::registry::{Access, Command, CommandSpec, Context};
use familiar_store::{Folder, FolderId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `fam folder list`.
pub struct FolderList;

/// Input for `fam folder list`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FolderListInput {
    /// Only this account (its name in config.toml).
    #[serde(default)]
    pub account: Option<String>,
}

/// One folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FolderRow {
    /// Local folder ID.
    pub id: i64,
    /// Account name.
    pub account: String,
    /// Display name.
    pub name: String,
    /// Full path from the top of the mailbox, such as `Inbox/Club`. Accepted by
    /// `fam mail list --folder`.
    pub path: String,
    /// Parent folder's local ID; absent at the top level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<i64>,
    /// How deep it is: 0 at the top level.
    pub depth: usize,
    /// Graph well-known name (`inbox`, `sentitems`, `deleteditems`, ...), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub well_known: Option<String>,
    /// Messages in the folder on this device.
    pub total: i64,
    /// Unread messages in the folder.
    pub unread: i64,
    /// Graph folder ID.
    pub remote_id: String,
    /// Its colour (`fam folder color`), on this device only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Pinned to the top of the folder pane (`fam folder pin`), on this device only.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
}

/// Output of `fam folder list`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FolderListOutput {
    /// Folders by account, each account's tree depth first, siblings by name.
    pub folders: Vec<FolderRow>,
}

/// Each folder's path (`Inbox/Club`) within its account.
pub(crate) fn folder_paths(folders: &[Folder]) -> HashMap<FolderId, String> {
    let by_id: HashMap<FolderId, &Folder> = folders.iter().map(|f| (f.id, f)).collect();
    folders
        .iter()
        .map(|f| {
            let mut parts = vec![f.name.as_str()];
            let mut at = f.parent;
            // Bounded in case of a cycle in bad data.
            while let Some(p) = at.and_then(|p| by_id.get(&p)) {
                if parts.len() > 64 {
                    break;
                }
                parts.push(&p.name);
                at = p.parent;
            }
            parts.reverse();
            (f.id, parts.join("/"))
        })
        .collect()
}

/// Lists folders from the store at `paths`. Used by the command and by tests.
pub fn folder_list_at(
    paths: &DaemonPaths,
    input: &FolderListInput,
) -> Result<FolderListOutput, CommandError> {
    let r = open_store(paths)?;
    let prefs = FolderPrefs::load(&paths.data_dir);
    let mut out = Vec::new();
    for a in pick_accounts(&r, input.account.as_deref())? {
        let folders = r.folders(a.id).map_err(store_err)?;
        let names = folder_paths(&folders);
        let known: std::collections::HashSet<FolderId> = folders.iter().map(|f| f.id).collect();
        let mut children: HashMap<Option<FolderId>, Vec<&Folder>> = HashMap::new();
        for f in &folders {
            // A folder whose parent isn't stored shows at the top level.
            let parent = f.parent.filter(|p| known.contains(p));
            children.entry(parent).or_default().push(f);
        }
        for list in children.values_mut() {
            list.sort_by(|x, y| {
                (x.name.to_lowercase(), x.id.0).cmp(&(y.name.to_lowercase(), y.id.0))
            });
        }
        let mut stack: Vec<(&Folder, usize)> = children
            .get(&None)
            .map(|v| v.iter().rev().map(|f| (*f, 0)).collect())
            .unwrap_or_default();
        while let Some((f, depth)) = stack.pop() {
            let pref = prefs.get(&a.key, &f.remote_id);
            out.push(FolderRow {
                id: f.id.0,
                account: a.key.clone(),
                name: f.name.clone(),
                path: names.get(&f.id).cloned().unwrap_or_default(),
                parent: f.parent.map(|p| p.0),
                depth,
                well_known: f.well_known.clone(),
                total: f.total,
                unread: f.unread,
                remote_id: f.remote_id.clone(),
                color: pref.color,
                pinned: pref.pinned,
            });
            if depth < 64
                && let Some(kids) = children.get(&Some(f.id))
            {
                stack.extend(kids.iter().rev().map(|k| (*k, depth + 1)));
            }
        }
    }
    Ok(FolderListOutput { folders: out })
}

impl Command for FolderList {
    type Input = FolderListInput;
    type Output = FolderListOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["folder", "list"],
        summary: "List mail folders",
        description: "Lists every folder on this device by account, as a tree, with message and unread counts. Reads the local store, so it works offline and without famd. The path column is what fam mail list --folder takes.",
        access: Access::Read,
        errors: &[ErrorKind::NotFound],
    };

    fn run(
        &self,
        _ctx: &Context<'_>,
        input: FolderListInput,
    ) -> Result<FolderListOutput, CommandError> {
        folder_list_at(&DaemonPaths::resolve()?, &input)
    }

    fn render_text(&self, o: &FolderListOutput) -> String {
        if o.folders.is_empty() {
            return "No folders on this device yet.".into();
        }
        let mut s = String::new();
        let mut account = "";
        for f in &o.folders {
            if f.account != account {
                if !account.is_empty() {
                    s.push('\n');
                }
                account = &f.account;
                s.push_str(&format!("{account}\n"));
            }
            let label = format!("{}{}", "  ".repeat(f.depth + 1), f.name);
            let unread = if f.unread > 0 {
                format!("{} unread", f.unread)
            } else {
                String::new()
            };
            s.push_str(&format!("{:<40} {:>8} {:>12}\n", label, f.total, unread));
        }
        s.trim_end().to_string()
    }
}

// ---------------------------------------------------------------------------
// Colours and pins

/// Finds `wanted` (path, well-known name or name) in the named account, or in
/// whichever account has it; changes its settings; returns the folder's row
/// as `fam folder list` shows it.
fn change_pref_at(
    paths: &DaemonPaths,
    wanted: &str,
    account: Option<&str>,
    f: impl FnOnce(&mut FolderPref),
) -> Result<FolderRow, CommandError> {
    let r = open_store(paths)?;
    let mut found = Vec::new();
    for a in pick_accounts(&r, account)? {
        let all = r.folders(a.id).map_err(store_err)?;
        if let Some(folder) = crate::ops::find_folder(&all, wanted, &a.key)? {
            let path = folder_paths(&all).remove(&folder.id).unwrap_or_default();
            found.push((a.key.clone(), folder.remote_id.clone(), path));
        }
    }
    let (key, remote_id, path) = match found.len() {
        0 => {
            return Err(CommandError::new(
                ErrorKind::NotFound,
                format!("no folder {wanted:?} on this device (see fam folder list)"),
            ));
        }
        1 => found.remove(0),
        _ => {
            let keys: Vec<&str> = found.iter().map(|(k, _, _)| k.as_str()).collect();
            return Err(CommandError::usage(format!(
                "more than one account has a folder {wanted:?}; give --account ({})",
                keys.join(", ")
            )));
        }
    };
    let mut prefs = FolderPrefs::load(&paths.data_dir);
    prefs.update(&key, &remote_id, f);
    prefs
        .save(&paths.data_dir)
        .map_err(|e| CommandError::internal(format!("save {}: {e}", crate::folder_prefs::FILE)))?;
    folder_list_at(
        paths,
        &FolderListInput {
            account: Some(key.clone()),
        },
    )?
    .folders
    .into_iter()
    .find(|row| row.path == path)
    .ok_or_else(|| CommandError::internal("the folder went away while it was changed"))
}

fn render_row(o: &FolderRow) -> String {
    let colour = o.color.as_deref().unwrap_or("no colour");
    let pinned = if o.pinned { "pinned" } else { "not pinned" };
    format!("{} / {}: {colour}, {pinned}", o.account, o.path)
}

/// `fam folder color`.
pub struct FolderColor;

/// Input for `fam folder color`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FolderColorInput {
    /// The folder (path, or name).
    #[schemars(extend("x-positional" = 0))]
    pub folder: String,
    /// red, orange, yellow, green, teal, blue, purple, pink, grey, or none.
    #[schemars(extend("x-positional" = 1))]
    pub color: String,
    /// The account, when more than one has the folder.
    #[serde(default)]
    pub account: Option<String>,
}

/// Sets folder colours from the store at `paths`. Used by the command and by tests.
pub fn folder_color_at(
    paths: &DaemonPaths,
    input: &FolderColorInput,
) -> Result<FolderRow, CommandError> {
    let wanted = input.color.trim().to_ascii_lowercase();
    let colour = match wanted.as_str() {
        "none" | "" => None,
        "gray" => Some("grey".to_string()),
        c if COLOURS.contains(&c) => Some(wanted.clone()),
        _ => {
            return Err(CommandError::usage(format!(
                "unknown colour {:?}; use one of {}, or none",
                input.color,
                COLOURS.join(", ")
            )));
        }
    };
    change_pref_at(paths, &input.folder, input.account.as_deref(), |p| {
        p.color = colour;
    })
}

impl Command for FolderColor {
    type Input = FolderColorInput;
    type Output = FolderRow;
    const SPEC: CommandSpec = CommandSpec {
        path: &["folder", "color"],
        summary: "Colour a mail folder in the folder pane",
        description: "Gives a folder a colour in the desktop app's folder pane, or none to clear it. Kept on this device only (folders.json in the data directory), not on the server; it follows the folder through a rename or move.",
        access: Access::Write,
        errors: &[ErrorKind::NotFound, ErrorKind::Usage],
    };

    fn run(&self, _ctx: &Context<'_>, input: FolderColorInput) -> Result<FolderRow, CommandError> {
        folder_color_at(&DaemonPaths::resolve()?, &input)
    }

    fn render_text(&self, o: &FolderRow) -> String {
        render_row(o)
    }
}

/// Input for `fam folder pin` and `fam folder unpin`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FolderPinInput {
    /// The folder (path, or name).
    #[schemars(extend("x-positional" = 0))]
    pub folder: String,
    /// The account, when more than one has the folder.
    #[serde(default)]
    pub account: Option<String>,
}

/// Pins or unpins a folder in the store at `paths`. Used by the commands and by tests.
pub fn folder_pin_at(
    paths: &DaemonPaths,
    input: &FolderPinInput,
    pinned: bool,
) -> Result<FolderRow, CommandError> {
    change_pref_at(paths, &input.folder, input.account.as_deref(), |p| {
        p.pinned = pinned;
    })
}

/// `fam folder pin`.
pub struct FolderPin;

impl Command for FolderPin {
    type Input = FolderPinInput;
    type Output = FolderRow;
    const SPEC: CommandSpec = CommandSpec {
        path: &["folder", "pin"],
        summary: "Pin a mail folder to the top of the folder pane",
        description: "Lists a folder below All inboxes in the desktop app's folder pane. Kept on this device only (folders.json in the data directory), not on the server.",
        access: Access::Write,
        errors: &[ErrorKind::NotFound, ErrorKind::Usage],
    };

    fn run(&self, _ctx: &Context<'_>, input: FolderPinInput) -> Result<FolderRow, CommandError> {
        folder_pin_at(&DaemonPaths::resolve()?, &input, true)
    }

    fn render_text(&self, o: &FolderRow) -> String {
        render_row(o)
    }
}

/// `fam folder unpin`.
pub struct FolderUnpin;

impl Command for FolderUnpin {
    type Input = FolderPinInput;
    type Output = FolderRow;
    const SPEC: CommandSpec = CommandSpec {
        path: &["folder", "unpin"],
        summary: "Unpin a mail folder",
        description: "Stops listing a pinned folder below All inboxes in the desktop app's folder pane.",
        access: Access::Write,
        errors: &[ErrorKind::NotFound, ErrorKind::Usage],
    };

    fn run(&self, _ctx: &Context<'_>, input: FolderPinInput) -> Result<FolderRow, CommandError> {
        folder_pin_at(&DaemonPaths::resolve()?, &input, false)
    }

    fn render_text(&self, o: &FolderRow) -> String {
        render_row(o)
    }
}
