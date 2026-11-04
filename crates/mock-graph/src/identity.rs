// SPDX-License-Identifier: MIT OR Apache-2.0
//! A mock of the Microsoft identity platform (`login.microsoftonline.com`), so the sign-in
//! code (WP 1.4) can be tested offline: authorisation code with PKCE, device code and
//! refresh tokens, served from the same port as the Graph mock.
//!
//! It copies the Entra behaviour Familiar has to cope with:
//!
//! - A tenant segment of `consumers` only takes personal accounts; `organizations` only
//!   work accounts; a tenant ID only its own accounts; `common` takes both.
//! - Refreshing a work account's token against `organizations` or `common` fails with
//!   `invalid_grant` AADSTS65001, as seen with Parklands in WP 0.2. Refreshes must go to
//!   the account's tenant ID (the `tid` claim of the ID token).
//! - Refresh tokens rotate; the old one stays usable until revoked, as in Entra.
//! - `offline_access` is never echoed in the granted `scope`, and without it there is no
//!   refresh token.
//! - ID tokens are JWTs carrying `tid`, `oid`, `preferred_username` and `name`. They are
//!   unsigned (`alg: none`) because the client does not verify them.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::model::User;

/// The tenant ID Entra gives every personal Microsoft account (Hotmail, Outlook.com).
pub const CONSUMER_TENANT: &str = "9188040d-6c67-4c5b-b112-36a304b66dad";
/// The tenant mailboxes belong to unless [`crate::Builder::tenant`] says otherwise.
pub const DEFAULT_TENANT: &str = "11111111-1111-4111-8111-111111111111";

/// An answer from the identity mock, turned into HTTP by the server.
pub(crate) enum IdReply {
    /// JSON with a status.
    Json(u16, Value),
    /// A redirect (the end of `/authorize`).
    Redirect(String),
    /// Plain text with a status.
    Text(u16, String),
}

struct AuthCode {
    key: String,
    client_id: String,
    redirect_uri: String,
    challenge: String,
    scope: String,
}

struct DeviceCode {
    client_id: String,
    scope: String,
    tenant: String,
    polls: u32,
    expires: Instant,
}

struct RefreshGrant {
    key: String,
    client_id: String,
    scope: String,
}

pub(crate) struct Identity {
    tenants: HashMap<String, String>,
    signed_in: Option<String>,
    codes: HashMap<String, AuthCode>,
    devices: HashMap<String, DeviceCode>,
    refresh: HashMap<String, RefreshGrant>,
    pub(crate) lifetime: Duration,
    pub(crate) device_interval: u64,
    counter: u64,
}

impl Identity {
    pub(crate) fn new(tenants: HashMap<String, String>) -> Self {
        Self {
            tenants,
            signed_in: None,
            codes: HashMap::new(),
            devices: HashMap::new(),
            refresh: HashMap::new(),
            lifetime: Duration::from_secs(3600),
            device_interval: 1,
            counter: 0,
        }
    }

    pub(crate) fn tenant_of(&self, key: &str) -> &str {
        self.tenants
            .get(key)
            .map(String::as_str)
            .unwrap_or(DEFAULT_TENANT)
    }

    pub(crate) fn sign_in_as(&mut self, key: Option<String>) {
        self.signed_in = key;
    }

    pub(crate) fn revoke(&mut self, key: &str) {
        self.refresh.retain(|_, g| g.key != key);
    }

    fn next(&mut self, prefix: &str, key: &str) -> String {
        self.counter += 1;
        format!("{prefix}-{key}-{:06}", self.counter)
    }
}

/// True when the path is an identity endpoint: `/{tenant}/oauth2/v2.0/{op}`.
pub(crate) fn parse_path(path: &str) -> Option<(String, String)> {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match segs.as_slice() {
        [tenant, "oauth2", "v2.0", op] if matches!(*op, "authorize" | "token" | "devicecode") => {
            Some((tenant.to_string(), op.to_string()))
        }
        _ => None,
    }
}

fn oauth_error(status: u16, error: &str, aadsts: &str, message: &str) -> IdReply {
    IdReply::Json(
        status,
        json!({
            "error": error,
            "error_description": format!("{aadsts}: {message}\r\nTrace ID: 00000000-0000-0000-0000-000000000000\r\nCorrelation ID: 00000000-0000-0000-0000-000000000000"),
            "error_codes": [aadsts.trim_start_matches("AADSTS").parse::<u64>().unwrap_or(0)],
        }),
    )
}

/// Whether an account in tenant `tid` may use the tenant segment `seg`.
fn tenant_allows(seg: &str, tid: &str) -> bool {
    let personal = tid == CONSUMER_TENANT;
    match seg.to_ascii_lowercase().as_str() {
        "common" => true,
        "consumers" => personal,
        "organizations" => !personal,
        other => other.eq_ignore_ascii_case(tid),
    }
}

fn get<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn id_token(user: &User, tid: &str, client_id: &str) -> String {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
    let now = now_secs();
    let claims = json!({
        "aud": client_id,
        "iss": format!("https://login.microsoftonline.com/{tid}/v2.0"),
        "iat": now,
        "nbf": now,
        "exp": now + 3600,
        "tid": tid,
        "oid": user.id,
        "preferred_username": user.user_principal_name,
        "name": user.display_name,
        "ver": "2.0",
    });
    let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
    format!("{header}.{payload}.")
}

/// Scopes as Entra reports them back: without `offline_access`.
fn granted(scope: &str) -> String {
    scope
        .split_whitespace()
        .filter(|s| !s.eq_ignore_ascii_case("offline_access"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Handles one identity request. `users` maps mailbox keys to their users; issued access
/// tokens are returned in `issued` so the server can accept them on Graph routes.
pub(crate) fn handle(
    id: &mut Identity,
    users: &HashMap<String, User>,
    tenant: &str,
    op: &str,
    method: &str,
    params: &[(String, String)],
    issued: &mut Vec<(String, String)>,
) -> IdReply {
    match (op, method) {
        ("authorize", "GET") => authorize(id, users, tenant, params),
        ("devicecode", "POST") => device_code(id, tenant, params),
        ("token", "POST") => token(id, users, tenant, params, issued),
        _ => IdReply::Text(405, "method not allowed".into()),
    }
}

fn find_user<'a>(users: &'a HashMap<String, User>, hint: &str) -> Option<&'a str> {
    users
        .iter()
        .find(|(k, u)| {
            k.eq_ignore_ascii_case(hint)
                || u.user_principal_name.eq_ignore_ascii_case(hint)
                || u.mail.eq_ignore_ascii_case(hint)
        })
        .map(|(k, _)| k.as_str())
}

fn authorize(
    id: &mut Identity,
    users: &HashMap<String, User>,
    tenant: &str,
    params: &[(String, String)],
) -> IdReply {
    let (Some(client_id), Some(redirect_uri)) =
        (get(params, "client_id"), get(params, "redirect_uri"))
    else {
        return IdReply::Text(
            400,
            "AADSTS900144: client_id and redirect_uri are required".into(),
        );
    };
    if get(params, "response_type") != Some("code") {
        return IdReply::Text(400, "AADSTS900144: response_type must be code".into());
    }
    let state = get(params, "state").unwrap_or("");
    let back = |pairs: &[(&str, &str)]| {
        let mut ser = form_urlencoded::Serializer::new(String::new());
        for (k, v) in pairs {
            ser.append_pair(k, v);
        }
        ser.append_pair("state", state);
        IdReply::Redirect(format!("{redirect_uri}?{}", ser.finish()))
    };
    let (Some(challenge), Some("S256")) = (
        get(params, "code_challenge"),
        get(params, "code_challenge_method"),
    ) else {
        return back(&[
            ("error", "invalid_request"),
            (
                "error_description",
                "AADSTS9002325: Proof Key for Code Exchange is required.",
            ),
        ]);
    };
    // The browser session decides who signs in; prompt=select_account in a real browser
    // would let the user pick, which the mock models as login_hint.
    let hinted = get(params, "login_hint").and_then(|h| find_user(users, h));
    let Some(key) = hinted.map(str::to_string).or_else(|| id.signed_in.clone()) else {
        return IdReply::Text(
            400,
            "mock identity: nobody is signed in (call sign_in_as)".into(),
        );
    };
    if !tenant_allows(tenant, id.tenant_of(&key)) {
        return back(&[
            ("error", "access_denied"),
            (
                "error_description",
                "AADSTS50020: User account does not exist in tenant.",
            ),
        ]);
    }
    let code = id.next("code", &key);
    id.codes.insert(
        code.clone(),
        AuthCode {
            key,
            client_id: client_id.to_string(),
            redirect_uri: redirect_uri.to_string(),
            challenge: challenge.to_string(),
            scope: get(params, "scope").unwrap_or("").to_string(),
        },
    );
    back(&[("code", &code)])
}

fn device_code(id: &mut Identity, tenant: &str, params: &[(String, String)]) -> IdReply {
    let Some(client_id) = get(params, "client_id") else {
        return oauth_error(
            400,
            "invalid_request",
            "AADSTS900144",
            "client_id is required.",
        );
    };
    let code = id.next("device", "pending");
    let user_code = format!("MOCK{:04}", id.counter % 10_000);
    let expires_in = 900;
    id.devices.insert(
        code.clone(),
        DeviceCode {
            client_id: client_id.to_string(),
            scope: get(params, "scope").unwrap_or("").to_string(),
            tenant: tenant.to_string(),
            polls: 0,
            expires: Instant::now() + Duration::from_secs(expires_in),
        },
    );
    IdReply::Json(
        200,
        json!({
            "device_code": code,
            "user_code": user_code,
            "verification_uri": "https://microsoft.com/devicelogin",
            "expires_in": expires_in,
            "interval": id.device_interval,
            "message": format!("To sign in, use a web browser to open the page https://microsoft.com/devicelogin and enter the code {user_code} to authenticate."),
        }),
    )
}

fn issue(
    id: &mut Identity,
    users: &HashMap<String, User>,
    key: &str,
    client_id: &str,
    scope: &str,
    issued: &mut Vec<(String, String)>,
) -> IdReply {
    let Some(user) = users.get(key) else {
        return oauth_error(
            400,
            "invalid_grant",
            "AADSTS50034",
            "The user account does not exist.",
        );
    };
    let tid = id.tenant_of(key).to_string();
    let access = id.next("at", key);
    issued.push((access.clone(), key.to_string()));
    let mut body = json!({
        "token_type": "Bearer",
        "scope": granted(scope),
        "expires_in": id.lifetime.as_secs(),
        "ext_expires_in": id.lifetime.as_secs(),
        "access_token": access,
    });
    let has = |s: &str| scope.split_whitespace().any(|x| x.eq_ignore_ascii_case(s));
    if has("offline_access") {
        let rt = id.next("rt", key);
        id.refresh.insert(
            rt.clone(),
            RefreshGrant {
                key: key.to_string(),
                client_id: client_id.to_string(),
                scope: scope.to_string(),
            },
        );
        body["refresh_token"] = json!(rt);
    }
    if has("openid") {
        body["id_token"] = json!(id_token(user, &tid, client_id));
    }
    IdReply::Json(200, body)
}

fn token(
    id: &mut Identity,
    users: &HashMap<String, User>,
    tenant: &str,
    params: &[(String, String)],
    issued: &mut Vec<(String, String)>,
) -> IdReply {
    let Some(client_id) = get(params, "client_id").map(str::to_string) else {
        return oauth_error(
            400,
            "invalid_request",
            "AADSTS900144",
            "client_id is required.",
        );
    };
    match get(params, "grant_type") {
        Some("authorization_code") => {
            let Some(code) = get(params, "code").and_then(|c| id.codes.remove(c)) else {
                return oauth_error(
                    400,
                    "invalid_grant",
                    "AADSTS70008",
                    "The provided authorization code is invalid or has expired.",
                );
            };
            if code.client_id != client_id {
                return oauth_error(
                    400,
                    "invalid_grant",
                    "AADSTS70000",
                    "The code was issued to another client.",
                );
            }
            if get(params, "redirect_uri") != Some(code.redirect_uri.as_str()) {
                return oauth_error(
                    400,
                    "invalid_grant",
                    "AADSTS50148",
                    "The redirect_uri does not match the one used to obtain the code.",
                );
            }
            let verifier = get(params, "code_verifier").unwrap_or("");
            if URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) != code.challenge {
                return oauth_error(
                    400,
                    "invalid_grant",
                    "AADSTS501481",
                    "The Code_Verifier does not match the code_challenge supplied in the authorization request.",
                );
            }
            if !tenant_allows(tenant, id.tenant_of(&code.key)) {
                return oauth_error(
                    400,
                    "invalid_grant",
                    "AADSTS50020",
                    "User account does not exist in tenant.",
                );
            }
            let scope = get(params, "scope")
                .map(str::to_string)
                .unwrap_or(code.scope);
            issue(id, users, &code.key, &client_id, &scope, issued)
        }
        Some("urn:ietf:params:oauth:grant-type:device_code") => {
            let Some(code) = get(params, "device_code") else {
                return oauth_error(
                    400,
                    "invalid_request",
                    "AADSTS900144",
                    "device_code is required.",
                );
            };
            let signed_in = id.signed_in.clone();
            let Some(dev) = id.devices.get_mut(code) else {
                return oauth_error(
                    400,
                    "expired_token",
                    "AADSTS70019",
                    "Verification code expired.",
                );
            };
            if Instant::now() > dev.expires {
                id.devices.remove(code);
                return oauth_error(
                    400,
                    "expired_token",
                    "AADSTS70019",
                    "Verification code expired.",
                );
            }
            if dev.client_id != client_id {
                return oauth_error(
                    400,
                    "invalid_grant",
                    "AADSTS70000",
                    "The code was issued to another client.",
                );
            }
            dev.polls += 1;
            // The first poll is always pending, as the user can't have finished yet.
            let Some(key) = signed_in.filter(|_| dev.polls > 1) else {
                return oauth_error(
                    400,
                    "authorization_pending",
                    "AADSTS70016",
                    "OAuth 2.0 device flow error. Authorization is pending.",
                );
            };
            let dev = id.devices.remove(code).expect("present");
            if !tenant_allows(&dev.tenant, id.tenant_of(&key)) {
                return oauth_error(
                    400,
                    "invalid_grant",
                    "AADSTS50020",
                    "User account does not exist in tenant.",
                );
            }
            issue(id, users, &key, &client_id, &dev.scope, issued)
        }
        Some("refresh_token") => {
            let Some(grant) = get(params, "refresh_token").and_then(|t| id.refresh.get(t)) else {
                return oauth_error(
                    400,
                    "invalid_grant",
                    "AADSTS700082",
                    "The refresh token has expired due to inactivity.",
                );
            };
            let (key, scope_was) = (grant.key.clone(), grant.scope.clone());
            if grant.client_id != client_id {
                return oauth_error(
                    400,
                    "invalid_grant",
                    "AADSTS700005",
                    "The refresh token was issued to another client.",
                );
            }
            let tid = id.tenant_of(&key).to_string();
            let seg = tenant.to_ascii_lowercase();
            if tid != CONSUMER_TENANT && (seg == "organizations" || seg == "common") {
                // What Parklands did in WP 0.2: the multi-tenant alias can't find the grant.
                return oauth_error(
                    400,
                    "invalid_grant",
                    "AADSTS65001",
                    "The user or administrator has not consented to use the application.",
                );
            }
            if !tenant_allows(tenant, &tid) {
                return oauth_error(
                    400,
                    "invalid_grant",
                    "AADSTS50020",
                    "User account does not exist in tenant.",
                );
            }
            let scope = get(params, "scope")
                .map(str::to_string)
                .unwrap_or(scope_was);
            issue(id, users, &key, &client_id, &scope, issued)
        }
        _ => oauth_error(
            400,
            "unsupported_grant_type",
            "AADSTS70003",
            "The app requested an unsupported grant type.",
        ),
    }
}
