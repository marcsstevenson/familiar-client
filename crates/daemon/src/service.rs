// SPDX-License-Identifier: MIT OR Apache-2.0
//! Answers requests from `fam` and the GUI.
//!
//! Requests that change the store (pin, on-demand body fetch) run here, not
//! in the account's engine, so they are not held up by a long first sync.
//! They share the one writer, a transaction at a time, and the account's
//! Graph scheduler at user priority, which goes ahead of background sync.

use std::collections::HashMap;
use std::sync::Arc;

use familiar_core::daemon::{
    AccountFailure, DaemonStatus, FetchBodyParams, FetchBodyResult, METHOD_FETCH_BODY, METHOD_PIN,
    METHOD_SEARCH_ONLINE, METHOD_STATUS, METHOD_STOP, METHOD_SYNC_NOW, PinParams, PinResult,
    SearchOnlineParams, SearchOnlineResult, SyncNowParams,
};
use familiar_core::{CommandError, ErrorKind};
use familiar_provider_graph::{Error as GraphError, MailApi, Priority};
use familiar_search::OnlineHit;
use familiar_store::{AccountId, BodyRef, FolderId, Message, MessageId, Reader, SharedWriter};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::engine::AccountShared;
use crate::policy::{Clock, SyncPolicy};

/// One account as the request handlers see it.
pub struct AccountHandle {
    /// Its key in `config.toml`.
    pub key: String,
    /// Graph reads for on-demand fetches (shares the engine's scheduler).
    pub api: MailApi,
    /// Live status and wake-up.
    pub shared: Arc<AccountShared>,
}

/// Everything a request can touch.
pub struct Service {
    pub(crate) w: SharedWriter,
    pub(crate) accounts: HashMap<AccountId, AccountHandle>,
    pub(crate) policy: Arc<SyncPolicy>,
    pub(crate) clock: Clock,
    pub(crate) started_at: i64,
    pub(crate) shutdown: watch::Sender<bool>,
}

/// Maps a provider error to the command error kind (and exit code) it means.
pub fn command_error(e: GraphError) -> CommandError {
    let kind = match &e {
        GraphError::Network(_) => ErrorKind::Offline,
        GraphError::Auth(_) => ErrorKind::AuthRequired,
        GraphError::Throttled { .. } => ErrorKind::Throttled,
        GraphError::NotFound(_) => ErrorKind::NotFound,
        GraphError::Graph { status: 403, .. } => ErrorKind::PermissionDenied,
        GraphError::Graph { .. } | GraphError::Decode(_) | GraphError::SyncStateGone(_) => {
            ErrorKind::Server
        }
        GraphError::Store(_) => ErrorKind::Internal,
    };
    CommandError::new(kind, e.to_string())
}

fn store_error(e: familiar_store::Error) -> CommandError {
    match e {
        familiar_store::Error::NotFound(m) => CommandError::new(ErrorKind::NotFound, m),
        e => CommandError::internal(format!("store: {e}")),
    }
}

fn params<T: DeserializeOwned>(v: Value) -> Result<T, CommandError> {
    let v = if v.is_null() { json!({}) } else { v };
    serde_json::from_value(v).map_err(|e| CommandError::usage(format!("bad params: {e}")))
}

fn to_value<T: serde::Serialize>(v: &T) -> Result<Value, CommandError> {
    serde_json::to_value(v).map_err(|e| CommandError::internal(e.to_string()))
}

impl Service {
    /// Runs one request.
    pub async fn handle(&self, method: &str, p: Value) -> Result<Value, CommandError> {
        match method {
            METHOD_STATUS => to_value(&self.status()),
            METHOD_PIN => to_value(&self.pin(params(p)?).await?),
            METHOD_FETCH_BODY => to_value(&self.fetch_body(params(p)?).await?),
            METHOD_SEARCH_ONLINE => to_value(&self.search_online(params(p)?).await?),
            METHOD_SYNC_NOW => {
                let p: SyncNowParams = params(p)?;
                let mut found = false;
                for a in self.accounts.values() {
                    if p.account.as_deref().is_none_or(|k| k == a.key) {
                        a.shared.sync_now();
                        found = true;
                    }
                }
                if !found {
                    return Err(CommandError::new(
                        ErrorKind::NotFound,
                        format!("famd is not syncing an account {:?}", p.account),
                    ));
                }
                Ok(json!({}))
            }
            METHOD_STOP => {
                let _ = self.shutdown.send(true);
                Ok(json!({}))
            }
            m => Err(CommandError::usage(format!("famd has no method {m:?}"))),
        }
    }

    /// `daemon.status`.
    pub fn status(&self) -> DaemonStatus {
        let mut accounts: Vec<_> = self
            .accounts
            .values()
            .map(|a| a.shared.snapshot())
            .collect();
        accounts.sort_by(|a, b| a.account.cmp(&b.account));
        DaemonStatus {
            version: env!("CARGO_PKG_VERSION").to_string(),
            pid: std::process::id(),
            started_at: self.started_at,
            accounts,
        }
    }

    fn message(&self, id: i64) -> Result<(Message, &AccountHandle), CommandError> {
        let r = Reader::open(&self.w.paths().root).map_err(store_error)?;
        let m = r
            .message(MessageId(id))
            .map_err(store_error)?
            .ok_or_else(|| CommandError::new(ErrorKind::NotFound, format!("message {id}")))?;
        let acc = self.accounts.get(&m.account).ok_or_else(|| {
            CommandError::new(
                ErrorKind::NotFound,
                format!("message {id} belongs to an account famd isn't syncing"),
            )
        })?;
        Ok((m, acc))
    }

    async fn download(&self, acc: &AccountHandle, m: &Message) -> Result<BodyRef, CommandError> {
        let body = acc
            .api
            .body(&m.remote_id, Priority::User)
            .await
            .map_err(command_error)?;
        let is_html = body.is_html();
        let id = m.id;
        self.w
            .write(|tx| {
                let hash = tx.put_body(body.content.as_bytes())?;
                let r = BodyRef { hash, is_html };
                tx.set_body(id, Some(r))?;
                Ok(r)
            })
            .map_err(store_error)
    }

    fn outside_window(&self, m: &Message) -> bool {
        m.received_at < self.policy.window_start((self.clock)())
    }

    /// `mail.pin`.
    pub async fn pin(&self, p: PinParams) -> Result<PinResult, CommandError> {
        let (m, acc) = self.message(p.message)?;
        let id = m.id;
        let mut fetched = false;
        if p.pinned {
            if m.body.is_none() {
                self.download(acc, &m).await?;
                fetched = true;
            }
            self.w
                .write(|tx| {
                    tx.set_body_pinned(id, true)?;
                    tx.mark_body_cached(id, None)
                })
                .map_err(store_error)?;
        } else {
            // An unpinned body outside the window joins the cache rather
            // than vanishing at once.
            let cache_at = (m.body.is_some() && self.outside_window(&m)).then(|| (self.clock)());
            self.w
                .write(|tx| {
                    tx.set_body_pinned(id, false)?;
                    if cache_at.is_some() {
                        tx.mark_body_cached(id, cache_at)?;
                    }
                    Ok(())
                })
                .map_err(store_error)?;
            self.trim_cache()?;
        }
        Ok(PinResult {
            message: p.message,
            pinned: p.pinned,
            has_body: fetched || m.body.is_some(),
            fetched,
        })
    }

    /// `mail.fetch_body`: a body opened outside the window goes into the
    /// size-limited cache.
    pub async fn fetch_body(&self, p: FetchBodyParams) -> Result<FetchBodyResult, CommandError> {
        let (m, acc) = self.message(p.message)?;
        let id = m.id;
        let cacheable = !m.body_pinned && self.outside_window(&m);
        let now = (self.clock)();
        if m.body.is_some() {
            if m.body_cached_at.is_some() {
                // Used again: the cache evicts least recently used first.
                self.w
                    .write(|tx| tx.mark_body_cached(id, Some(now)))
                    .map_err(store_error)?;
            }
            return Ok(FetchBodyResult {
                message: p.message,
                fetched: false,
                cached: m.body_cached_at.is_some(),
            });
        }
        if cacheable && self.policy.older_bodies == familiar_config::model::OlderBodies::None {
            return Err(CommandError::new(
                ErrorKind::PermissionDenied,
                "sync.older_bodies is \"none\": bodies older than the window are not downloaded",
            ));
        }
        self.download(acc, &m).await?;
        if cacheable {
            self.w
                .write(|tx| tx.mark_body_cached(id, Some(now)))
                .map_err(store_error)?;
            self.trim_cache()?;
        }
        Ok(FetchBodyResult {
            message: p.message,
            fetched: true,
            cached: cacheable,
        })
    }

    fn trim_cache(&self) -> Result<(), CommandError> {
        let limit = i64::try_from(self.policy.cache_limit).unwrap_or(i64::MAX);
        self.w
            .write(|tx| tx.evict_cached_bodies(limit))
            .map_err(store_error)?;
        Ok(())
    }

    /// `mail.search_online`: Graph `$search` on each account (or those
    /// named), at user priority. Results are matched to local headers where
    /// they exist; nothing is written. A failure in one account is reported
    /// for that account and the others still answer.
    pub async fn search_online(
        &self,
        p: SearchOnlineParams,
    ) -> Result<SearchOnlineResult, CommandError> {
        let r = Reader::open(&self.w.paths().root).map_err(store_error)?;
        let mut targets: Vec<(AccountId, &AccountHandle, String)> = Vec::new();
        for acc in r.accounts().map_err(store_error)? {
            let Some(h) = self.accounts.get(&acc.id) else {
                continue;
            };
            let named = p.accounts.is_empty()
                || p.accounts.iter().any(|n| {
                    n.eq_ignore_ascii_case(&acc.key) || n.eq_ignore_ascii_case(&acc.email)
                });
            if named {
                targets.push((acc.id, h, acc.key));
            }
        }
        targets.sort_by(|a, b| a.2.cmp(&b.2));
        let limit = p.limit.clamp(1, familiar_search::online::MAX_ONLINE);
        // One account after another: there are only a few, and each search
        // is one or two requests.
        let mut answers = Vec::new();
        for (id, h, key) in &targets {
            answers.push(
                search_account(&h.api, &p.kql, limit)
                    .await
                    .map(|found| (*id, key.clone(), found)),
            );
        }
        let mut out = SearchOnlineResult::default();
        for (i, a) in answers.into_iter().enumerate() {
            let key = targets[i].2.clone();
            match a {
                Ok((account, key, msgs)) => {
                    out.searched.push(key.clone());
                    for g in msgs {
                        let n = g.to_new_message(account, FolderId(0));
                        let local = r.message_by_remote(account, &g.id).map_err(store_error)?;
                        let folder = match &g.parent_folder_id {
                            Some(f) => r
                                .folder_by_remote(account, f)
                                .map_err(store_error)?
                                .map(|f| f.id.0),
                            None => None,
                        }
                        .or(local.as_ref().map(|m| m.folder.0));
                        out.hits.push(OnlineHit {
                            account: key.clone(),
                            remote_id: g.id.clone(),
                            local_id: local.as_ref().map(|m| m.id.0),
                            folder_id: folder,
                            subject: n.subject,
                            from_name: n.from.name,
                            from_addr: n.from.addr,
                            received_at: n.received_at,
                            is_read: n.is_read,
                            is_flagged: n.is_flagged,
                            has_attachments: n.has_attachments,
                            preview: n.preview,
                            body_local: local.as_ref().is_some_and(|m| m.body.is_some()),
                        });
                    }
                }
                Err(e) => {
                    let e = command_error(e);
                    out.failures.push(AccountFailure {
                        account: key,
                        kind: e.kind,
                        message: e.message,
                    });
                }
            }
        }
        out.hits
            .sort_by(|a, b| (b.received_at, &b.remote_id).cmp(&(a.received_at, &a.remote_id)));
        Ok(out)
    }
}

/// Up to `limit` server results for one mailbox, following next links.
async fn search_account(
    api: &MailApi,
    kql: &str,
    limit: usize,
) -> Result<Vec<familiar_provider_graph::GraphMessage>, GraphError> {
    let mut found = Vec::new();
    let mut link: Option<String> = None;
    loop {
        let page = api
            .search_page(kql, limit.min(250), link.as_deref(), Priority::User)
            .await?;
        found.extend(page.items);
        match page.next_link {
            Some(n) if found.len() < limit => link = Some(n),
            _ => break,
        }
    }
    found.truncate(limit);
    Ok(found)
}
