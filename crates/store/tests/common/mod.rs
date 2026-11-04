// SPDX-License-Identifier: MIT OR Apache-2.0
//! Helpers shared by the store tests.
#![allow(dead_code)]

use familiar_store::*;

pub fn account(w: &mut Writer, key: &str) -> AccountId {
    w.write(|tx| {
        tx.upsert_account(&NewAccount {
            key: key.into(),
            email: format!("{key}@example.com"),
            ..Default::default()
        })
    })
    .unwrap()
}

pub fn folder(w: &mut Writer, account: AccountId, name: &str) -> FolderId {
    w.write(|tx| {
        tx.upsert_folder(&NewFolder {
            account,
            remote_id: format!("folder-{name}"),
            name: name.into(),
            well_known: Some(name.to_lowercase()),
            ..Default::default()
        })
    })
    .unwrap()
}

pub fn msg(
    account: AccountId,
    folder: FolderId,
    remote: &str,
    conv: &str,
    at: i64,
    read: bool,
) -> NewMessage {
    NewMessage {
        account,
        folder,
        remote_id: remote.into(),
        internet_message_id: Some(format!("<{remote}@example.com>")),
        conversation_id: conv.into(),
        subject: format!("Subject {conv}"),
        from: Address {
            name: "Ann".into(),
            addr: "ann@example.com".into(),
        },
        to: vec![Address {
            name: String::new(),
            addr: "me@example.com".into(),
        }],
        received_at: at,
        is_read: read,
        importance: 1,
        preview: "preview".into(),
        ..Default::default()
    }
}

/// A tiny deterministic generator, so failures reproduce.
pub struct Lcg(pub u64);

impl Lcg {
    pub fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}
