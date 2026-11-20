// SPDX-License-Identifier: MIT OR Apache-2.0
//! Google sign-in: the authorisation endpoint, the token endpoint (authorisation code with
//! PKCE, and refresh) and revocation, plus the bearer tokens the API routes accept.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

pub(crate) struct Access {
    pub mailbox: String,
    pub expires_at: Option<Instant>,
    pub expired: bool,
}

struct Refresh {
    mailbox: String,
    scope: String,
    revoked: bool,
}

struct Code {
    mailbox: String,
    challenge: Option<(String, String)>,
    redirect_uri: String,
    scope: String,
    client_id: String,
    offline: bool,
}

pub(crate) struct Auth {
    pub tokens: HashMap<String, Access>,
    refresh: HashMap<String, Refresh>,
    codes: HashMap<String, Code>,
    signed_in: Option<String>,
    counter: u64,
    pub lifetime: Duration,
    pub client: Option<(String, String)>,
}

pub(crate) enum AuthReply {
    Json(u16, Value),
    Redirect(String),
}

/// A mailbox key and its user's address and name.
pub(crate) type Users = HashMap<String, (String, String)>;

impl Auth {
    pub fn new(lifetime: Duration, client: Option<(String, String)>) -> Self {
        Self {
            tokens: HashMap::new(),
            refresh: HashMap::new(),
            codes: HashMap::new(),
            signed_in: None,
            counter: 0,
            lifetime,
            client,
        }
    }

    pub fn add_static(&mut self, token: String, mailbox: String) {
        self.tokens.insert(
            token,
            Access {
                mailbox,
                expires_at: None,
                expired: false,
            },
        );
    }

    pub fn sign_in_as(&mut self, key: Option<String>) {
        self.signed_in = key;
    }

    pub fn revoke(&mut self, key: &str) {
        for r in self.refresh.values_mut() {
            if r.mailbox == key {
                r.revoked = true;
            }
        }
    }

    pub fn expire_access(&mut self, key: &str) {
        for t in self.tokens.values_mut() {
            if t.mailbox == key {
                t.expired = true;
            }
        }
    }

    /// The mailbox a bearer token belongs to.
    pub fn check(&self, token: &str) -> Option<String> {
        let t = self.tokens.get(token)?;
        let lapsed = t.expires_at.is_some_and(|e| Instant::now() >= e);
        (!t.expired && !lapsed).then(|| t.mailbox.clone())
    }

    fn next(&mut self) -> u64 {
        self.counter += 1;
        self.counter
    }

    fn issue_access(&mut self, key: &str) -> String {
        let n = self.next();
        let token = format!("ya29.mock-{key}-{n}");
        self.tokens.insert(
            token.clone(),
            Access {
                mailbox: key.into(),
                expires_at: Some(Instant::now() + self.lifetime),
                expired: false,
            },
        );
        token
    }
}

fn oauth_error(status: u16, error: &str, description: &str) -> AuthReply {
    AuthReply::Json(
        status,
        json!({"error": error, "error_description": description}),
    )
}

fn get<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
        .filter(|v| !v.is_empty())
}

fn id_token(key: &str, email: &str, name: &str, aud: &str) -> String {
    let now = chrono::Utc::now().timestamp();
    let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
    let claims = json!({
        "iss": "https://accounts.google.com",
        "aud": aud,
        "sub": format!("mock-sub-{key}"),
        "email": email,
        "email_verified": true,
        "name": name,
        "iat": now,
        "exp": now + 3600,
    });
    format!("{header}.{}.", URL_SAFE_NO_PAD.encode(claims.to_string()))
}

/// Serves `/o/oauth2/v2/auth`, `/token` and `/revoke`.
pub(crate) fn handle(
    auth: &mut Auth,
    users: &Users,
    path: &str,
    params: &[(String, String)],
) -> AuthReply {
    match path {
        "/o/oauth2/v2/auth" => authorize(auth, users, params),
        "/token" => token(auth, users, params),
        "/revoke" => {
            let Some(t) = get(params, "token") else {
                return oauth_error(400, "invalid_request", "Missing required parameter: token");
            };
            if let Some(r) = auth.refresh.get_mut(t) {
                r.revoked = true;
                let key = r.mailbox.clone();
                auth.expire_access(&key);
            } else if let Some(a) = auth.tokens.get_mut(t) {
                a.expired = true;
            } else {
                return oauth_error(400, "invalid_token", "Token expired or revoked");
            }
            AuthReply::Json(200, json!({}))
        }
        _ => oauth_error(404, "not_found", "Unknown endpoint"),
    }
}

fn authorize(auth: &mut Auth, users: &Users, params: &[(String, String)]) -> AuthReply {
    let Some(redirect_uri) = get(params, "redirect_uri") else {
        return oauth_error(
            400,
            "invalid_request",
            "Missing required parameter: redirect_uri",
        );
    };
    let Some(client_id) = get(params, "client_id") else {
        return oauth_error(
            400,
            "invalid_request",
            "Missing required parameter: client_id",
        );
    };
    if auth.client.as_ref().is_some_and(|(id, _)| id != client_id) {
        return oauth_error(401, "invalid_client", "The OAuth client was not found.");
    }
    if get(params, "response_type") != Some("code") {
        return oauth_error(
            400,
            "unsupported_response_type",
            "response_type must be code",
        );
    }
    let challenge = match get(params, "code_challenge") {
        None => None,
        Some(c) => {
            let method = get(params, "code_challenge_method").unwrap_or("plain");
            if method != "S256" && method != "plain" {
                return oauth_error(400, "invalid_request", "Invalid code_challenge_method");
            }
            Some((method.to_string(), c.to_string()))
        }
    };
    let state = get(params, "state");
    let mut ser = form_urlencoded::Serializer::new(String::new());
    if let Some(s) = state {
        ser.append_pair("state", s);
    }
    let hint = get(params, "login_hint").map(str::to_ascii_lowercase);
    let who = hint
        .and_then(|h| {
            users
                .iter()
                .find(|(_, (e, _))| e.eq_ignore_ascii_case(&h))
                .map(|(k, _)| k.clone())
        })
        .or_else(|| auth.signed_in.clone());
    let sep = if redirect_uri.contains('?') { '&' } else { '?' };
    let Some(key) = who else {
        ser.append_pair("error", "access_denied");
        return AuthReply::Redirect(format!("{redirect_uri}{sep}{}", ser.finish()));
    };
    let scope = get(params, "scope").unwrap_or("").to_string();
    let n = auth.next();
    let code = format!("4/mock-code-{n}");
    auth.codes.insert(
        code.clone(),
        Code {
            mailbox: key,
            challenge,
            redirect_uri: redirect_uri.into(),
            scope: scope.clone(),
            client_id: client_id.into(),
            offline: get(params, "access_type") == Some("offline"),
        },
    );
    ser.append_pair("code", &code);
    ser.append_pair("scope", &scope);
    AuthReply::Redirect(format!("{redirect_uri}{sep}{}", ser.finish()))
}

fn token(auth: &mut Auth, users: &Users, params: &[(String, String)]) -> AuthReply {
    let client_id = get(params, "client_id").unwrap_or("");
    if let Some((id, secret)) = &auth.client
        && (id != client_id || get(params, "client_secret") != Some(secret.as_str()))
    {
        return oauth_error(401, "invalid_client", "Unauthorized");
    }
    let lifetime = auth.lifetime.as_secs();
    match get(params, "grant_type") {
        Some("authorization_code") => {
            let Some(code) = get(params, "code").and_then(|c| auth.codes.remove(c)) else {
                return oauth_error(400, "invalid_grant", "Malformed auth code.");
            };
            if get(params, "redirect_uri") != Some(code.redirect_uri.as_str()) {
                return oauth_error(400, "redirect_uri_mismatch", "Bad Request");
            }
            if let Some((method, challenge)) = &code.challenge {
                let Some(verifier) = get(params, "code_verifier") else {
                    return oauth_error(400, "invalid_grant", "Missing code verifier.");
                };
                let computed = if method == "S256" {
                    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
                } else {
                    verifier.to_string()
                };
                if &computed != challenge {
                    return oauth_error(400, "invalid_grant", "Invalid code verifier.");
                }
            }
            let access = auth.issue_access(&code.mailbox);
            let (email, name) = users.get(&code.mailbox).cloned().unwrap_or_default();
            let mut body = json!({
                "access_token": access,
                "expires_in": lifetime,
                "scope": code.scope,
                "token_type": "Bearer",
                "id_token": id_token(&code.mailbox, &email, &name, &code.client_id),
            });
            if code.offline {
                let n = auth.next();
                let refresh = format!("1//mock-refresh-{}-{n}", code.mailbox);
                auth.refresh.insert(
                    refresh.clone(),
                    Refresh {
                        mailbox: code.mailbox.clone(),
                        scope: code.scope.clone(),
                        revoked: false,
                    },
                );
                body["refresh_token"] = json!(refresh);
            }
            AuthReply::Json(200, body)
        }
        Some("refresh_token") => {
            let Some(r) = get(params, "refresh_token").and_then(|t| auth.refresh.get(t)) else {
                return oauth_error(400, "invalid_grant", "Bad Request");
            };
            if r.revoked {
                return oauth_error(400, "invalid_grant", "Token has been expired or revoked.");
            }
            let (key, scope) = (r.mailbox.clone(), r.scope.clone());
            let access = auth.issue_access(&key);
            let (email, name) = users.get(&key).cloned().unwrap_or_default();
            // Google does not rotate refresh tokens.
            AuthReply::Json(
                200,
                json!({
                    "access_token": access,
                    "expires_in": lifetime,
                    "scope": scope,
                    "token_type": "Bearer",
                    "id_token": id_token(&key, &email, &name, client_id),
                }),
            )
        }
        _ => oauth_error(400, "unsupported_grant_type", "Invalid grant_type"),
    }
}
