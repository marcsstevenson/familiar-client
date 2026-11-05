// SPDX-License-Identifier: MIT OR Apache-2.0
//! Server search for mail whose text isn't on this device.
//!
//! Bodies are kept locally only for the sync window (one year by default),
//! so a word in an older message's text can only be found by the server.
//! Graph's `$search` does that, but it returns at most 1,000 results and
//! can't be combined with `$filter` or `$orderby`. So the words, phrases
//! and `from:`/`to:`/`subject:`/`body:` terms go to the server as KQL, and
//! everything else (dates, folders, accounts, attachments, read state) is
//! applied here to what comes back ([`refine`]).
//!
//! The daemon makes the call (it holds the tokens and the Graph scheduler);
//! the command merges the answer with the local page and labels each server
//! result as online.

use std::collections::HashSet;

use familiar_store::{AccountId, FolderId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::query::{Query, TextField};

/// Most results asked of the server per account.
pub const MAX_ONLINE: usize = 1000;

/// A message the server found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OnlineHit {
    /// The account's name in config.toml.
    pub account: String,
    /// Graph immutable ID.
    pub remote_id: String,
    /// Local message ID, when its header is on this device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_id: Option<i64>,
    /// Local folder ID, when the folder is on this device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<i64>,
    /// Subject.
    pub subject: String,
    /// Sender name.
    pub from_name: String,
    /// Sender address.
    pub from_addr: String,
    /// Received time (Unix seconds).
    pub received_at: i64,
    /// Read state.
    pub is_read: bool,
    /// Flagged state.
    pub is_flagged: bool,
    /// Has file attachments.
    pub has_attachments: bool,
    /// Graph's text preview.
    pub preview: String,
    /// Whether the body is on this device.
    pub body_local: bool,
}

/// The query as Graph `$search` KQL, wrapped in the double quotes Graph
/// requires, or `None` if it has no words or phrases (then the server has
/// nothing to add: every header is local). Exclusions are left out; Graph's
/// KQL `NOT` isn't reliable with `$search` on messages.
pub fn to_kql(q: &Query) -> Option<String> {
    let mut parts = Vec::new();
    for t in &q.text {
        let text = t.text.replace(['"', '\\'], " ");
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let value = if text.contains(char::is_whitespace) {
            format!("\\\"{text}\\\"")
        } else {
            text.to_string()
        };
        parts.push(match t.field {
            TextField::Any => value,
            TextField::From => format!("from:{value}"),
            TextField::To => format!("to:{value}"),
            TextField::Subject => format!("subject:{value}"),
            TextField::Body => format!("body:{value}"),
        });
    }
    if parts.is_empty() {
        return None;
    }
    Some(format!("\"{}\"", parts.join(" ")))
}

/// What the query's filters resolved to locally.
#[derive(Debug, Clone, Default)]
pub struct Filters {
    /// `account:`, if given.
    pub accounts: Option<HashSet<AccountId>>,
    /// `-account:`.
    pub not_accounts: HashSet<AccountId>,
    /// `folder:`, if given.
    pub folders: Option<HashSet<FolderId>>,
    /// `-folder:`.
    pub not_folders: HashSet<FolderId>,
}

/// Applies the parts of `q` the server couldn't to its results. `account_of`
/// maps an account name to its local ID.
pub fn refine(
    q: &Query,
    f: &Filters,
    hits: Vec<OnlineHit>,
    account_of: impl Fn(&str) -> Option<AccountId>,
) -> Vec<OnlineHit> {
    hits.into_iter()
        .filter(|h| q.after.is_none_or(|a| h.received_at >= a))
        .filter(|h| q.before.is_none_or(|b| h.received_at < b))
        .filter(|h| q.has_attachment.is_none_or(|a| a == h.has_attachments))
        .filter(|h| q.unread.is_none_or(|u| u != h.is_read))
        .filter(|h| q.flagged.is_none_or(|fl| fl == h.is_flagged))
        .filter(|h| {
            let acc = account_of(&h.account);
            f.accounts
                .as_ref()
                .is_none_or(|s| acc.is_some_and(|a| s.contains(&a)))
                && acc.is_none_or(|a| !f.not_accounts.contains(&a))
        })
        .filter(|h| {
            let folder = h.folder_id.map(FolderId);
            f.folders
                .as_ref()
                .is_none_or(|s| folder.is_some_and(|x| s.contains(&x)))
                && folder.is_none_or(|x| !f.not_folders.contains(&x))
        })
        .collect()
}

/// Percent-encodes a query parameter value.
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(account: &str, at: i64) -> OnlineHit {
        OnlineHit {
            account: account.into(),
            remote_id: format!("r{at}"),
            local_id: None,
            folder_id: Some(1),
            subject: String::new(),
            from_name: String::new(),
            from_addr: String::new(),
            received_at: at,
            is_read: true,
            is_flagged: false,
            has_attachments: false,
            preview: String::new(),
            body_local: false,
        }
    }

    #[test]
    fn kql() {
        let q = Query::parse(
            r#"invoice "annual meeting" from:club subject:"AGM 2026" body:x -spam has:attachment before:2015"#,
            0,
        )
        .unwrap();
        assert_eq!(
            to_kql(&q).unwrap(),
            r#""invoice \"annual meeting\" from:club subject:\"AGM 2026\" body:x""#
        );
        assert_eq!(
            to_kql(&Query::parse("is:unread folder:inbox", 0).unwrap()),
            None
        );
        // Quotes and backslashes can't break out of the value.
        let q = Query::parse(r#"a\"b"#, 0).unwrap();
        assert_eq!(to_kql(&q).unwrap(), r#""\"a b\"""#);
    }

    #[test]
    fn refine_applies_local_filters() {
        let q = Query::parse("x before:1970-01-02 account:home", 0).unwrap();
        let f = Filters {
            accounts: Some([AccountId(1)].into()),
            ..Default::default()
        };
        let acc = |k: &str| (k == "home").then_some(AccountId(1));
        let out = refine(
            &q,
            &f,
            vec![hit("home", 10), hit("work", 10), hit("home", 90_000)],
            acc,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].account, "home");
        assert_eq!(out[0].received_at, 10);
    }

    #[test]
    fn encodes() {
        assert_eq!(url_encode(r#""a b:c""#), "%22a%20b%3Ac%22");
    }
}
