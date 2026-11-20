// SPDX-License-Identifier: MIT OR Apache-2.0
//! Just enough RFC 5322 and MIME to serve Gmail's formats: headers, multipart trees,
//! transfer decoding, addresses and building simple messages.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD};

/// Splits a message or part into unfolded headers and the body.
pub(crate) fn split(raw: &[u8]) -> (Vec<(String, String)>, &[u8]) {
    let (head, body) = match find(raw, b"\r\n\r\n") {
        Some(i) => (&raw[..i], &raw[i + 4..]),
        None => match find(raw, b"\n\n") {
            Some(i) => (&raw[..i], &raw[i + 2..]),
            None => (raw, &raw[raw.len()..]),
        },
    };
    let text = String::from_utf8_lossy(head);
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.starts_with([' ', '\t']) {
            if let Some((_, v)) = headers.last_mut() {
                v.push(' ');
                v.push_str(line.trim());
            }
        } else if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    (headers, body)
}

/// The first header with this name, case-insensitively.
pub(crate) fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// A parameter such as `boundary` or `filename` from a structured header value.
pub(crate) fn param(value: &str, name: &str) -> Option<String> {
    value.split(';').skip(1).find_map(|p| {
        let (k, v) = p.split_once('=')?;
        k.trim()
            .eq_ignore_ascii_case(name)
            .then(|| v.trim().trim_matches('"').to_string())
    })
}

/// The lower-cased media type of a part, `text/plain` when there is none.
pub(crate) fn mime_type(headers: &[(String, String)]) -> String {
    header(headers, "content-type")
        .and_then(|v| v.split(';').next())
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "text/plain".to_string())
}

/// The parts of a multipart body, each with its own headers.
pub(crate) fn multipart<'a>(body: &'a [u8], boundary: &str) -> Vec<&'a [u8]> {
    let delim = format!("--{boundary}");
    let d = delim.as_bytes();
    let mut parts = Vec::new();
    let mut starts = Vec::new();
    let mut i = 0;
    while let Some(off) = find(&body[i..], d) {
        let at = i + off;
        // A delimiter must start a line.
        if at == 0 || body[at - 1] == b'\n' {
            starts.push(at);
        }
        i = at + d.len();
    }
    for w in starts.windows(2) {
        let after = w[0] + d.len();
        if body[after..].starts_with(b"--") {
            break;
        }
        let content_start = match find(&body[after..w[1]], b"\n") {
            Some(n) => after + n + 1,
            None => continue,
        };
        let mut end = w[1];
        if end > content_start && body[end - 1] == b'\n' {
            end -= 1;
        }
        if end > content_start && body[end - 1] == b'\r' {
            end -= 1;
        }
        parts.push(&body[content_start..end.max(content_start)]);
    }
    parts
}

/// Undoes a `Content-Transfer-Encoding`.
pub(crate) fn decode(cte: Option<&str>, body: &[u8]) -> Vec<u8> {
    match cte.map(|c| c.trim().to_ascii_lowercase()).as_deref() {
        Some("base64") => {
            let clean: Vec<u8> = body
                .iter()
                .copied()
                .filter(|b| !b.is_ascii_whitespace())
                .collect();
            STANDARD.decode(&clean).unwrap_or_else(|_| body.to_vec())
        }
        Some("quoted-printable") => quoted_printable(body),
        _ => body.to_vec(),
    }
}

fn quoted_printable(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len());
    let mut i = 0;
    while i < body.len() {
        if body[i] == b'=' {
            if body[i + 1..].starts_with(b"\r\n") {
                i += 3;
                continue;
            }
            if body[i + 1..].starts_with(b"\n") {
                i += 2;
                continue;
            }
            if let Some(hex) = body.get(i + 1..i + 3)
                && let Ok(b) = u8::from_str_radix(&String::from_utf8_lossy(hex), 16)
            {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(body[i]);
        i += 1;
    }
    out
}

/// Lower-cased email addresses from an address-list header such as `To`.
pub(crate) fn addresses(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for ch in value.chars().chain(std::iter::once(',')) {
        match ch {
            '"' => {
                quoted = !quoted;
                cur.push(ch);
            }
            ',' if !quoted => {
                let item = cur.trim();
                let addr = match (item.rfind('<'), item.rfind('>')) {
                    (Some(a), Some(b)) if a < b => &item[a + 1..b],
                    _ => item,
                };
                let addr = addr.trim().to_ascii_lowercase();
                if addr.contains('@') {
                    out.push(addr);
                }
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    out
}

/// Replaces (or adds) a top-level header, keeping the rest of the message as it was.
pub(crate) fn set_header(raw: &[u8], name: &str, value: &str) -> Vec<u8> {
    let (head_end, sep) = match find(raw, b"\r\n\r\n") {
        Some(i) => (i, 4),
        None => match find(raw, b"\n\n") {
            Some(i) => (i, 2),
            None => (raw.len(), 0),
        },
    };
    let head = String::from_utf8_lossy(&raw[..head_end]);
    let mut out = format!("{name}: {value}\r\n");
    let mut skipping = false;
    for line in head.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.starts_with([' ', '\t']) {
            if !skipping {
                out.push_str(line);
                out.push_str("\r\n");
            }
            continue;
        }
        skipping = line
            .split_once(':')
            .is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(name));
        if !skipping && !line.is_empty() {
            out.push_str(line);
            out.push_str("\r\n");
        }
    }
    let mut bytes = out.into_bytes();
    bytes.extend_from_slice(b"\r\n");
    if sep > 0 {
        bytes.extend_from_slice(&raw[head_end + sep..]);
    }
    bytes
}

/// What the mock needs to know about a message, worked out once when it is stored.
pub(crate) struct Summary {
    pub headers: Vec<(String, String)>,
    pub text: String,
    pub has_attachment: bool,
}

pub(crate) fn summarise(raw: &[u8]) -> Summary {
    let (headers, _) = split(raw);
    let mut text = None;
    let mut html = None;
    let mut has_attachment = false;
    walk(raw, &mut |h, body| {
        if filename(h).is_some() {
            has_attachment = true;
            return;
        }
        let t = mime_type(h);
        let decoded = || {
            String::from_utf8_lossy(&decode(header(h, "content-transfer-encoding"), body))
                .into_owned()
        };
        if t == "text/plain" && text.is_none() {
            text = Some(decoded());
        } else if t == "text/html" && html.is_none() {
            html = Some(strip_tags(&decoded()));
        }
    });
    Summary {
        headers,
        text: text.or(html).unwrap_or_default(),
        has_attachment,
    }
}

/// Calls `f` with the headers and raw body of every leaf part.
type Headers = [(String, String)];

fn walk(part: &[u8], f: &mut dyn FnMut(&Headers, &[u8])) {
    let (h, body) = split(part);
    let t = mime_type(&h);
    if t.starts_with("multipart/")
        && let Some(b) = header(&h, "content-type").and_then(|v| param(v, "boundary"))
    {
        for p in multipart(body, &b) {
            walk(p, f);
        }
    } else {
        f(&h, body);
    }
}

/// The attachment file name of a part, from `Content-Disposition` or `Content-Type`.
pub(crate) fn filename(headers: &[(String, String)]) -> Option<String> {
    header(headers, "content-disposition")
        .and_then(|v| param(v, "filename"))
        .or_else(|| header(headers, "content-type").and_then(|v| param(v, "name")))
        .filter(|n| !n.is_empty())
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// Gmail's snippet: the start of the text with whitespace collapsed.
pub(crate) fn snippet(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let joined = words.join(" ");
    joined.chars().take(120).collect()
}

/// Base64url as Gmail sends it.
pub(crate) fn b64url(bytes: &[u8]) -> String {
    URL_SAFE.encode(bytes)
}

/// Base64url (or standard base64) with or without padding, as clients send it.
pub(crate) fn from_b64url(s: &str) -> Option<Vec<u8>> {
    let s: String = s
        .trim()
        .trim_end_matches('=')
        .chars()
        .map(|c| match c {
            '+' => '-',
            '/' => '_',
            c => c,
        })
        .collect();
    URL_SAFE_NO_PAD.decode(s).ok()
}

pub(crate) fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}
