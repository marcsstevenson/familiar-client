// SPDX-License-Identifier: MIT OR Apache-2.0
//! Quota errors, outages, expired and revoked tokens, and the Google sign-in endpoints.

mod common;

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::Client;
use familiar_mock_gmail::{Fault, Mailbox, MockGmail, SMALL_EMAIL};
use sha2::{Digest, Sha256};

#[tokio::test]
async fn missing_and_expired_tokens() {
    let mock = MockGmail::start().await.unwrap();
    let r = Client::new(&mock.origin(), "").get("profile").await;
    assert_eq!(r.status, 401);
    assert_eq!(r.reason(), "authError");
    assert_eq!(r.body["error"]["status"], "UNAUTHENTICATED");
    assert!(
        r.header("www-authenticate")
            .unwrap()
            .contains("invalid_token")
    );
    assert_eq!(
        Client::new(&mock.origin(), "bogus")
            .get("profile")
            .await
            .status,
        401
    );

    let c = Client::new(&mock.origin(), "test-token");
    assert_eq!(c.get("profile").await.status, 200);
    mock.expire_token("test-token");
    assert_eq!(c.get("profile").await.status, 401);
    mock.add_token("fresh", "default");
    assert_eq!(
        Client::new(&mock.origin(), "fresh")
            .get("profile")
            .await
            .status,
        200
    );
    mock.expire_access_tokens("default");
    assert_eq!(
        Client::new(&mock.origin(), "fresh")
            .get("profile")
            .await
            .status,
        401
    );
}

#[tokio::test]
async fn quota_faults() {
    let mock = MockGmail::start().await.unwrap();
    let c = Client::new(&mock.origin(), "test-token");
    mock.push_fault(Fault::rate_limited().on_path("/history").times(1));
    mock.push_fault(Fault::user_rate_limited().on_method("POST").times(1));
    mock.push_fault(
        Fault::unavailable()
            .retry_after(3)
            .on_path("/labels")
            .times(1),
    );

    let r = c.get("history?startHistoryId=1").await;
    assert_eq!(r.status, 429);
    assert_eq!(r.reason(), "rateLimitExceeded");
    assert_eq!(r.body["error"]["errors"][0]["domain"], "usageLimits");
    assert!(
        r.header("retry-after").is_none(),
        "Gmail sends no Retry-After"
    );

    let r = c
        .post("messages/msg-dinner/trash", serde_json::json!({}))
        .await;
    assert_eq!(r.status, 403);
    assert_eq!(r.reason(), "userRateLimitExceeded");

    let r = c.get("labels").await;
    assert_eq!(r.status, 503);
    assert_eq!(r.header("retry-after"), Some("3"));

    // Each fault fired once.
    assert_eq!(c.get("labels").await.status, 200);
    assert_eq!(
        c.post("messages/msg-dinner/trash", serde_json::json!({}))
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn quota_budget_counts_units() {
    let mock = MockGmail::builder()
        .mailbox("gmail", Mailbox::small())
        .quota(12, Duration::from_millis(300))
        .start()
        .await
        .unwrap();
    let c = Client::new(&mock.origin(), "gmail-token");
    assert_eq!(
        c.get("messages/msg-dinner?format=minimal").await.status,
        200
    ); // 5
    assert_eq!(c.get("history?startHistoryId=100000").await.status, 404); // 2, history gone
    assert_eq!(
        c.get("messages/msg-photos?format=minimal").await.status,
        200
    ); // 5 = 12
    let r = c.get("profile").await;
    assert_eq!(r.status, 429);
    assert_eq!(r.reason(), "rateLimitExceeded");
    assert_eq!(mock.mailbox("gmail").units_used(), 12);
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert_eq!(c.get("profile").await.status, 200);
}

#[tokio::test]
async fn concurrency_limit() {
    let mock = MockGmail::builder()
        .mailbox("gmail", Mailbox::small())
        .max_concurrent(Some(2))
        .latency(Duration::from_millis(100))
        .start()
        .await
        .unwrap();
    let c = Client::new(&mock.origin(), "gmail-token");
    let (a, b, d) = tokio::join!(c.get("profile"), c.get("profile"), c.get("profile"));
    let mut statuses = [a.status, b.status, d.status];
    statuses.sort();
    assert_eq!(statuses, [200, 200, 429]);
    assert_eq!(mock.mailbox("gmail").peak_in_flight(), 2);
}

fn location(r: &common::Resp) -> String {
    r.header("location").unwrap().to_string()
}

fn query_param(url: &str, name: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    form_urlencoded::parse(q.as_bytes())
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
}

#[tokio::test]
async fn sign_in_with_pkce_refresh_and_revoke() {
    let mock = MockGmail::builder()
        .mailbox("gmail", Mailbox::small())
        .client("client-1", "secret-1")
        .token_lifetime(Duration::from_millis(300))
        .start()
        .await
        .unwrap();
    let c = Client::new(&mock.origin(), "");
    let verifier = "a-verifier-that-is-long-enough-for-pkce-0123456789";
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let auth = |hint: &str| {
        format!(
            "{}?client_id=client-1&redirect_uri=http%3A%2F%2F127.0.0.1%3A5555%2F&response_type=code\
             &scope=https%3A%2F%2Fmail.google.com%2F&access_type=offline&state=s1\
             &code_challenge={challenge}&code_challenge_method=S256{hint}",
            mock.auth_url()
        )
    };

    // Nobody signed in: the user "cancels".
    let r = c.send(c.http.get(auth(""))).await;
    assert_eq!(r.status, 302);
    assert_eq!(
        query_param(&location(&r), "error").as_deref(),
        Some("access_denied")
    );

    let r = c
        .send(c.http.get(auth(&format!("&login_hint={SMALL_EMAIL}"))))
        .await;
    let loc = location(&r);
    assert!(loc.starts_with("http://127.0.0.1:5555/?"));
    assert_eq!(query_param(&loc, "state").as_deref(), Some("s1"));
    let code = query_param(&loc, "code").unwrap();

    let token = |form: Vec<(&'static str, String)>| {
        let c = &c;
        let url = mock.token_url();
        async move { c.send(c.http.post(url).form(&form)).await }
    };
    let base = |code: &str, verifier: &str| {
        vec![
            ("grant_type", "authorization_code".to_string()),
            ("code", code.to_string()),
            ("code_verifier", verifier.to_string()),
            ("redirect_uri", "http://127.0.0.1:5555/".to_string()),
            ("client_id", "client-1".to_string()),
            ("client_secret", "secret-1".to_string()),
        ]
    };
    let r = token(base(&code, "wrong")).await;
    assert_eq!(r.body["error"], "invalid_grant");
    // Codes are single use, even after a failed exchange.
    let r = token(base(&code, verifier)).await;
    assert_eq!(r.body["error"], "invalid_grant");

    mock.sign_in_as(Some("gmail"));
    let r = c.send(c.http.get(auth(""))).await;
    let code = query_param(&location(&r), "code").unwrap();
    let mut bad_secret = base(&code, verifier);
    bad_secret[5].1 = "nope".into();
    assert_eq!(token(bad_secret).await.status, 401);
    let r = c.send(c.http.get(auth(""))).await;
    let code = query_param(&location(&r), "code").unwrap();
    let r = token(base(&code, verifier)).await;
    assert_eq!(r.status, 200, "{}", r.text);
    let access = r.body["access_token"].as_str().unwrap().to_string();
    let refresh = r.body["refresh_token"].as_str().unwrap().to_string();
    assert_eq!(r.body["token_type"], "Bearer");
    let claims = r.body["id_token"]
        .as_str()
        .unwrap()
        .split('.')
        .nth(1)
        .unwrap()
        .to_string();
    let claims: serde_json::Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(claims).unwrap()).unwrap();
    assert_eq!(claims["email"], SMALL_EMAIL);

    let api = Client::new(&mock.origin(), &access);
    assert_eq!(api.get("profile").await.status, 200);
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert_eq!(api.get("profile").await.status, 401, "access token lapsed");

    let refresh_form = || {
        vec![
            ("grant_type", "refresh_token".to_string()),
            ("refresh_token", refresh.clone()),
            ("client_id", "client-1".to_string()),
            ("client_secret", "secret-1".to_string()),
        ]
    };
    let r = token(refresh_form()).await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert!(
        r.body.get("refresh_token").is_none(),
        "Google does not rotate"
    );
    let access = r.body["access_token"].as_str().unwrap().to_string();
    assert_eq!(
        Client::new(&mock.origin(), &access)
            .get("profile")
            .await
            .status,
        200
    );

    mock.revoke_refresh_tokens("gmail");
    let r = token(refresh_form()).await;
    assert_eq!(r.status, 400);
    assert_eq!(r.body["error"], "invalid_grant");
    assert_eq!(
        r.body["error_description"],
        "Token has been expired or revoked."
    );
}

#[tokio::test]
async fn revoke_endpoint() {
    let mock = MockGmail::builder()
        .mailbox("gmail", Mailbox::small())
        .start()
        .await
        .unwrap();
    mock.sign_in_as(Some("gmail"));
    let c = Client::new(&mock.origin(), "");
    let r = c
        .send(c.http.get(format!(
            "{}?client_id=x&redirect_uri=http://localhost/cb&response_type=code&access_type=offline",
            mock.auth_url()
        )))
        .await;
    let code = query_param(&location(&r), "code").unwrap();
    let r = c
        .send(c.http.post(mock.token_url()).form(&[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", "http://localhost/cb"),
            ("client_id", "x"),
        ]))
        .await;
    let access = r.body["access_token"].as_str().unwrap().to_string();
    let refresh = r.body["refresh_token"].as_str().unwrap().to_string();
    let r = c
        .send(
            c.http
                .post(mock.revoke_url())
                .form(&[("token", refresh.as_str())]),
        )
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(
        Client::new(&mock.origin(), &access)
            .get("profile")
            .await
            .status,
        401
    );
}
