// SPDX-License-Identifier: MIT OR Apache-2.0
//! Sign-in, refresh, restart and sign-out against the mock identity platform and Graph.

use familiar_auth::{
    Auth, AuthErrorKind, CONSUMER_TENANT, DEFAULT_CLIENT_ID, Endpoints, FileStore, Flow,
    LoginRequest, MemoryStore, Prompt, SCOPES, TokenStore,
};
use familiar_mock_graph::{DEFAULT_TENANT, Mailbox, MockGraph};
use std::sync::Mutex;
use std::time::Duration;

struct Env {
    rt: tokio::runtime::Runtime,
    mock: Option<MockGraph>,
}

impl Env {
    fn start(lifetime: Duration) -> Self {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut hotmail = Mailbox::small();
        hotmail.user.user_principal_name = "marc@hotmail.example".into();
        hotmail.user.mail = "marc@hotmail.example".into();
        let mut parklands = Mailbox::small();
        parklands.user.user_principal_name = "marc@parklands.example".into();
        parklands.user.mail = "marc@parklands.example".into();
        let mock = rt
            .block_on(
                MockGraph::builder()
                    .mailbox("hotmail", hotmail)
                    .tenant("hotmail", CONSUMER_TENANT)
                    .mailbox("parklands", parklands)
                    .token_lifetime(lifetime)
                    .device_interval(0)
                    .start(),
            )
            .unwrap();
        Self {
            rt,
            mock: Some(mock),
        }
    }

    fn mock(&self) -> &MockGraph {
        self.mock.as_ref().unwrap()
    }

    fn endpoints(&self) -> Endpoints {
        Endpoints {
            login: self.mock().login_url(),
            graph: self.mock().base_url().to_string(),
        }
    }

    fn auth(&self, store: Box<dyn TokenStore>) -> Auth {
        Auth::new(store, self.endpoints()).with_browser_timeout(Duration::from_secs(20))
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _guard = self.rt.enter();
        self.mock.take();
    }
}

/// A "browser" that follows the authorize redirect to the loopback port.
#[derive(Default)]
struct FakeBrowser {
    shown: Mutex<Vec<String>>,
}

impl Prompt for FakeBrowser {
    fn show(&self, message: &str) {
        self.shown.lock().unwrap().push(message.to_string());
    }
    fn open_browser(&self, url: &str) -> bool {
        let url = url.to_string();
        std::thread::spawn(move || {
            // ureq follows the 302 to http://localhost:<port>/?code=...
            let _ = ureq::get(&url).call();
        });
        true
    }
}

fn request(account: &str, authority: &str, flow: Flow) -> LoginRequest {
    LoginRequest {
        account: account.into(),
        authority: authority.into(),
        client_id: DEFAULT_CLIENT_ID.into(),
        flow,
        email: None,
        scopes: SCOPES.into(),
    }
}

fn token_paths(mock: &MockGraph) -> Vec<String> {
    mock.requests()
        .into_iter()
        .filter(|r| r.path.ends_with("/oauth2/v2.0/token"))
        .map(|r| format!("{} {}", r.path, r.status))
        .collect()
}

#[test]
fn pkce_sign_in_hotmail_and_use_graph() {
    let env = Env::start(Duration::from_secs(3600));
    env.mock().sign_in_as(Some("hotmail"));
    let auth = env.auth(Box::new(MemoryStore::memory()));
    let browser = FakeBrowser::default();
    let rec = auth
        .login(&request("hotmail", "consumers", Flow::Browser), &browser)
        .unwrap();
    assert_eq!(rec.tenant_id.as_deref(), Some(CONSUMER_TENANT));
    assert_eq!(rec.username.as_deref(), Some("marc@hotmail.example"));
    assert!(!rec.scopes.iter().any(|s| s == "offline_access"));
    assert!(rec.scopes.iter().any(|s| s == "Mail.ReadWrite"));
    assert!(
        browser
            .shown
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("/consumers/oauth2/v2.0/authorize"))
    );
    let p = auth.check("hotmail").unwrap();
    assert_eq!(
        p.user_principal_name.as_deref(),
        Some("marc@hotmail.example")
    );
}

#[test]
fn device_code_parklands_refreshes_against_its_tenant() {
    let env = Env::start(Duration::from_secs(1));
    env.mock().sign_in_as(Some("parklands"));
    let auth = env.auth(Box::new(MemoryStore::memory()));
    let browser = FakeBrowser::default();
    let rec = auth
        .login(
            &request("parklands", "organizations", Flow::DeviceCode),
            &browser,
        )
        .unwrap();
    assert!(browser.shown.lock().unwrap()[0].contains("microsoft.com/devicelogin"));
    assert_eq!(rec.tenant_id.as_deref(), Some(DEFAULT_TENANT));
    assert_eq!(rec.refresh_authority(), DEFAULT_TENANT);
    // The 1-second token is inside the 5-minute margin, so this refreshes.
    let before = rec.refresh_token.clone();
    let token = auth.access_token("parklands").unwrap();
    assert_ne!(Some(token.as_str()), rec.access_token.as_deref());
    let after = auth.record("parklands").unwrap().unwrap();
    assert_ne!(after.refresh_token, before, "rotated refresh token saved");
    assert!(after.refreshed_at.is_some());
    // Sign-in: one pending poll and one success at organizations. Everything after is refresh.
    let paths = token_paths(env.mock());
    assert_eq!(
        paths[..2],
        [
            "/organizations/oauth2/v2.0/token 400",
            "/organizations/oauth2/v2.0/token 200"
        ]
    );
    assert_eq!(
        paths.last().unwrap(),
        &format!("/{DEFAULT_TENANT}/oauth2/v2.0/token 200"),
        "{paths:?}"
    );
    assert!(
        !paths[2..].iter().any(|p| p.starts_with("/organizations/")),
        "refresh must never go to organizations: {paths:?}"
    );
    assert!(auth.check("parklands").is_ok());
}

#[test]
fn work_account_refresh_via_organizations_is_what_failed_in_the_spike() {
    // Guard on the mock itself: if it stopped modelling AADSTS65001 the test above would prove nothing.
    let env = Env::start(Duration::from_secs(3600));
    env.mock().sign_in_as(Some("parklands"));
    let auth = env.auth(Box::new(MemoryStore::memory()));
    let rec = auth
        .login(
            &request("parklands", "organizations", Flow::DeviceCode),
            &FakeBrowser::default(),
        )
        .unwrap();
    let oauth = familiar_auth::oauth::OAuthClient::new(env.mock().login_url());
    let e = oauth
        .refresh(
            "organizations",
            DEFAULT_CLIENT_ID,
            &rec.refresh_token,
            SCOPES,
        )
        .unwrap()
        .err()
        .expect("organizations refresh fails");
    assert!(e.description.starts_with("AADSTS65001"), "{e}");
}

#[test]
fn sign_in_survives_restart_with_the_file_store() {
    let env = Env::start(Duration::from_secs(1));
    let dir = std::env::temp_dir().join(format!("familiar-auth-restart-{}", std::process::id()));
    let path = dir.join("tokens.enc");
    let _ = std::fs::remove_dir_all(&dir);
    {
        env.mock().sign_in_as(Some("hotmail"));
        let auth = env.auth(Box::new(
            FileStore::new(&path, "a long passphrase").unwrap(),
        ));
        auth.login(
            &request("hotmail", "consumers", Flow::Browser),
            &FakeBrowser::default(),
        )
        .unwrap();
        env.mock().sign_in_as(Some("parklands"));
        auth.login(
            &request("parklands", "organizations", Flow::DeviceCode),
            &FakeBrowser::default(),
        )
        .unwrap();
    }
    // "Restart": nobody is signed in to the browser any more, old access tokens are dead,
    // and a new process opens the store from disk.
    env.mock().sign_in_as(None);
    env.mock().expire_access_tokens("hotmail");
    env.mock().expire_access_tokens("parklands");
    let auth = env.auth(Box::new(
        FileStore::new(&path, "a long passphrase").unwrap(),
    ));
    assert_eq!(auth.accounts().unwrap(), ["hotmail", "parklands"]);
    for account in ["hotmail", "parklands"] {
        let p = auth.check(account).unwrap();
        assert!(p.user_principal_name.unwrap().starts_with("marc@"));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn revoked_refresh_token_means_sign_in_again() {
    let env = Env::start(Duration::from_secs(1));
    env.mock().sign_in_as(Some("hotmail"));
    let auth = env.auth(Box::new(MemoryStore::memory()));
    auth.login(
        &request("hotmail", "consumers", Flow::Browser),
        &FakeBrowser::default(),
    )
    .unwrap();
    env.mock().revoke_refresh_tokens("hotmail");
    let e = auth.access_token("hotmail").unwrap_err();
    assert_eq!(e.kind, AuthErrorKind::AuthRequired);
    assert!(e.message.contains("fam auth login hotmail"), "{e}");
    assert!(!e.message.contains("rt-"), "no token in messages: {e}");
}

#[test]
fn wrong_account_is_refused_and_not_saved() {
    let env = Env::start(Duration::from_secs(3600));
    env.mock().sign_in_as(Some("parklands"));
    let auth = env.auth(Box::new(MemoryStore::memory()));
    let mut req = request("hotmail", "common", Flow::DeviceCode);
    req.email = Some("marc@hotmail.example".into());
    let e = auth.login(&req, &FakeBrowser::default()).unwrap_err();
    assert_eq!(e.kind, AuthErrorKind::Usage);
    assert!(e.message.contains("marc@parklands.example"), "{e}");
    assert!(auth.accounts().unwrap().is_empty());
}

#[test]
fn login_hint_picks_the_account_in_the_browser() {
    let env = Env::start(Duration::from_secs(3600));
    env.mock().sign_in_as(Some("parklands"));
    let auth = env.auth(Box::new(MemoryStore::memory()));
    let mut req = request("hotmail", "consumers", Flow::Browser);
    req.email = Some("marc@hotmail.example".into());
    let rec = auth.login(&req, &FakeBrowser::default()).unwrap();
    assert_eq!(rec.username.as_deref(), Some("marc@hotmail.example"));
}

#[test]
fn personal_account_on_organizations_is_refused() {
    let env = Env::start(Duration::from_secs(3600));
    env.mock().sign_in_as(Some("hotmail"));
    let auth = env.auth(Box::new(MemoryStore::memory()));
    let e = auth
        .login(
            &request("hotmail", "organizations", Flow::Browser),
            &FakeBrowser::default(),
        )
        .unwrap_err();
    assert_eq!(e.kind, AuthErrorKind::AuthRequired);
    assert!(e.message.contains("AADSTS50020"), "{e}");
}

#[test]
fn logout_forgets_the_account() {
    let env = Env::start(Duration::from_secs(3600));
    env.mock().sign_in_as(Some("hotmail"));
    let auth = env.auth(Box::new(MemoryStore::memory()));
    auth.login(
        &request("hotmail", "common", Flow::Browser),
        &FakeBrowser::default(),
    )
    .unwrap();
    assert!(auth.logout("hotmail").unwrap());
    assert!(!auth.logout("hotmail").unwrap());
    assert_eq!(
        auth.access_token("hotmail").unwrap_err().kind,
        AuthErrorKind::AuthRequired
    );
}

#[test]
fn offline_is_reported_as_offline() {
    let auth = Auth::new(
        Box::new(MemoryStore::memory()),
        Endpoints {
            login: "http://127.0.0.1:9".into(),
            graph: "http://127.0.0.1:9/v1.0".into(),
        },
    );
    let e = auth
        .login(
            &request("hotmail", "consumers", Flow::DeviceCode),
            &FakeBrowser::default(),
        )
        .unwrap_err();
    assert_eq!(e.kind, AuthErrorKind::Offline, "{e}");
}
