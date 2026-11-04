// SPDX-License-Identifier: MIT OR Apache-2.0
//! The OAuth 2.0 flows against the Microsoft identity platform: authorisation code with
//! PKCE (system browser, loopback redirect) and device code, plus token refresh.
//! Production version of the WP 0.2 spike (`spikes/auth`).

use crate::error::{AuthError, AuthErrorKind, Result};
use crate::store::random;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// A successful token response.
pub struct TokenResponse {
    /// The access token.
    pub access_token: String,
    /// The refresh token, when `offline_access` was granted.
    pub refresh_token: Option<String>,
    /// The ID token, when `openid` was asked for.
    pub id_token: Option<String>,
    /// Granted scopes, space separated.
    pub scope: String,
    /// Access token lifetime in seconds.
    pub expires_in: u64,
}

/// An OAuth error from the token endpoint.
#[derive(Debug, Clone)]
pub struct OAuthError {
    /// HTTP status.
    pub status: u16,
    /// The `error` field, for example `invalid_grant`.
    pub error: String,
    /// The first line of `error_description`: the AADSTS code and message.
    pub description: String,
}

impl OAuthError {
    fn from_body(status: u16, body: &Value) -> Self {
        Self {
            status,
            error: body["error"].as_str().unwrap_or("unknown").to_string(),
            description: body["error_description"]
                .as_str()
                .and_then(|d| d.lines().next())
                .unwrap_or("")
                .trim()
                .to_string(),
        }
    }

    /// True when only a new interactive sign-in can fix it.
    pub fn needs_sign_in(&self) -> bool {
        matches!(
            self.error.as_str(),
            "invalid_grant" | "interaction_required" | "consent_required" | "login_required"
        )
    }
}

impl std::fmt::Display for OAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "HTTP {} {}: {}",
            self.status, self.error, self.description
        )
    }
}

/// What [`OAuthClient::pkce`] signs in to.
#[derive(Debug, Clone, Copy)]
pub struct PkceRequest<'a> {
    /// `consumers`, `organizations`, `common`, a tenant ID or domain.
    pub authority: &'a str,
    /// The app (client) ID.
    pub client_id: &'a str,
    /// Space-separated scopes.
    pub scopes: &'a str,
    /// Sent as `login_hint`; without one the page asks which account (`prompt=select_account`).
    pub login_hint: Option<&'a str>,
    /// How long to wait for the redirect.
    pub timeout: Duration,
}

/// Talks to one identity platform host.
pub struct OAuthClient {
    login: String,
    agent: ureq::Agent,
}

/// Turns a transport failure into an `Offline` error.
pub(crate) fn transport(what: &str, e: &ureq::Error) -> AuthError {
    AuthError::new(AuthErrorKind::Offline, format!("can't reach {what}: {e}"))
}

pub(crate) fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout(Duration::from_secs(60))
        .user_agent(concat!("familiar/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// Called with each line the person needs to see, such as the device code message.
pub type Notify<'a> = &'a dyn Fn(&str);

impl OAuthClient {
    /// A client for `login`, normally `https://login.microsoftonline.com`.
    pub fn new(login: impl Into<String>) -> Self {
        Self {
            login: login.into().trim_end_matches('/').to_string(),
            agent: agent(),
        }
    }

    fn url(&self, authority: &str, op: &str) -> String {
        format!("{}/{authority}/oauth2/v2.0/{op}", self.login)
    }

    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<(u16, Value)> {
        let resp = match self.agent.post(url).send_form(form) {
            Ok(r) => r,
            Err(ureq::Error::Status(_, r)) => r,
            Err(e) => return Err(transport("the Microsoft sign-in service", &e)),
        };
        let status = resp.status();
        let body = resp.into_json::<Value>().map_err(|e| {
            AuthError::server(format!(
                "sign-in service answered HTTP {status} with an unreadable body: {e}"
            ))
        })?;
        Ok((status, body))
    }

    /// POSTs to the token endpoint. Outer error: transport; inner: the OAuth error.
    pub fn token(
        &self,
        authority: &str,
        form: &[(&str, &str)],
    ) -> Result<std::result::Result<TokenResponse, OAuthError>> {
        let (status, body) = self.post_form(&self.url(authority, "token"), form)?;
        if status != 200 {
            return Ok(Err(OAuthError::from_body(status, &body)));
        }
        let access_token = body["access_token"]
            .as_str()
            .ok_or_else(|| AuthError::server("token response without access_token"))?
            .to_string();
        Ok(Ok(TokenResponse {
            access_token,
            refresh_token: body["refresh_token"].as_str().map(str::to_string),
            id_token: body["id_token"].as_str().map(str::to_string),
            scope: body["scope"].as_str().unwrap_or("").to_string(),
            expires_in: body["expires_in"].as_u64().unwrap_or(3600),
        }))
    }

    /// Refreshes with a refresh token.
    pub fn refresh(
        &self,
        authority: &str,
        client_id: &str,
        refresh_token: &str,
        scopes: &str,
    ) -> Result<std::result::Result<TokenResponse, OAuthError>> {
        self.token(
            authority,
            &[
                ("client_id", client_id),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("scope", scopes),
            ],
        )
    }

    /// Device code sign-in. Shows the code through `notify` and polls until the person
    /// finishes, the code expires or sign-in fails.
    pub fn device_code(
        &self,
        authority: &str,
        client_id: &str,
        scopes: &str,
        notify: Notify<'_>,
    ) -> Result<TokenResponse> {
        let (status, body) = self.post_form(
            &self.url(authority, "devicecode"),
            &[("client_id", client_id), ("scope", scopes)],
        )?;
        if status != 200 {
            return Err(sign_in_failed(&OAuthError::from_body(status, &body)));
        }
        let device_code = body["device_code"]
            .as_str()
            .ok_or_else(|| AuthError::server("device code response without device_code"))?;
        let message = body["message"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| {
                format!(
                    "Open {} and enter the code {}",
                    body["verification_uri"]
                        .as_str()
                        .unwrap_or("https://microsoft.com/devicelogin"),
                    body["user_code"].as_str().unwrap_or("?")
                )
            });
        let mut interval = body["interval"].as_u64().unwrap_or(5);
        let deadline =
            Instant::now() + Duration::from_secs(body["expires_in"].as_u64().unwrap_or(900));
        notify(&message);
        let form = [
            ("client_id", client_id),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", device_code),
        ];
        loop {
            sleep(Duration::from_secs(interval));
            match self.token(authority, &form)? {
                Ok(t) => return Ok(t),
                Err(e) if e.error == "authorization_pending" => {}
                Err(e) if e.error == "slow_down" => interval += 5,
                Err(e) if e.error == "expired_token" => {
                    return Err(AuthError::auth_required(
                        "the device code expired before sign-in finished; run the command again",
                    ));
                }
                Err(e) => return Err(sign_in_failed(&e)),
            }
            if Instant::now() > deadline {
                return Err(AuthError::auth_required(
                    "the device code expired before sign-in finished; run the command again",
                ));
            }
        }
    }

    /// Authorisation code with PKCE: opens `open_browser(url)` and waits up to
    /// `req.timeout` for the redirect to a loopback port. The URL is also shown through
    /// `notify`, to paste by hand if no browser opened.
    pub fn pkce(
        &self,
        req: &PkceRequest<'_>,
        open_browser: &dyn Fn(&str) -> bool,
        notify: Notify<'_>,
    ) -> Result<TokenResponse> {
        let PkceRequest {
            authority,
            client_id,
            scopes,
            login_hint,
            timeout,
        } = *req;
        // The app registers `http://localhost`; Entra accepts any port on a loopback
        // redirect. Listen on IPv4 and, where possible, IPv6 so `localhost` works whichever
        // the browser picks.
        let local =
            |e: std::io::Error| AuthError::internal(format!("can't open a loopback port: {e}"));
        let v4 = TcpListener::bind("127.0.0.1:0").map_err(local)?;
        let port = v4.local_addr().map_err(local)?.port();
        let mut listeners = vec![v4];
        if let Ok(v6) = TcpListener::bind(("::1", port)) {
            listeners.push(v6);
        }
        for l in &listeners {
            l.set_nonblocking(true).map_err(local)?;
        }
        let redirect_uri = format!("http://localhost:{port}");
        let verifier = URL_SAFE_NO_PAD.encode(random::<48>()?);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let state = URL_SAFE_NO_PAD.encode(random::<16>()?);
        let mut url = format!(
            "{}?client_id={}&response_type=code&redirect_uri={}&response_mode=query\
             &scope={}&state={state}&code_challenge={challenge}&code_challenge_method=S256",
            self.url(authority, "authorize"),
            encode(client_id),
            encode(&redirect_uri),
            encode(scopes),
        );
        match login_hint {
            Some(h) => url.push_str(&format!("&login_hint={}", encode(h))),
            None => url.push_str("&prompt=select_account"),
        }
        if open_browser(&url) {
            notify(
                "Opened the browser to sign in. If it didn't open, paste this URL into a browser:",
            );
        } else {
            notify("Open this URL in a browser to sign in:");
        }
        notify(&url);

        let params = wait_for_redirect(&listeners, &state, timeout)?;
        let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        if let Some(error) = get("error") {
            let description = get("error_description")
                .and_then(|d| d.lines().next())
                .unwrap_or("");
            return Err(AuthError::auth_required(format!(
                "sign-in failed: {error}: {description}"
            )));
        }
        let code = get("code").ok_or_else(|| AuthError::server("the redirect had no code"))?;
        let form = [
            ("client_id", client_id),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri.as_str()),
            ("code_verifier", verifier.as_str()),
            ("scope", scopes),
        ];
        self.token(authority, &form)?
            .map_err(|e| sign_in_failed(&e))
    }
}

fn sign_in_failed(e: &OAuthError) -> AuthError {
    let kind = if e.needs_sign_in() || e.status == 400 || e.status == 401 {
        AuthErrorKind::AuthRequired
    } else {
        AuthErrorKind::Server
    };
    AuthError::new(kind, format!("sign-in failed: {e}"))
}

fn wait_for_redirect(
    listeners: &[TcpListener],
    state: &str,
    timeout: Duration,
) -> Result<Vec<(String, String)>> {
    let deadline = Instant::now() + timeout;
    loop {
        for listener in listeners {
            if let Ok((stream, _)) = listener.accept() {
                if let Some(params) = handle_redirect(stream, state) {
                    return Ok(params);
                }
            }
        }
        if Instant::now() > deadline {
            return Err(AuthError::auth_required(
                "timed out waiting for the browser to finish sign-in",
            ));
        }
        sleep(Duration::from_millis(50));
    }
}

/// Answers one request on the loopback port. Returns the query parameters of a redirect
/// that carries our `state`; anything else (a favicon, a stray request) gets a 404 and None.
fn handle_redirect(mut stream: TcpStream, state: &str) -> Option<Vec<(String, String)>> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line).ok()?;
    let target = line.split_whitespace().nth(1).unwrap_or("");
    let params: Option<Vec<(String, String)>> = target.strip_prefix("/?").map(|q| {
        q.split('&')
            .filter_map(|pair| {
                let (k, v) = pair.split_once('=')?;
                Some((decode(k), decode(v)))
            })
            .collect()
    });
    let ours = params
        .as_ref()
        .is_some_and(|p| p.iter().any(|(k, v)| k == "state" && v == state));
    let (status, body) = if ours {
        let failed = params
            .as_ref()
            .is_some_and(|p| p.iter().any(|(k, _)| k == "error"));
        let text = if failed {
            "Familiar: sign-in failed. Go back to the terminal for details."
        } else {
            "Familiar: you're signed in. You can close this tab."
        };
        ("200 OK", text)
    } else {
        ("404 Not Found", "Not found")
    };
    let page = format!(
        "<!doctype html><meta charset=utf-8><title>Familiar</title><p style=\"font:16px system-ui;margin:3em\">{body}</p>"
    );
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
        page.len()
    );
    if ours { params } else { None }
}

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(v) => {
                        out.push(v);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The claims of a JWT, without verifying it. Used on ID tokens only, for the tenant and
/// username; access tokens are opaque to the client (consumer ones are not JWTs).
pub fn jwt_claims(token: &str) -> Option<Value> {
    let payload = URL_SAFE_NO_PAD
        .decode(token.split('.').nth(1)?.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&payload).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode() {
        let s = "a b&c=d/é";
        assert_eq!(decode(&encode(s)), s);
        assert_eq!(decode("a+b%2Fc"), "a b/c");
    }

    #[test]
    fn claims() {
        let t = format!(
            "{}.{}.",
            URL_SAFE_NO_PAD.encode("{}"),
            URL_SAFE_NO_PAD.encode(r#"{"tid":"x","preferred_username":"a@b"}"#)
        );
        assert_eq!(jwt_claims(&t).unwrap()["tid"], "x");
        assert!(jwt_claims("opaque").is_none());
    }
}
