// SPDX-License-Identifier: MIT OR Apache-2.0
//! Test harness: a mock Graph, a store in a temp dir and a MailSync.
#![allow(dead_code, missing_docs)]

use std::sync::Arc;

use familiar_mock_graph::MockGraph;
use familiar_provider_graph::{
    ClientOptions, FolderTreeReport, GraphClient, MailApi, MailSync, MessageSyncReport, Priority,
    StaticToken,
};
use familiar_store::{AccountId, Folder, Message, NewAccount, Reader, Writer};

pub struct Env {
    pub mock: Arc<MockGraph>,
    pub dir: tempfile::TempDir,
    pub w: Writer,
    pub acc: AccountId,
    pub sync: MailSync,
}

pub fn client(mock: &MockGraph, token: &str) -> GraphClient {
    GraphClient::new(
        ClientOptions::with_base(mock.base_url()),
        Arc::new(StaticToken(token.to_string())),
    )
    .unwrap()
}

impl Env {
    pub fn new(mock: MockGraph, token: &str) -> Self {
        let c = client(&mock, token);
        Self::with_client(mock, c)
    }

    pub fn with_client(mock: MockGraph, client: GraphClient) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut w = Writer::open(dir.path()).unwrap();
        let acc = w
            .write(|tx| {
                tx.upsert_account(&NewAccount {
                    key: "default".into(),
                    email: "pat@example.com".into(),
                    ..Default::default()
                })
            })
            .unwrap();
        let sync = MailSync::new(MailApi::new(client), acc);
        Self {
            mock: Arc::new(mock),
            dir,
            w,
            acc,
            sync,
        }
    }

    pub fn page_size(mut self, n: usize) -> Self {
        self.sync = self.sync.with_page_size(n);
        self
    }

    pub fn reader(&self) -> Reader {
        Reader::open(self.dir.path()).unwrap()
    }

    pub async fn sync_tree(&mut self) -> FolderTreeReport {
        self.sync
            .sync_folder_tree(&mut self.w, Priority::Background)
            .await
            .unwrap()
    }

    pub async fn sync_folders(&mut self, folders: &[&str]) -> MessageSyncReport {
        self.sync
            .sync_folders(&mut self.w, folders, Priority::Background)
            .await
            .unwrap()
    }

    pub async fn sync_all(&mut self) -> MessageSyncReport {
        let ids: Vec<String> = self.folders().into_iter().map(|f| f.remote_id).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        self.sync_folders(&refs).await
    }

    pub fn folders(&self) -> Vec<Folder> {
        self.reader().folders(self.acc).unwrap()
    }

    pub fn folder(&self, remote: &str) -> Folder {
        self.reader()
            .folder_by_remote(self.acc, remote)
            .unwrap()
            .unwrap()
    }

    pub fn local(&self, remote: &str) -> Option<Message> {
        self.reader().message_by_remote(self.acc, remote).unwrap()
    }

    pub fn remote_ids(&self, folder: &str) -> Vec<String> {
        let f = self.folder(folder);
        self.reader()
            .messages(f.id, None, 100_000)
            .unwrap()
            .into_iter()
            .map(|m| m.remote_id)
            .collect()
    }

    pub fn state(&self, resource: &str) -> Option<String> {
        self.reader()
            .sync_state(self.acc, resource)
            .unwrap()
            .and_then(|s| s.delta_link)
    }

    pub async fn fetch_body(&mut self, remote: &str) {
        let id = self.local(remote).unwrap().id;
        self.sync
            .fetch_body(&mut self.w, id, Priority::User)
            .await
            .unwrap();
    }

    pub fn body_text(&self, m: &Message) -> String {
        let b = m.body.expect("message has a body");
        String::from_utf8(self.reader().body(&b.hash).unwrap()).unwrap()
    }
}
