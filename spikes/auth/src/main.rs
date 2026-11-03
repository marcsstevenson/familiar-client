// SPDX-License-Identifier: MIT OR Apache-2.0
//
// WP 0.2 step B: throwaway auth spike. Signs in with authorisation code + PKCE
// (system browser, loopback redirect) or device code, calls a few Graph
// endpoints and refreshes the token once. Prints ok/failed with HTTP status and
// Graph error code only. Tokens stay in memory and are never printed or saved.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::ExitCode;
use std::thread::sleep;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};

const LOGIN: &str = "https://login.microsoftonline.com";
const GRAPH: &str = "https://graph.microsoft.com/v1.0";
const FULL_SCOPES: &str = "openid profile offline_access User.Read Mail.ReadWrite Mail.Send \
                           Calendars.ReadWrite Contacts.Read MailboxSettings.ReadWrite";
const READ_SCOPES: &str = "openid profile offline_access User.Read Mail.Read Calendars.Read";

const GRAPH_CALLS: &[(&str, &str)] = &[
    ("me", "/me?$select=id"),
    ("mailFolders", "/me/mailFolders?$top=5&$select=id"),
    ("messages", "/me/messages?$top=1&$select=id"),
    ("calendars", "/me/calendars?$select=id"),
    ("mailboxSettings", "/me/mailboxSettings"),
];

const USAGE: &str = "\
Usage: auth-spike --account hotmail|parklands --flow pkce|device [--client-id ID] [--read-only]

  --account    hotmail uses the `consumers` authority, parklands uses `organizations`
  --flow       pkce opens the system browser; device prints a code to enter at microsoft.com/devicelogin
  --client-id  the Familiar app's Application (client) ID; defaults to $FAMILIAR_CLIENT_ID
  --read-only  ask for User.Read offline_access Mail.Read Calendars.Read only";

#[derive(PartialEq)]
enum Flow {
    Pkce,
    Device,
}

struct Args {
    authority: &'static str,
    account: String,
    flow: Flow,
    client_id: String,
    scopes: &'static str,
}

fn parse_args() -> Result<Args, String> {
    let mut account = None;
    let mut flow = None;
    let mut client_id = std::env::var("FAMILIAR_CLIENT_ID").ok();
    let mut scopes = FULL_SCOPES;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--account" => account = it.next(),
            "--flow" => flow = it.next(),
            "--client-id" => client_id = it.next(),
            "--read-only" => scopes = READ_SCOPES,
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    let account = account.ok_or("--account is required")?;
    let authority = match account.as_str() {
        "hotmail" => "consumers",
        "parklands" => "organizations",
        other => return Err(format!("unknown account `{other}`")),
    };
    let flow = match flow.as_deref() {
        Some("pkce") => Flow::Pkce,
        Some("device") => Flow::Device,
        Some(other) => return Err(format!("unknown flow `{other}`")),
        None => return Err("--flow is required".into()),
    };
    let client_id = client_id
        .filter(|id| !id.trim().is_empty())
        .ok_or("no client ID: pass --client-id or set FAMILIAR_CLIENT_ID")?;
    Ok(Args { authority, account, flow, client_id: client_id.trim().to_string(), scopes })
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(msg) => {
            if !msg.is_empty() {
                eprintln!("error: {msg}\n");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let flow_name = if args.flow == Flow::Pkce { "pkce" } else { "device" };
    let scope_name = if args.scopes == FULL_SCOPES { "full" } else { "read-only" };
    println!("account={} authority={} flow={flow_name} scopes={scope_name}", args.account, args.authority);

    let sign_in = match args.flow {
        Flow::Pkce => sign_in_pkce(&args),
        Flow::Device => sign_in_device(&args),
    };
    let tokens = match sign_in {
        Ok(tokens) => {
            println!("sign-in          ok");
            tokens
        }
        Err(e) => {
            println!("sign-in          failed  {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut all_ok = true;
    for (name, path) in GRAPH_CALLS {
        all_ok &= graph_call(name, path, &tokens.access);
    }

    match tokens.refresh.as_deref() {
        None => {
            println!("refresh          failed  no refresh_token returned (offline_access not granted?)");
            all_ok = false;
        }
        Some(refresh) => {
            let form = [
                ("client_id", args.client_id.as_str()),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh),
                ("scope", args.scopes),
            ];
            match token_request(args.authority, &form) {
                Ok(Ok(new)) => {
                    println!("refresh          ok");
                    all_ok &= graph_call("me (refreshed)", "/me?$select=id", &new.access);
                }
                Ok(Err(e)) => {
                    println!("refresh          failed  {e}");
                    all_ok = false;
                }
                Err(e) => {
                    println!("refresh          failed  {e}");
                    all_ok = false;
                }
            }
        }
    }

    if all_ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

struct Tokens {
    access: String,
    refresh: Option<String>,
}

/// Outer error: transport or unexpected response. Inner error: the token
/// endpoint's OAuth error, kept separate so device code polling can read it.
fn token_request(authority: &str, form: &[(&str, &str)]) -> Result<Result<Tokens, OAuthError>, String> {
    let url = format!("{LOGIN}/{authority}/oauth2/v2.0/token");
    let (status, body) = post_form(&url, form)?;
    if status == 200 {
        let access = body["access_token"].as_str().ok_or("200 without access_token")?.to_string();
        let refresh = body["refresh_token"].as_str().map(str::to_string);
        Ok(Ok(Tokens { access, refresh }))
    } else {
        Ok(Err(OAuthError::from_body(status, &body)))
    }
}

struct OAuthError {
    status: u16,
    error: String,
    description: String,
}

impl OAuthError {
    fn from_body(status: u16, body: &Value) -> Self {
        let error = body["error"].as_str().unwrap_or("unknown").to_string();
        // The first line holds the AADSTS code and message; the rest is trace and correlation IDs.
        let description =
            body["error_description"].as_str().and_then(|d| d.lines().next()).unwrap_or("").to_string();
        OAuthError { status, error, description }
    }
}

impl std::fmt::Display for OAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {} {}: {}", self.status, self.error, self.description)
    }
}

impl From<OAuthError> for String {
    fn from(e: OAuthError) -> Self {
        e.to_string()
    }
}

fn post_form(url: &str, form: &[(&str, &str)]) -> Result<(u16, Value), String> {
    let response = match ureq::post(url).send_form(form) {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(e) => return Err(format!("request failed: {e}")),
    };
    let status = response.status();
    let body = response.into_json::<Value>().map_err(|e| format!("HTTP {status}, unreadable body: {e}"))?;
    Ok((status, body))
}

fn graph_call(name: &str, path: &str, access: &str) -> bool {
    let response = match ureq::get(&format!("{GRAPH}{path}")).set("Authorization", &format!("Bearer {access}")).call() {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(e) => {
            println!("{name:<16} failed  request failed: {e}");
            return false;
        }
    };
    let status = response.status();
    let body = response.into_json::<Value>().unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        // Field names only, never values, so the signature question can be answered without personal data.
        let detail = match name {
            "mailboxSettings" => {
                let keys: Vec<&str> =
                    body.as_object().map(|o| o.keys().map(String::as_str).collect()).unwrap_or_default();
                format!("  fields: {}", keys.join(", "))
            }
            _ => String::new(),
        };
        println!("{name:<16} ok      HTTP {status}{detail}");
        true
    } else {
        let code = body["error"]["code"].as_str().unwrap_or("-");
        println!("{name:<16} failed  HTTP {status} {code}");
        false
    }
}

fn random_url_safe(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf).expect("OS random source");
    URL_SAFE_NO_PAD.encode(buf)
}

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
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

fn sign_in_pkce(args: &Args) -> Result<Tokens, String> {
    // The app registers `http://localhost`; Entra accepts any port on a loopback redirect.
    // Listen on IPv4 and, where possible, IPv6 so `localhost` works whichever the browser picks.
    let v4 = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("can't open loopback port: {e}"))?;
    let port = v4.local_addr().map_err(|e| e.to_string())?.port();
    let mut listeners = vec![v4];
    if let Ok(v6) = TcpListener::bind(("::1", port)) {
        listeners.push(v6);
    }
    for l in &listeners {
        l.set_nonblocking(true).map_err(|e| e.to_string())?;
    }
    let redirect_uri = format!("http://localhost:{port}");

    let verifier = random_url_safe(48);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_url_safe(16);
    let url = format!(
        "{LOGIN}/{}/oauth2/v2.0/authorize?client_id={}&response_type=code&redirect_uri={}&response_mode=query\
         &scope={}&state={state}&code_challenge={challenge}&code_challenge_method=S256&prompt=select_account",
        args.authority,
        encode(&args.client_id),
        encode(&redirect_uri),
        encode(args.scopes),
    );
    println!("Opening the browser to sign in. If it doesn't open, paste this URL into a private window:\n{url}\n");
    if webbrowser::open(&url).is_err() {
        println!("(couldn't open a browser automatically)");
    }

    let params = wait_for_redirect(&listeners, Duration::from_secs(300))?;
    let get = |k: &str| params.iter().find(|(name, _)| name == k).map(|(_, v)| v.as_str());
    if let Some(error) = get("error") {
        let description = get("error_description").and_then(|d| d.lines().next()).unwrap_or("");
        return Err(format!("authorize returned {error}: {description}"));
    }
    if get("state") != Some(state.as_str()) {
        return Err("state mismatch on redirect".into());
    }
    let code = get("code").ok_or("redirect had no code")?;

    let form = [
        ("client_id", args.client_id.as_str()),
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri.as_str()),
        ("code_verifier", verifier.as_str()),
        ("scope", args.scopes),
    ];
    Ok(token_request(args.authority, &form)??)
}

fn wait_for_redirect(listeners: &[TcpListener], timeout: Duration) -> Result<Vec<(String, String)>, String> {
    let deadline = Instant::now() + timeout;
    loop {
        for listener in listeners {
            if let Ok((stream, _)) = listener.accept() {
                if let Some(params) = handle_redirect(stream) {
                    return Ok(params);
                }
            }
        }
        if Instant::now() > deadline {
            return Err("timed out waiting for the browser redirect".into());
        }
        sleep(Duration::from_millis(100));
    }
}

/// Returns the query parameters of a `GET /?...` request, or None for anything
/// else (such as a favicon request), which gets a 404.
fn handle_redirect(mut stream: TcpStream) -> Option<Vec<(String, String)>> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line).ok()?;
    let target = line.split_whitespace().nth(1).unwrap_or("");
    let query = target.strip_prefix("/?");
    let (status, body) = match query {
        Some(_) => ("200 OK", "Familiar auth spike: you can close this window and go back to the terminal."),
        None => ("404 Not Found", "Not found"),
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let params = query?
        .split('&')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            Some((decode(k), decode(v)))
        })
        .collect();
    Some(params)
}

fn sign_in_device(args: &Args) -> Result<Tokens, String> {
    let url = format!("{LOGIN}/{}/oauth2/v2.0/devicecode", args.authority);
    let (status, body) = post_form(&url, &[("client_id", args.client_id.as_str()), ("scope", args.scopes)])?;
    if status != 200 {
        return Err(OAuthError::from_body(status, &body).into());
    }
    let device_code = body["device_code"].as_str().ok_or("devicecode response had no device_code")?;
    let message = body["message"].as_str().unwrap_or("Enter the code shown at https://microsoft.com/devicelogin");
    let mut interval = body["interval"].as_u64().unwrap_or(5);
    let expires_in = body["expires_in"].as_u64().unwrap_or(900);
    println!("{message}\n");

    let deadline = Instant::now() + Duration::from_secs(expires_in);
    let form = [
        ("client_id", args.client_id.as_str()),
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ("device_code", device_code),
    ];
    loop {
        sleep(Duration::from_secs(interval));
        match token_request(args.authority, &form)? {
            Ok(tokens) => return Ok(tokens),
            Err(e) if e.error == "authorization_pending" => {}
            Err(e) if e.error == "slow_down" => interval += 5,
            Err(e) => return Err(e.into()),
        }
        if Instant::now() > deadline {
            return Err("device code expired before sign-in finished".into());
        }
    }
}
