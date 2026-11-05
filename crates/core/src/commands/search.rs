// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam mail search` (WP 1.9): instant local search, plus server search for
//! older message text.
//!
//! Local search reads the index `famd` keeps beside the store and the rows
//! from SQLite, so it works with or without the daemon, offline included.
//! Bodies are only on this device for the sync window, so with `--scope all`
//! the words of the query also go to the server (Graph `$search`, through
//! `famd`, which holds the tokens). Server results that the local index
//! would not find are merged in and labelled `online`. When the server can't
//! be reached, the results say that older message text wasn't searched.

use std::collections::{HashMap, HashSet};

use crate::commands::auth::{config, rfc3339};
use crate::daemon::{
    Client, DaemonPaths, METHOD_SEARCH_ONLINE, SearchOnlineParams, SearchOnlineResult,
};
use crate::error::{CommandError, ErrorKind};
use crate::registry::{Access, Command, CommandSpec, Context};
use familiar_search::online::{Filters, MAX_ONLINE, refine};
use familiar_search::{
    LocalPage, OnlineHit, Query, Searcher, account_ids_named, folder_ids_named, to_kql,
};
use familiar_store::{AccountId, Cursor, FolderId, Message, MessageId, Reader};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `fam mail search`.
pub struct MailSearch;

/// Where to search.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchScope {
    /// This device only: every header, and message text for the sync window.
    #[default]
    Local,
    /// This device, plus message text on the server for older mail.
    All,
}

/// Input for `fam mail search`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MailSearchInput {
    /// The query, for example 'from:club subject:"AGM" has:attachment after:2026-01-01'.
    /// Operators: from: to: subject: body: has:attachment before: after: folder: account:
    /// is:unread is:read is:flagged, "phrases" and -exclusions.
    #[schemars(extend("x-positional" = 0))]
    pub query: String,
    /// local (default) searches this device; all also searches message text on the server.
    #[serde(default)]
    pub scope: SearchScope,
    /// Most results per page (default 50, at most 500).
    #[serde(default)]
    pub limit: Option<u32>,
    /// The next-page token from a previous search.
    #[serde(default)]
    pub after: Option<String>,
}

/// Where a result came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HitSource {
    /// Found on this device.
    Local,
    /// Found only by the server: its text isn't on this device.
    Online,
}

/// One result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchHit {
    /// Local message ID, when its header is on this device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    /// `local`, or `online` for a result only the server found.
    pub source: HitSource,
    /// Account name.
    pub account: String,
    /// Folder name, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// Subject.
    pub subject: String,
    /// Sender name.
    pub from_name: String,
    /// Sender address.
    pub from_addr: String,
    /// Received time (RFC 3339, UTC).
    pub received_at: String,
    /// Read state.
    pub is_read: bool,
    /// Flagged state.
    pub is_flagged: bool,
    /// Has file attachments.
    pub has_attachments: bool,
    /// Short text preview.
    pub preview: String,
    /// Whether the body is on this device.
    pub body_local: bool,
    /// Graph immutable ID.
    pub remote_id: String,
}

/// What happened to the server search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OnlineStatus {
    /// `--scope local`, or a later page.
    NotRequested,
    /// The query has no words to search for; every header is local.
    NotNeeded,
    /// The server was searched (for every account, or see `notices`).
    Searched,
    /// The server couldn't be reached: older message text wasn't searched.
    Offline,
    /// famd isn't running, so the server wasn't searched.
    DaemonUnavailable,
    /// The server search failed for every account; see `notices`.
    Failed,
}

/// Output of `fam mail search`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MailSearchOutput {
    /// The query as given.
    pub query: String,
    /// Results, newest first. Server-only results are labelled `online`.
    pub hits: Vec<SearchHit>,
    /// How many results are `online`.
    pub online_hits: usize,
    /// What happened to the server search.
    pub online: OnlineStatus,
    /// Pass as `--after` for the next page of local results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    /// Things to know about these results, such as text that wasn't searched.
    pub notices: Vec<String>,
}

const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 500;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn store_err(e: familiar_store::Error) -> CommandError {
    CommandError::internal(format!("store: {e}"))
}

fn search_err(e: familiar_search::Error) -> CommandError {
    match e {
        familiar_search::Error::Query(q) => CommandError::usage(q.to_string()),
        e => CommandError::internal(e.to_string()),
    }
}

fn parse_cursor(s: &str) -> Result<Cursor, CommandError> {
    let bad = || CommandError::usage(format!("--after {s:?} isn't a token from a search"));
    let (t, id) = s.split_once('.').ok_or_else(bad)?;
    Ok(Cursor {
        received_at: t.parse().map_err(|_| bad())?,
        id: id.parse().map_err(|_| bad())?,
    })
}

fn cursor_token(c: &Cursor) -> String {
    format!("{}.{}", c.received_at, c.id)
}

fn time(secs: i64) -> String {
    rfc3339(secs.max(0) as u64)
}

/// Account and folder names for results.
struct Names {
    accounts: HashMap<AccountId, String>,
    keys: HashMap<String, AccountId>,
    folders: HashMap<FolderId, String>,
}

impl Names {
    fn load(r: &Reader) -> familiar_store::Result<Self> {
        let mut n = Names {
            accounts: HashMap::new(),
            keys: HashMap::new(),
            folders: HashMap::new(),
        };
        for a in r.accounts()? {
            for f in r.folders(a.id)? {
                n.folders.insert(f.id, f.name);
            }
            n.keys.insert(a.key.clone(), a.id);
            n.accounts.insert(a.id, a.key);
        }
        Ok(n)
    }

    fn local_hit(&self, m: &Message) -> SearchHit {
        SearchHit {
            id: Some(m.id.0),
            source: HitSource::Local,
            account: self.accounts.get(&m.account).cloned().unwrap_or_default(),
            folder: self.folders.get(&m.folder).cloned(),
            subject: m.subject.clone(),
            from_name: m.from.name.clone(),
            from_addr: m.from.addr.clone(),
            received_at: time(m.received_at),
            is_read: m.is_read,
            is_flagged: m.is_flagged,
            has_attachments: m.has_attachments,
            preview: m.preview.clone(),
            body_local: m.body.is_some(),
            remote_id: m.remote_id.clone(),
        }
    }

    fn online_hit(&self, h: &OnlineHit) -> SearchHit {
        SearchHit {
            id: h.local_id,
            source: HitSource::Online,
            account: h.account.clone(),
            folder: h
                .folder_id
                .and_then(|f| self.folders.get(&FolderId(f)).cloned()),
            subject: h.subject.clone(),
            from_name: h.from_name.clone(),
            from_addr: h.from_addr.clone(),
            received_at: time(h.received_at),
            is_read: h.is_read,
            is_flagged: h.is_flagged,
            has_attachments: h.has_attachments,
            preview: h.preview.clone(),
            body_local: h.body_local,
            remote_id: h.remote_id.clone(),
        }
    }
}

/// Runs a search against the store, index and daemon at `paths`. `window`
/// is `sync.window` from config.toml (for the notices) and `now` anchors
/// relative dates. Used by the command and by tests.
pub fn search_at(
    paths: &DaemonPaths,
    input: &MailSearchInput,
    window: &str,
    now: i64,
) -> Result<MailSearchOutput, CommandError> {
    let q = Query::parse(&input.query, now).map_err(|e| CommandError::usage(e.to_string()))?;
    let limit = input
        .limit
        .map_or(DEFAULT_LIMIT, |l| l as usize)
        .clamp(1, MAX_LIMIT);
    let after = input.after.as_deref().map(parse_cursor).transpose()?;
    let mut out = MailSearchOutput {
        query: input.query.clone(),
        hits: Vec::new(),
        online_hits: 0,
        online: OnlineStatus::NotRequested,
        next: None,
        notices: Vec::new(),
    };

    let r = match Reader::open(&paths.store) {
        Ok(r) => r,
        Err(familiar_store::Error::NoStore(_)) => {
            out.notices
                .push("No mail has synced to this device yet; start famd.".into());
            return Ok(out);
        }
        Err(e) => return Err(store_err(e)),
    };
    let searcher = match Searcher::open(&paths.index) {
        Ok(s) => Some(s),
        Err(familiar_search::Error::NotBuilt(_)) => {
            out.notices
                .push("The search index isn't built yet; famd builds it when it starts.".into());
            None
        }
        Err(e) => return Err(search_err(e)),
    };
    let pending = r.search_queue_len().map_err(store_err)?;
    if searcher.is_some() && pending > 0 {
        out.notices.push(format!(
            "{pending} recent changes are still being indexed, so the newest mail may be missing."
        ));
    }
    let names = Names::load(&r).map_err(store_err)?;

    let page = match &searcher {
        Some(s) => s.search(&r, &q, after, limit).map_err(search_err)?,
        None => LocalPage::default(),
    };
    out.notices.extend(page.notices.iter().cloned());
    out.next = page.next.as_ref().map(cursor_token);
    let mut hits: Vec<(i64, i64, SearchHit)> = page
        .hits
        .iter()
        .map(|m| (m.received_at, m.id.0, names.local_hit(m)))
        .collect();

    let wants_online = input.scope == SearchScope::All && after.is_none();
    if wants_online {
        let online = online_hits(
            paths,
            &q,
            &r,
            searcher.as_ref(),
            &page,
            &names,
            window,
            limit,
            &mut out,
        )?;
        out.online_hits = online.len();
        hits.extend(
            online
                .iter()
                .map(|h| (h.received_at, h.local_id.unwrap_or(0), names.online_hit(h))),
        );
    } else if input.scope == SearchScope::Local && q.searches_body() && page.next.is_none() {
        out.notices.push(format!(
            "Message text was searched for the last {window} only; use --scope all to search older mail on the server."
        ));
    }
    hits.sort_by_key(|h| std::cmp::Reverse((h.0, h.1)));
    out.hits = hits.into_iter().map(|(_, _, h)| h).collect();
    Ok(out)
}

/// Asks famd to search the server and keeps what the local page doesn't
/// already have and the local index wouldn't find on a later page.
#[allow(clippy::too_many_arguments)]
fn online_hits(
    paths: &DaemonPaths,
    q: &Query,
    r: &Reader,
    searcher: Option<&Searcher>,
    page: &LocalPage,
    names: &Names,
    window: &str,
    limit: usize,
    out: &mut MailSearchOutput,
) -> Result<Vec<OnlineHit>, CommandError> {
    let Some(kql) = to_kql(q) else {
        out.online = OnlineStatus::NotNeeded;
        return Ok(Vec::new());
    };
    let not_searched = format!("message text older than the {window} sync window wasn't searched");
    let answer: SearchOnlineResult = match Client::connect(&paths.socket).and_then(|mut c| {
        c.call(
            METHOD_SEARCH_ONLINE,
            &SearchOnlineParams {
                kql,
                accounts: q.accounts.clone(),
                limit: limit.min(MAX_ONLINE),
            },
        )
    }) {
        Ok(a) => a,
        Err(e) if e.kind == ErrorKind::DaemonUnavailable => {
            out.online = OnlineStatus::DaemonUnavailable;
            out.notices.push(format!(
                "famd isn't running, so {not_searched}; start famd to search the server."
            ));
            return Ok(Vec::new());
        }
        Err(e) if e.kind == ErrorKind::Offline => {
            out.online = OnlineStatus::Offline;
            out.notices.push(format!("Offline: {not_searched}."));
            return Ok(Vec::new());
        }
        Err(e) => return Err(e),
    };
    let all_failed = answer.searched.is_empty() && !answer.failures.is_empty();
    out.online = if all_failed {
        if answer.failures.iter().all(|f| f.kind == ErrorKind::Offline) {
            out.notices.push(format!("Offline: {not_searched}."));
            OnlineStatus::Offline
        } else {
            OnlineStatus::Failed
        }
    } else {
        OnlineStatus::Searched
    };
    for f in &answer.failures {
        if f.kind == ErrorKind::Offline && out.online == OnlineStatus::Offline {
            continue;
        }
        out.notices.push(format!(
            "The server search failed for {} ({}), so its {not_searched}: {}",
            f.account,
            f.kind.as_str(),
            f.message
        ));
    }

    let filters = Filters {
        accounts: (!q.accounts.is_empty())
            .then(|| account_ids_named(r, &q.accounts))
            .transpose()
            .map_err(search_err)?,
        not_accounts: account_ids_named(r, &q.not_accounts).map_err(search_err)?,
        folders: (!q.folders.is_empty())
            .then(|| folder_ids_named(r, &q.folders))
            .transpose()
            .map_err(search_err)?,
        not_folders: folder_ids_named(r, &q.not_folders).map_err(search_err)?,
    };
    let refined = refine(q, &filters, answer.hits, |k| names.keys.get(k).copied());

    // Drop what local search already shows or would show on a later page.
    let on_page: HashSet<MessageId> = page.hits.iter().map(|m| m.id).collect();
    let local_ids: Vec<MessageId> = refined
        .iter()
        .filter_map(|h| h.local_id.map(MessageId))
        .filter(|id| !on_page.contains(id))
        .collect();
    let found_locally = match searcher {
        Some(s) => s.matching(r, q, &local_ids).map_err(search_err)?,
        None => HashSet::new(),
    };
    let mut seen = HashSet::new();
    Ok(refined
        .into_iter()
        .filter(|h| match h.local_id.map(MessageId) {
            Some(id) => !on_page.contains(&id) && !found_locally.contains(&id),
            None => true,
        })
        .filter(|h| seen.insert((h.account.clone(), h.remote_id.clone())))
        .collect())
}

fn short_date(rfc: &str) -> &str {
    rfc.get(..10).unwrap_or(rfc)
}

fn fit(s: &str, n: usize) -> String {
    let count = s.chars().count();
    if count <= n {
        format!("{s:<n$}")
    } else {
        let mut t: String = s.chars().take(n.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

impl Command for MailSearch {
    type Input = MailSearchInput;
    type Output = MailSearchOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["mail", "search"],
        summary: "Search mail across all accounts",
        description: "Searches every account on this device instantly, offline included: every header, and message text for the sync window. Operators: from: to: subject: body: has:attachment before: after: (YYYY-MM-DD, YYYY-MM, YYYY or an age such as 30d) folder: account: is:unread is:read is:flagged, \"phrases\" and -exclusions. With --scope all the words are also searched on the server (through famd), so older message text is found too; those results are labelled online. Results are newest first; --after takes the next-page token.",
        access: Access::Read,
        errors: &[
            ErrorKind::Usage,
            ErrorKind::Config,
            ErrorKind::Throttled,
            ErrorKind::Server,
        ],
    };

    fn run(
        &self,
        _ctx: &Context<'_>,
        input: MailSearchInput,
    ) -> Result<MailSearchOutput, CommandError> {
        let window = config()?.map_or_else(|| "1y".to_string(), |c| c.sync.window);
        search_at(&DaemonPaths::resolve()?, &input, &window, now())
    }

    fn render_text(&self, o: &MailSearchOutput) -> String {
        let mut s = String::new();
        if o.hits.is_empty() {
            s.push_str(&format!("No results for {:?}.\n", o.query));
        } else {
            s.push_str(&format!(
                "{} result{} for {:?}{}\n",
                o.hits.len(),
                if o.hits.len() == 1 { "" } else { "s" },
                o.query,
                if o.online_hits > 0 {
                    format!(" ({} online)", o.online_hits)
                } else {
                    String::new()
                }
            ));
            for h in &o.hits {
                let from = if h.from_name.is_empty() {
                    &h.from_addr
                } else {
                    &h.from_name
                };
                let id = h.id.map_or_else(|| "-".to_string(), |i| i.to_string());
                let place = match &h.folder {
                    Some(f) => format!("{}/{f}", h.account),
                    None => h.account.clone(),
                };
                s.push_str(&format!(
                    "{:>8}  {}  {} {}  {}{}{}\n",
                    id,
                    short_date(&h.received_at),
                    if h.is_read { ' ' } else { '*' },
                    fit(from, 22),
                    fit(&h.subject, 50),
                    format_args!("  [{place}]"),
                    if h.source == HitSource::Online {
                        "  (online)"
                    } else {
                        ""
                    }
                ));
            }
        }
        for n in &o.notices {
            s.push_str(&format!("Note: {n}\n"));
        }
        if let Some(n) = &o.next {
            s.push_str(&format!("More: --after {n}\n"));
        }
        s.trim_end().to_string()
    }
}
