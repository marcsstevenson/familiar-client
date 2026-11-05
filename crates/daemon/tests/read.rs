// SPDX-License-Identifier: MIT OR Apache-2.0
//! WP 1.10 through a running famd: `fam mail fetch` downloads a body that
//! isn't on this device into the on-demand cache, and `fam api` GETs Graph
//! with the account's token. The store-only paths of the read commands are
//! tested through the `fam` binary in `familiar-cli` (`tests/mail.rs`).

mod common;

use std::time::Duration;

use common::{Env, fast_policy};
use familiar_core::ErrorKind;
use familiar_core::commands::api::{ApiInput, api_at};
use familiar_core::commands::mail::{
    BodyFormat, BodyType, MailFetchInput, MailListInput, fetch_at, list_at,
};
use familiar_store::MessageId;

const LIMIT: Duration = Duration::from_secs(60);

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.unwrap()
}

fn api(path: &str, account: Option<&str>) -> ApiInput {
    ApiInput {
        path: path.into(),
        account: account.map(str::to_string),
        method: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fetch_downloads_and_api_gets() {
    let mut env = Env::new(300).await;
    env.start(fast_policy());
    env.until("first sync", LIMIT, |e| e.settled(30)).await;

    // An Inbox message older than the window has no body yet.
    let paths = env.paths.clone();
    let old = {
        let r = env.reader();
        let acc = env.account();
        let since = env.window_start(30);
        let mut found = None;
        for f in r.folders(acc).unwrap() {
            if let Some(m) = r
                .messages(f.id, None, 1000)
                .unwrap()
                .into_iter()
                .find(|m| m.received_at < since && m.body.is_none())
            {
                found = Some(m);
                break;
            }
        }
        found.expect("an old message without a body")
    };

    // --local leaves it alone.
    let p = paths.clone();
    let id = old.id.0;
    let local = blocking(move || {
        fetch_at(
            &p,
            &MailFetchInput {
                message: id,
                format: BodyFormat::Text,
                local: true,
            },
        )
        .unwrap()
    })
    .await;
    assert!(local.body.is_none());
    assert!(!local.downloaded);

    // Without it famd downloads the body into the cache.
    let p = paths.clone();
    let got = blocking(move || {
        fetch_at(
            &p,
            &MailFetchInput {
                message: id,
                format: BodyFormat::Html,
                local: false,
            },
        )
        .unwrap()
    })
    .await;
    assert!(got.downloaded, "{got:?}");
    assert!(got.cached, "outside the window, so cached");
    assert!(got.header.body_local);
    assert!(got.body.as_deref().is_some_and(|b| !b.is_empty()));
    let stored = env.reader().message(MessageId(id)).unwrap().unwrap();
    assert!(stored.body.is_some());
    assert_eq!(
        got.body_type,
        Some(if stored.body.unwrap().is_html {
            BodyType::Html
        } else {
            BodyType::Text
        })
    );

    // A second fetch reads it locally.
    let p = paths.clone();
    let again = blocking(move || {
        fetch_at(
            &p,
            &MailFetchInput {
                message: id,
                format: BodyFormat::Text,
                local: false,
            },
        )
        .unwrap()
    })
    .await;
    assert!(!again.downloaded);
    assert_eq!(again.body_type, Some(BodyType::Text));

    // mail list sees the synced Inbox with the daemon running.
    let p = paths.clone();
    let list = blocking(move || list_at(&p, &MailListInput::default()).unwrap()).await;
    assert_eq!(list.folders.len(), 1);
    assert_eq!(list.folders[0].folder, "Inbox");
    assert!(!list.messages.is_empty());

    // fam api: a GET with the only account's token, Graph's JSON unchanged.
    let p = paths.clone();
    let folders =
        blocking(move || api_at(&p, &api("/me/mailFolders?$top=100", None)).unwrap()).await;
    let names: Vec<&str> = folders["value"]
        .as_array()
        .expect("Graph's value array")
        .iter()
        .filter_map(|f| f["displayName"].as_str())
        .collect();
    assert!(names.contains(&"Inbox"), "{names:?}");
    let p = paths.clone();
    let me = blocking(move || {
        api_at(
            &p,
            &api(
                &format!("{}/me", "https://graph.microsoft.com/v1.0"),
                Some(common::KEY),
            ),
        )
    })
    .await;
    // The mock serves its own origin, so a graph.microsoft.com link is refused
    // by the client (it never follows links to another host).
    assert!(me.is_err());
    let p = paths.clone();
    let me = blocking(move || api_at(&p, &api("/me", Some(common::KEY))).unwrap()).await;
    assert!(
        me["mail"].is_string() || me["userPrincipalName"].is_string(),
        "{me}"
    );

    // Errors keep their kinds.
    let p = paths.clone();
    let e = blocking(move || api_at(&p, &api("/me", Some("nobody"))).unwrap_err()).await;
    assert_eq!(e.kind, ErrorKind::NotFound, "{e:?}");
    let p = paths.clone();
    let e =
        blocking(move || api_at(&p, &api("/me/messages/no-such-message", None)).unwrap_err()).await;
    assert_eq!(e.kind, ErrorKind::NotFound, "{e:?}");
    let p = paths.clone();
    let e = blocking(move || {
        api_at(
            &p,
            &ApiInput {
                path: "/me/messages".into(),
                account: None,
                method: Some("POST".into()),
            },
        )
        .unwrap_err()
    })
    .await;
    assert_eq!(e.kind, ErrorKind::PermissionDenied);

    env.stop().await;
}
