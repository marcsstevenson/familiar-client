// SPDX-License-Identifier: MIT OR Apache-2.0
//! Local search: a [`Query`] run against the index, rows read from SQLite.

use std::collections::{HashMap, HashSet};
use std::ops::Bound;
use std::path::Path;

use familiar_store::{Account, AccountId, Cursor, Folder, FolderId, Message, MessageId, Reader};
use tantivy::collector::TopDocs;
use tantivy::query::{
    AllQuery, BooleanQuery, Occur, PhraseQuery, Query as TQuery, RangeQuery, TermQuery,
    TermSetQuery,
};
use tantivy::schema::{Field, IndexRecordOption};
use tantivy::{Index, IndexReader, Order, ReloadPolicy, Term};

use crate::index::{Fields, open_existing};
use crate::query::{Query, TextField, TextTerm};
use crate::{Error, Result};

/// One page of local results.
#[derive(Debug, Clone, Default)]
pub struct LocalPage {
    /// Matching messages, newest first.
    pub hits: Vec<Message>,
    /// Pass as `after` for the next page; `None` on the last page.
    pub next: Option<Cursor>,
    /// Things the user should know, such as a folder name that matched no
    /// folder.
    pub notices: Vec<String>,
}

/// Searches the index from any process. Cheap to open; open one per
/// command, or keep one and call [`Searcher::reload`] to see new commits.
pub struct Searcher {
    index: Index,
    reader: IndexReader,
    f: Fields,
}

impl std::fmt::Debug for Searcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Searcher").finish_non_exhaustive()
    }
}

/// Folders and accounts named in a query, as local IDs.
struct Scope {
    accounts: Option<Vec<AccountId>>,
    not_accounts: Vec<AccountId>,
    folders: Option<Vec<FolderId>>,
    not_folders: Vec<FolderId>,
    notices: Vec<String>,
}

/// Well-known folder names people type, and what Graph calls them.
const FOLDER_ALIASES: &[(&str, &str)] = &[
    ("inbox", "inbox"),
    ("sent", "sentitems"),
    ("sentitems", "sentitems"),
    ("sent items", "sentitems"),
    ("drafts", "drafts"),
    ("deleted", "deleteditems"),
    ("deleteditems", "deleteditems"),
    ("deleted items", "deleteditems"),
    ("trash", "deleteditems"),
    ("bin", "deleteditems"),
    ("junk", "junkemail"),
    ("junkemail", "junkemail"),
    ("spam", "junkemail"),
    ("archive", "archive"),
    ("outbox", "outbox"),
];

/// Whether `name` (as typed in `folder:`) names `f`, whose full path from
/// the top is `path`.
fn folder_matches(name: &str, f: &Folder, path: &str) -> bool {
    let n = name.trim().trim_matches('/').to_lowercase();
    if f.name.to_lowercase() == n || path.to_lowercase() == n {
        return true;
    }
    let wk = f.well_known.as_deref().unwrap_or("");
    !wk.is_empty()
        && FOLDER_ALIASES
            .iter()
            .any(|(alias, w)| *alias == n && *w == wk)
}

fn account_matches(name: &str, a: &Account) -> bool {
    a.key.eq_ignore_ascii_case(name) || a.email.eq_ignore_ascii_case(name)
}

/// Every folder with its full path (`Parent/Child`).
pub(crate) fn folders_with_paths(
    r: &Reader,
    accounts: &[Account],
) -> Result<Vec<(Folder, String)>> {
    let mut out = Vec::new();
    for a in accounts {
        let folders = r.folders(a.id)?;
        let by_id: HashMap<FolderId, &Folder> = folders.iter().map(|f| (f.id, f)).collect();
        for f in &folders {
            let mut parts = vec![f.name.as_str()];
            let mut cur = f.parent;
            // Guard against a cycle in bad data.
            for _ in 0..64 {
                let Some(p) = cur.and_then(|id| by_id.get(&id)) else {
                    break;
                };
                parts.push(p.name.as_str());
                cur = p.parent;
            }
            parts.reverse();
            out.push((f.clone(), parts.join("/")));
        }
    }
    Ok(out)
}

fn resolve_scope(r: &Reader, q: &Query) -> Result<Scope> {
    let accounts = r.accounts()?;
    let mut s = Scope {
        accounts: None,
        not_accounts: Vec::new(),
        folders: None,
        not_folders: Vec::new(),
        notices: Vec::new(),
    };
    let pick_accounts = |names: &[String], notices: &mut Vec<String>| {
        let mut ids = Vec::new();
        for n in names {
            let found: Vec<_> = accounts
                .iter()
                .filter(|a| account_matches(n, a))
                .map(|a| a.id)
                .collect();
            if found.is_empty() {
                notices.push(format!("No account is called {n:?}."));
            }
            ids.extend(found);
        }
        ids
    };
    if !q.accounts.is_empty() {
        s.accounts = Some(pick_accounts(&q.accounts, &mut s.notices));
    }
    s.not_accounts = pick_accounts(&q.not_accounts, &mut s.notices);
    if !q.folders.is_empty() || !q.not_folders.is_empty() {
        let all = folders_with_paths(r, &accounts)?;
        let pick = |names: &[String], notices: &mut Vec<String>| {
            let mut ids = Vec::new();
            for n in names {
                let found: Vec<_> = all
                    .iter()
                    .filter(|(f, p)| folder_matches(n, f, p))
                    .map(|(f, _)| f.id)
                    .collect();
                if found.is_empty() {
                    notices.push(format!("No folder is called {n:?}."));
                }
                ids.extend(found);
            }
            ids
        };
        if !q.folders.is_empty() {
            s.folders = Some(pick(&q.folders, &mut s.notices));
        }
        s.not_folders = pick(&q.not_folders, &mut s.notices);
    }
    Ok(s)
}

fn u64_set(field: Field, ids: impl IntoIterator<Item = i64>) -> Box<dyn TQuery> {
    Box::new(TermSetQuery::new(
        ids.into_iter()
            .map(|i| Term::from_field_u64(field, i as u64)),
    ))
}

impl Searcher {
    /// Opens the index in `dir`. [`Error::NotBuilt`] if `famd` hasn't
    /// created it yet.
    pub fn open(dir: &Path) -> Result<Self> {
        let (index, f) = open_existing(dir)?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;
        Ok(Self { index, reader, f })
    }

    /// Picks up commits made since the searcher opened.
    pub fn reload(&self) -> Result<()> {
        Ok(self.reader.reload()?)
    }

    fn tokens(&self, text: &str) -> Result<Vec<String>> {
        let mut a = self.index.tokenizer_for_field(self.f.body)?;
        let mut out = Vec::new();
        let mut s = a.token_stream(text);
        while s.advance() {
            out.push(s.token().text.clone());
        }
        Ok(out)
    }

    fn fields_for(&self, t: TextField) -> Vec<Field> {
        let f = &self.f;
        match t {
            TextField::Any => vec![f.subject, f.from, f.to, f.body],
            TextField::From => vec![f.from],
            TextField::To => vec![f.to],
            TextField::Subject => vec![f.subject],
            TextField::Body => vec![f.body],
        }
    }

    /// The index query for one word or phrase, or `None` if it has no
    /// searchable characters (such as `!!!`).
    fn text_query(&self, t: &TextTerm) -> Result<Option<Box<dyn TQuery>>> {
        let tokens = self.tokens(&t.text)?;
        if tokens.is_empty() {
            return Ok(None);
        }
        let mut per_field: Vec<(Occur, Box<dyn TQuery>)> = Vec::new();
        for field in self.fields_for(t.field) {
            let q: Box<dyn TQuery> = if tokens.len() == 1 {
                Box::new(TermQuery::new(
                    Term::from_field_text(field, &tokens[0]),
                    IndexRecordOption::Basic,
                ))
            } else {
                Box::new(PhraseQuery::new(
                    tokens
                        .iter()
                        .map(|w| Term::from_field_text(field, w))
                        .collect(),
                ))
            };
            per_field.push((Occur::Should, q));
        }
        Ok(Some(if per_field.len() == 1 {
            per_field
                .pop()
                .map(|(_, q)| q)
                .unwrap_or_else(|| Box::new(AllQuery))
        } else {
            Box::new(BooleanQuery::new(per_field))
        }))
    }

    /// The whole query, or `None` if a filter can match nothing (a folder
    /// name that matched no folder).
    fn build(
        &self,
        r: &Reader,
        q: &Query,
        scope: &Scope,
        before: Bound<i64>,
        within: Option<&[MessageId]>,
    ) -> Result<Option<Box<dyn TQuery>>> {
        let f = &self.f;
        let mut must: Vec<(Occur, Box<dyn TQuery>)> = Vec::new();
        for t in &q.text {
            if let Some(tq) = self.text_query(t)? {
                must.push((Occur::Must, tq));
            }
        }
        for t in &q.exclude {
            if let Some(tq) = self.text_query(t)? {
                must.push((Occur::MustNot, tq));
            }
        }
        if let Some(a) = q.has_attachment {
            must.push((
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_u64(f.attachment, u64::from(a)),
                    IndexRecordOption::Basic,
                )),
            ));
        }
        let lower = q.after.map_or(Bound::Unbounded, Bound::Included);
        let mut upper = q.before.map_or(Bound::Unbounded, Bound::Excluded);
        // The tighter of `before:` and the page cursor.
        upper = match (upper, before) {
            (Bound::Excluded(a), Bound::Included(b)) if a <= b => Bound::Excluded(a),
            (_, Bound::Included(b)) => Bound::Included(b),
            (u, _) => u,
        };
        if !matches!((lower, upper), (Bound::Unbounded, Bound::Unbounded)) {
            let term = |t: i64| Term::from_field_i64(f.received, t);
            must.push((
                Occur::Must,
                Box::new(RangeQuery::new(lower.map(term), upper.map(term))),
            ));
        }
        if let Some(ids) = &scope.accounts {
            if ids.is_empty() {
                return Ok(None);
            }
            must.push((Occur::Must, u64_set(f.account, ids.iter().map(|i| i.0))));
        }
        if !scope.not_accounts.is_empty() {
            must.push((
                Occur::MustNot,
                u64_set(f.account, scope.not_accounts.iter().map(|i| i.0)),
            ));
        }
        if let Some(ids) = &scope.folders {
            if ids.is_empty() {
                return Ok(None);
            }
            must.push((Occur::Must, u64_set(f.folder, ids.iter().map(|i| i.0))));
        }
        if !scope.not_folders.is_empty() {
            must.push((
                Occur::MustNot,
                u64_set(f.folder, scope.not_folders.iter().map(|i| i.0)),
            ));
        }
        // Read and flag state live in SQLite (they change too often to
        // re-index); the store hands over the matching IDs from small
        // partial indexes.
        if let Some(unread) = q.unread {
            let ids = r.unread_ids()?;
            if unread && ids.is_empty() {
                return Ok(None);
            }
            let occur = if unread { Occur::Must } else { Occur::MustNot };
            must.push((occur, u64_set(f.id, ids.into_iter().map(|i| i.0))));
        }
        if let Some(flagged) = q.flagged {
            let ids = r.flagged_ids()?;
            if flagged && ids.is_empty() {
                return Ok(None);
            }
            let occur = if flagged { Occur::Must } else { Occur::MustNot };
            must.push((occur, u64_set(f.id, ids.into_iter().map(|i| i.0))));
        }
        if let Some(ids) = within {
            must.push((Occur::Must, u64_set(f.id, ids.iter().map(|i| i.0))));
        }
        if !must.iter().any(|(o, _)| *o == Occur::Must) {
            must.push((Occur::Must, Box::new(AllQuery)));
        }
        Ok(Some(Box::new(BooleanQuery::new(must))))
    }

    /// Message IDs of the newest `limit` matches, newest first.
    fn top_ids(&self, query: &dyn TQuery, limit: usize) -> Result<Vec<(i64, MessageId)>> {
        let searcher = self.reader.searcher();
        let top = searcher.search(
            query,
            &TopDocs::with_limit(limit.max(1)).order_by_fast_field::<i64>("received", Order::Desc),
        )?;
        let mut cols = HashMap::new();
        let mut out = Vec::with_capacity(top.len());
        for (received, addr) in top {
            let col = match cols.entry(addr.segment_ord) {
                std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::hash_map::Entry::Vacant(e) => e.insert(
                    searcher
                        .segment_reader(addr.segment_ord)
                        .fast_fields()
                        .u64("id")?,
                ),
            };
            if let Some(id) = col.first(addr.doc_id) {
                out.push((received.unwrap_or(0), MessageId(id as i64)));
            }
        }
        // Equal times come back in index order; settle them by ID so the
        // page cursor is exact.
        out.sort_by(|a, b| b.cmp(a));
        Ok(out)
    }

    /// One page of results for `q`, newest first. `after` is the previous
    /// page's [`LocalPage::next`].
    pub fn search(
        &self,
        r: &Reader,
        q: &Query,
        after: Option<Cursor>,
        limit: usize,
    ) -> Result<LocalPage> {
        let limit = limit.max(1);
        let scope = resolve_scope(r, q)?;
        let mut page = LocalPage {
            notices: scope.notices.clone(),
            ..Default::default()
        };
        let before = after.map_or(Bound::Unbounded, |c| Bound::Included(c.received_at));
        let Some(query) = self.build(r, q, &scope, before, None)? else {
            return Ok(page);
        };
        // Rows on the cursor's own second that were already shown are
        // skipped, so ask for a few more than a page.
        let mut want = limit + 1;
        loop {
            let ids = self.top_ids(query.as_ref(), want)?;
            let exhausted = ids.len() < want;
            let ids: Vec<MessageId> = ids
                .into_iter()
                .filter(|(t, id)| after.is_none_or(|c| *t < c.received_at || id.0 < c.id))
                .map(|(_, id)| id)
                .collect();
            if ids.len() <= limit && !exhausted {
                want *= 4;
                continue;
            }
            let rows = r.messages_by_ids(&ids)?;
            let mut by_id: HashMap<MessageId, Message> =
                rows.into_iter().map(|m| (m.id, m)).collect();
            // A row can be gone (deleted since the index last committed);
            // read and flag state are checked again because they can change
            // between the ID lists and the rows.
            let mut hits: Vec<Message> = ids
                .iter()
                .filter_map(|id| by_id.remove(id))
                .filter(|m| q.unread.is_none_or(|u| u != m.is_read))
                .filter(|m| q.flagged.is_none_or(|fl| fl == m.is_flagged))
                .collect();
            let more = hits.len() > limit;
            hits.truncate(limit);
            page.next = more.then(|| {
                let last = &hits[hits.len() - 1];
                Cursor {
                    received_at: last.received_at,
                    id: last.id.0,
                }
            });
            page.hits = hits;
            return Ok(page);
        }
    }

    /// Which of `ids` the local index matches for `q`. Used to tell server
    /// results the local search would find anyway from those only the
    /// server found.
    pub fn matching(&self, r: &Reader, q: &Query, ids: &[MessageId]) -> Result<HashSet<MessageId>> {
        if ids.is_empty() {
            return Ok(HashSet::new());
        }
        let scope = resolve_scope(r, q)?;
        let Some(query) = self.build(r, q, &scope, Bound::Unbounded, Some(ids))? else {
            return Ok(HashSet::new());
        };
        Ok(self
            .top_ids(query.as_ref(), ids.len())?
            .into_iter()
            .map(|(_, id)| id)
            .collect())
    }

    /// Messages in the index.
    pub fn num_docs(&self) -> u64 {
        self.reader.searcher().num_docs()
    }
}

/// Resolves a folder name from `folder:` to local folders, for refining
/// server results. Exposed for the merge step.
pub fn folder_ids_named(r: &Reader, names: &[String]) -> Result<HashSet<FolderId>> {
    let accounts = r.accounts()?;
    let all = folders_with_paths(r, &accounts)?;
    Ok(all
        .iter()
        .filter(|(f, p)| names.iter().any(|n| folder_matches(n, f, p)))
        .map(|(f, _)| f.id)
        .collect())
}

/// Resolves account names from `account:` to local accounts.
pub fn account_ids_named(r: &Reader, names: &[String]) -> Result<HashSet<AccountId>> {
    Ok(r.accounts()?
        .iter()
        .filter(|a| names.iter().any(|n| account_matches(n, a)))
        .map(|a| a.id)
        .collect())
}

impl From<familiar_store::Error> for Error {
    fn from(e: familiar_store::Error) -> Self {
        Error::Store(e)
    }
}
