// SPDX-License-Identifier: MIT OR Apache-2.0
//! Reading Gmail's messages: headers from `metadata`, bodies from `raw`.

use familiar_store::{AccountId, Address, FolderId, NewMessage};
use mail_parser::{MessageParser, PartType};

use crate::model::GmailMessage;

/// A message's body as the store keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Body {
    /// The HTML, or plain text when the message has no HTML part.
    pub content: String,
    /// Whether `content` is HTML.
    pub is_html: bool,
}

/// The body of an RFC 5322 message: its HTML parts if it has any, else its
/// text parts. Attachments are left out.
pub fn body_from_raw(raw: &[u8]) -> Body {
    let Some(m) = MessageParser::default().parse(raw) else {
        return Body {
            content: String::from_utf8_lossy(raw).into_owned(),
            is_html: false,
        };
    };
    let has_html = m.html_bodies().any(|p| matches!(p.body, PartType::Html(_)));
    let parts = if has_html {
        m.html_body_count()
    } else {
        m.text_body_count()
    };
    let content = (0..parts)
        .filter_map(|i| {
            if has_html {
                m.body_html(i)
            } else {
                m.body_text(i)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    Body {
        content,
        is_html: has_html,
    }
}

fn addresses(list: Option<&mail_parser::Address<'_>>) -> Vec<Address> {
    let Some(list) = list else { return Vec::new() };
    list.iter()
        .filter_map(|m| {
            Some(Address {
                name: m.name.as_deref().unwrap_or_default().to_string(),
                addr: m.address.as_deref()?.to_string(),
            })
        })
        .collect()
}

/// Gmail's snippets are HTML-escaped.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        // Entities are ASCII: look for the `;` byte, so a multibyte
        // character after a bare `&` can't split a slice.
        let Some(end) = rest.bytes().take(10).position(|b| b == b';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => ent
                .strip_prefix("#x")
                .or_else(|| ent.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// 0 low, 1 normal, 2 high, from `Importance` or `X-Priority`.
fn importance(m: &GmailMessage) -> i64 {
    if let Some(i) = m.header("Importance") {
        match i.trim().to_ascii_lowercase().as_str() {
            "high" => return 2,
            "low" => return 0,
            _ => {}
        }
    }
    match m.header("X-Priority").and_then(|p| p.trim().chars().next()) {
        Some('1' | '2') => 2,
        Some('4' | '5') => 0,
        _ => 1,
    }
}

/// The store's header row for a `metadata` message in `folder`.
pub fn to_new_message(m: &GmailMessage, account: AccountId, folder: FolderId) -> NewMessage {
    // Gmail hands the headers back as written; parse them as a header block
    // so encoded words and address lists come out as for any message.
    let mut block = String::new();
    if let Some(p) = &m.payload {
        for h in &p.headers {
            if !h.value.contains(['\r', '\n']) {
                block.push_str(&h.name);
                block.push_str(": ");
                block.push_str(&h.value);
                block.push_str("\r\n");
            }
        }
    }
    block.push_str("\r\n");
    let parsed = MessageParser::default().parse(block.as_bytes());
    let received_at = m
        .internal_date
        .as_deref()
        .and_then(|d| d.parse::<i64>().ok())
        .map_or(0, |ms| ms.div_euclid(1000));
    let (from, to, cc, subject, sent_at, imid) = match &parsed {
        Some(p) => (
            addresses(p.from()).into_iter().next().unwrap_or_default(),
            addresses(p.to()),
            addresses(p.cc()),
            p.subject().unwrap_or_default().to_string(),
            p.date().map(|d| d.to_timestamp()),
            p.message_id().map(|id| format!("<{id}>")),
        ),
        None => Default::default(),
    };
    let mime = m
        .payload
        .as_ref()
        .map(|p| p.mime_type.to_ascii_lowercase())
        .unwrap_or_default();
    NewMessage {
        account,
        folder,
        remote_id: m.id.clone(),
        internet_message_id: imid,
        conversation_id: if m.thread_id.is_empty() {
            format!("familiar:{}", m.id)
        } else {
            m.thread_id.clone()
        },
        subject,
        from,
        to,
        cc,
        received_at,
        sent_at,
        is_read: !m.has("UNREAD"),
        is_flagged: m.has("STARRED"),
        importance: importance(m),
        has_attachments: mime == "multipart/mixed",
        preview: unescape(&m.snippet),
        body: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Header, Payload};

    #[test]
    fn snippets_are_unescaped() {
        assert_eq!(unescape("it&#39;s &lt;b&gt; &amp; more"), "it's <b> & more");
        assert_eq!(unescape("AT&T &unknown; &#x263A;"), "AT&T &unknown; ☺");
        assert_eq!(unescape("&éééé &amp;é"), "&éééé &é");
    }

    #[test]
    fn headers_become_a_row() {
        let h = |n: &str, v: &str| Header {
            name: n.into(),
            value: v.into(),
        };
        let m = GmailMessage {
            id: "m1".into(),
            thread_id: "t1".into(),
            label_ids: vec!["INBOX".into(), "UNREAD".into(), "STARRED".into()],
            snippet: "Hi &amp; bye".into(),
            internal_date: Some("1700000000123".into()),
            payload: Some(Payload {
                mime_type: "multipart/mixed".into(),
                headers: vec![
                    h("From", "=?UTF-8?Q?Ann_Sm=C3=AEth?= <ann@example.com>"),
                    h("To", "bob@example.com, \"Cy\" <cy@example.com>"),
                    h("Subject", "Lunch"),
                    h("Date", "Tue, 14 Nov 2023 22:13:20 +0000"),
                    h("Message-ID", "<abc@example.com>"),
                    h("X-Priority", "1"),
                ],
            }),
            raw: None,
        };
        let r = to_new_message(&m, AccountId(1), FolderId(2));
        assert_eq!(r.from.name, "Ann Smîth");
        assert_eq!(r.from.addr, "ann@example.com");
        assert_eq!(r.to.len(), 2);
        assert_eq!(r.to[1].name, "Cy");
        assert_eq!(r.subject, "Lunch");
        assert_eq!(r.received_at, 1_700_000_000);
        assert_eq!(r.sent_at, Some(1_700_000_000));
        assert_eq!(r.internet_message_id.as_deref(), Some("<abc@example.com>"));
        assert_eq!(r.conversation_id, "t1");
        assert!(!r.is_read && r.is_flagged && r.has_attachments);
        assert_eq!(r.importance, 2);
        assert_eq!(r.preview, "Hi & bye");
    }

    #[test]
    fn bodies_prefer_html() {
        let raw = b"Content-Type: multipart/alternative; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nplain\r\n--b\r\nContent-Type: text/html\r\n\r\n<p>html</p>\r\n--b--\r\n";
        let b = body_from_raw(raw);
        assert!(b.is_html);
        assert!(b.content.contains("<p>html</p>"));
        let b = body_from_raw(b"Subject: x\r\n\r\njust text\r\n");
        assert!(!b.is_html);
        assert_eq!(b.content.trim(), "just text");
    }
}
