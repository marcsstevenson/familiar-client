// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Gmail plan WP G.0: throwaway Google sign-in spike. Signs in with
// authorisation code + PKCE (system browser, loopback redirect), calls a few
// Gmail, Calendar and People endpoints, then refreshes the token once.
// Prints ok/failed with the HTTP status and Google's error reason, plus
// counts only (never names, addresses or subjects). Tokens stay in memory and
// are never printed or saved.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::ExitCode;
use std::thread::sleep;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};

const AUTHORIZE: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN: &str = "https://oauth2.googleapis.com/token";
const GMAIL: &str = "https://gmail.googleapis.com/gmail/v1/users/me";
const CALENDAR: &str = "https://www.googleapis.com/calendar/v3";
const PEOPLE: &str = "https://people.googleapis.com/v1";

/// Everything the Gmail plan asks for, so one consent covers mail now and
/// calendar and contacts later.
const SCOPES: &str = "openid email profile \
    https://www.googleapis.com/auth/gmail.modify \
    https://www.googleapis.com/auth/gmail.send \
    https://www.googleapis.com/auth/gmail.settings.basic \
    https://www.googleapis.com/auth/calendar \
    https://www.googleapis.com/auth/contacts.readonly \
    https://www.googleapis.com/auth/contacts.other.readonly";

const USAGE: &str = "\
Usage: google-auth-spike [--client-file PATH]

  --client-file  the OAuth client JSON downloaded from Google Cloud (Desktop app);
                 defaults to %USERPROFILE%\\.familiar\\google-client.json
                 (or ~/.familiar/google-client.json)";

struct Client {
    id: String,
    secret: String,
}

fn default_client_file() -> String {
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    format!("{home}/.familiar/google-client.json")
}

fn load_client() -> Result<Client, String> {
    let mut path = default_client_file();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--client-file" => path = it.next().ok_or("--client-file needs a path")?,
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("can't read {path}: {e}"))?;
    let json: Value = serde_json::from_str(&text).map_err(|e| format!("{path} isn't JSON: {e}"))?;
    let installed = json
        .get("installed")
        .ok_or("the client file has no `installed` section: create the client as a Desktop app")?;
    let field = |k: &str| installed[k].as_str().map(str::to_string).ok_or(format!("the client file has no {k}"));
    Ok(Client { id: field("client_id")?, secret: field("client_secret")? })
}

fn main() -> ExitCode {
    let client = match load_client() {
        Ok(c) => c,
        Err(msg) => {
            if !msg.is_empty() {
                eprintln!("error: {msg}\n");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    let tokens = match sign_in(&client) {
        Ok(t) => {
            println!("sign-in            ok      granted: {}", short_scopes(&t.scope));
            t
        }
        Err(e) => {
            println!("sign-in            failed  {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut all_ok = run_checks(&tokens.access);

    match tokens.refresh.as_deref() {
        None => {
            println!("refresh            failed  no refresh_token returned");
            all_ok = false;
        }
        Some(refresh) => {
            let form = [
                ("client_id", client.id.as_str()),
                ("client_secret", client.secret.as_str()),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh),
            ];
            match token_request(&form) {
                Ok(new) => {
                    println!("refresh            ok      granted: {}", short_scopes(&new.scope));
                    all_ok &= get("profile (again)", &format!("{GMAIL}/profile"), &new.access).is_some();
                }
                Err(e) => {
                    println!("refresh            failed  {e}");
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

/// One call per API the plan uses. Prints counts only.
fn run_checks(access: &str) -> bool {
    let mut ok = true;

    let profile = get("profile", &format!("{GMAIL}/profile"), access);
    ok &= profile.is_some();
    let history_id = profile.as_ref().and_then(|p| p["historyId"].as_str().map(str::to_string));
    if let Some(p) = &profile {
        println!(
            "                           messages: {}, threads: {}",
            p["messagesTotal"].as_u64().unwrap_or(0),
            p["threadsTotal"].as_u64().unwrap_or(0)
        );
    }

    if let Some(labels) = get("labels", &format!("{GMAIL}/labels"), access) {
        let all = labels["labels"].as_array().cloned().unwrap_or_default();
        let user = all.iter().filter(|l| l["type"] == "user").count();
        println!("                           system: {}, user: {user}", all.len() - user);
    } else {
        ok = false;
    }

    match &history_id {
        Some(id) => ok &= get("history", &format!("{GMAIL}/history?startHistoryId={id}&maxResults=1"), access).is_some(),
        None => println!("history            skipped no historyId from profile"),
    }

    ok &= get("messages.list", &format!("{GMAIL}/messages?maxResults=1"), access).is_some();

    if let Some(send_as) = get("sendAs", &format!("{GMAIL}/settings/sendAs"), access) {
        let all = send_as["sendAs"].as_array().cloned().unwrap_or_default();
        let signed = all.iter().filter(|s| !s["signature"].as_str().unwrap_or("").is_empty()).count();
        println!("                           addresses: {}, with a signature: {signed}", all.len());
    } else {
        ok = false;
    }

    if let Some(cals) = get("calendarList", &format!("{CALENDAR}/users/me/calendarList"), access) {
        println!("                           calendars: {}", cals["items"].as_array().map_or(0, Vec::len));
    } else {
        ok = false;
    }

    if let Some(people) = get(
        "contacts",
        &format!("{PEOPLE}/people/me/connections?personFields=names&pageSize=1"),
        access,
    ) {
        println!("                           total: {}", people["totalPeople"].as_u64().unwrap_or(0));
    } else {
        ok = false;
    }

    ok &= get("otherContacts", &format!("{PEOPLE}/otherContacts?readMask=emailAddresses&pageSize=1"), access).is_some();

    ok
}

/// The granted scopes without the long URL prefix.
fn short_scopes(scope: &str) -> String {
    scope
        .split_whitespace()
        .map(|s| s.trim_start_matches("https://www.googleapis.com/auth/"))
        .collect::<Vec<_>>()
        .join(" ")
}

struct Tokens {
    access: String,
    refresh: Option<String>,
    scope: String,
}

fn token_request(form: &[(&str, &str)]) -> Result<Tokens, String> {
    let response = match ureq::post(TOKEN).send_form(form) {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(e) => return Err(format!("request failed: {e}")),
    };
    let status = response.status();
    let body = response.into_json::<Value>().map_err(|e| format!("HTTP {status}, unreadable body: {e}"))?;
    if status != 200 {
        let error = body["error"].as_str().unwrap_or("unknown");
        let description = body["error_description"].as_str().unwrap_or("");
        return Err(format!("HTTP {status} {error}: {description}"));
    }
    Ok(Tokens {
        access: body["access_token"].as_str().ok_or("200 without access_token")?.to_string(),
        refresh: body["refresh_token"].as_str().map(str::to_string),
        scope: body["scope"].as_str().unwrap_or("-").to_string(),
    })
}

/// GET a Google API URL; prints ok or failed with the status and error reason.
fn get(name: &str, url: &str, access: &str) -> Option<Value> {
    let response = match ureq::get(url).set("Authorization", &format!("Bearer {access}")).call() {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(e) => {
            println!("{name:<18} failed  request failed: {e}");
            return None;
        }
    };
    let status = response.status();
    let body = response.into_json::<Value>().unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        println!("{name:<18} ok      HTTP {status}");
        Some(body)
    } else {
        let reason = body["error"]["errors"][0]["reason"]
            .as_str()
            .or_else(|| body["error"]["status"].as_str())
            .unwrap_or("-");
        println!("{name:<18} failed  HTTP {status} {reason}");
        None
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

fn sign_in(client: &Client) -> Result<Tokens, String> {
    // Google accepts any port on a loopback redirect for Desktop clients.
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("can't open loopback port: {e}"))?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}");

    let verifier = random_url_safe(48);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_url_safe(16);
    // access_type=offline and prompt=consent make Google return a refresh token every time.
    let url = format!(
        "{AUTHORIZE}?client_id={}&response_type=code&redirect_uri={}&scope={}&state={state}\
         &code_challenge={challenge}&code_challenge_method=S256&access_type=offline&prompt=consent",
        encode(&client.id),
        encode(&redirect_uri),
        encode(SCOPES),
    );
    println!("Opening the browser to sign in. If it doesn't open, paste this URL into a private window:\n{url}\n");
    if webbrowser::open(&url).is_err() {
        println!("(couldn't open a browser automatically)");
    }

    let params = wait_for_redirect(&listener, Duration::from_secs(300))?;
    let param = |k: &str| params.iter().find(|(name, _)| name == k).map(|(_, v)| v.as_str());
    if let Some(error) = param("error") {
        return Err(format!("authorize returned {error}"));
    }
    if param("state") != Some(state.as_str()) {
        return Err("state mismatch on redirect".into());
    }
    let code = param("code").ok_or("redirect had no code")?;

    token_request(&[
        ("client_id", client.id.as_str()),
        ("client_secret", client.secret.as_str()),
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri.as_str()),
        ("code_verifier", verifier.as_str()),
    ])
}

fn wait_for_redirect(listener: &TcpListener, timeout: Duration) -> Result<Vec<(String, String)>, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok((stream, _)) = listener.accept() {
            if let Some(params) = handle_redirect(stream) {
                return Ok(params);
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
        Some(_) => ("200 OK", "Familiar Google spike: you can close this window and go back to the terminal."),
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
