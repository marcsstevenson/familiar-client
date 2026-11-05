// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam folder list` (WP 1.10): every folder on this device with its counts.
//!
//! Reads the store directly, so it works offline and without `famd`.

use std::collections::HashMap;

use crate::commands::mail::{open_store, pick_accounts, store_err};
use crate::daemon::DaemonPaths;
use crate::error::{CommandError, ErrorKind};
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
