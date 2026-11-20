// SPDX-License-Identifier: MIT OR Apache-2.0
//! Writing messages for Gmail (WP G.7): Gmail takes drafts and mail to send
//! as whole RFC 5322 messages, so Familiar builds the MIME itself.
//!
//! A draft update replaces the whole message, so [`Outgoing::from_raw`]
//! reads the draft Gmail has (its recipients, threading headers and
//! attachments) for the update to change.

use mail_builder::MessageBuilder;
use mail_builder::headers::address::Address as MbAddress;
use mail_builder::headers::raw::Raw;
use mail_parser::{MessageParser, MimeHeaders};

use crate::mime::body_from_raw;

/// A file in a message.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Part {
    /// File name.
    pub name: String,
    /// MIME type, e.g. `image/png`.
    pub content_type: String,
    /// Shown in the body as `cid:<content_id>`.
    pub inline: bool,
    /// Content ID of an inline image, without `<>`.
    pub content_id: Option<String>,
    /// The bytes.
    pub data: Vec<u8>,
}

/// A name and address.
pub type Mailbox = (String, String);

/// A message to save as a draft or send.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outgoing {
    /// From.
    pub from: Mailbox,
    /// To.
    pub to: Vec<Mailbox>,
    /// Cc.
    pub cc: Vec<Mailbox>,
    /// Bcc (Gmail sends to them and takes the header off).
    pub bcc: Vec<Mailbox>,
    /// Subject.
    pub subject: String,
    /// The HTML body.
    pub html: String,
    /// `low`, `normal` or `high`.
    pub importance: Option<String>,
    /// The Message-ID answered, without `<>`.
    pub in_reply_to: Option<String>,
    /// The thread's Message-IDs, oldest first, without `<>`.
    pub references: Vec<String>,
    /// Files.
    pub parts: Vec<Part>,
}

fn bare(id: &str) -> String {
    id.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_string()
}

fn mailboxes(list: Option<&mail_parser::Address<'_>>) -> Vec<Mailbox> {
    let Some(list) = list else { return Vec::new() };
    list.iter()
        .filter_map(|a| {
            Some((
                a.name.as_deref().unwrap_or_default().to_string(),
                a.address.as_deref()?.to_string(),
            ))
        })
        .collect()
}

fn ids(v: &mail_parser::HeaderValue<'_>) -> Vec<String> {
    match v {
        mail_parser::HeaderValue::Text(t) => vec![bare(t)],
        mail_parser::HeaderValue::TextList(l) => l.iter().map(|t| bare(t)).collect(),
        _ => Vec::new(),
    }
}

/// The files in an RFC 5322 message, inline images included.
pub fn parts_of(raw: &[u8]) -> Vec<Part> {
    let Some(m) = MessageParser::default().parse(raw) else {
        return Vec::new();
    };
    m.attachments()
        .map(|p| {
            let ct = p.content_type().map_or_else(
                || "application/octet-stream".to_string(),
                |c| match c.subtype() {
                    Some(s) => format!("{}/{s}", c.ctype()),
                    None => c.ctype().to_string(),
                },
            );
            let content_id = p.content_id().map(bare);
            Part {
                name: p.attachment_name().unwrap_or("attachment").to_string(),
                content_type: ct,
                inline: content_id.is_some()
                    && !p.content_disposition().is_some_and(|d| d.is_attachment()),
                content_id,
                data: p.contents().to_vec(),
            }
        })
        .collect()
}

/// The Message-ID of an RFC 5322 message, without `<>`.
pub fn message_id(raw: &[u8]) -> Option<String> {
    MessageParser::default()
        .parse_headers(raw)?
        .message_id()
        .map(bare)
}

impl Outgoing {
    /// Reads a message Gmail has (a draft, to update it).
    pub fn from_raw(raw: &[u8]) -> Self {
        let body = body_from_raw(raw);
        let html = if body.is_html {
            body.content
        } else {
            format!("<pre>{}</pre>", html_escape(&body.content))
        };
        let Some(m) = MessageParser::default().parse(raw) else {
            return Self {
                html,
                ..Self::default()
            };
        };
        let importance = m
            .header("Importance")
            .and_then(|h| h.as_text())
            .map(|s| s.trim().to_ascii_lowercase());
        Self {
            from: mailboxes(m.from()).into_iter().next().unwrap_or_default(),
            to: mailboxes(m.to()),
            cc: mailboxes(m.cc()),
            bcc: mailboxes(m.bcc()),
            subject: m.subject().unwrap_or_default().to_string(),
            html,
            importance,
            in_reply_to: ids(m.in_reply_to()).into_iter().next(),
            references: ids(m.references()),
            parts: parts_of(raw),
        }
    }

    /// The message as RFC 5322 bytes.
    pub fn build(&self) -> Vec<u8> {
        let mut b = MessageBuilder::new()
            .from(one(&self.from))
            .subject(self.subject.as_str())
            .html_body(self.html.as_str());
        if !self.to.is_empty() {
            b = b.to(list(&self.to));
        }
        if !self.cc.is_empty() {
            b = b.cc(list(&self.cc));
        }
        if !self.bcc.is_empty() {
            b = b.bcc(list(&self.bcc));
        }
        if let Some(r) = &self.in_reply_to {
            b = b.in_reply_to(r.as_str());
        }
        if !self.references.is_empty() {
            let refs: Vec<&str> = self.references.iter().map(String::as_str).collect();
            b = b.references(refs);
        }
        match self.importance.as_deref() {
            Some("high") => {
                b = b
                    .header("Importance", Raw::new("high"))
                    .header("X-Priority", Raw::new("1"));
            }
            Some("low") => {
                b = b
                    .header("Importance", Raw::new("low"))
                    .header("X-Priority", Raw::new("5"));
            }
            _ => {}
        }
        for p in &self.parts {
            let data: &[u8] = &p.data;
            b = match (&p.content_id, p.inline) {
                (Some(cid), true) => b.inline(p.content_type.as_str(), cid.as_str(), data),
                _ => b.attachment(p.content_type.as_str(), p.name.as_str(), data),
            };
        }
        b.write_to_vec().unwrap_or_default()
    }
}

fn list(v: &[Mailbox]) -> MbAddress<'_> {
    MbAddress::new_list(v.iter().map(one).collect())
}

fn one(m: &Mailbox) -> MbAddress<'_> {
    MbAddress::new_address(Some(m.0.as_str()).filter(|n| !n.is_empty()), m.1.as_str())
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_and_reads_back() {
        let o = Outgoing {
            from: ("Kea".into(), "kea@example.com".into()),
            to: vec![("Ann Acme".into(), "ann@acme.example".into())],
            cc: vec![],
            bcc: vec![(String::new(), "secret@example.com".into())],
            subject: "Re: Quote – café".into(),
            html: "<p>Thanks <img src=\"cid:logo\"></p>".into(),
            importance: Some("high".into()),
            in_reply_to: Some("acme-1@acme.example".into()),
            references: vec!["acme-1@acme.example".into()],
            parts: vec![
                Part {
                    name: "logo.png".into(),
                    content_type: "image/png".into(),
                    inline: true,
                    content_id: Some("logo".into()),
                    data: b"\x89PNG".to_vec(),
                },
                Part {
                    name: "quote.pdf".into(),
                    content_type: "application/pdf".into(),
                    inline: false,
                    content_id: None,
                    data: b"%PDF-1.4".to_vec(),
                },
            ],
        };
        let raw = o.build();
        let back = Outgoing::from_raw(&raw);
        assert_eq!(back.from, o.from);
        assert_eq!(back.to, o.to);
        assert_eq!(back.bcc, o.bcc);
        assert_eq!(back.subject, o.subject);
        assert!(back.html.contains("Thanks"), "{}", back.html);
        assert_eq!(back.importance.as_deref(), Some("high"));
        assert_eq!(back.in_reply_to, o.in_reply_to);
        assert_eq!(back.references, o.references);
        assert_eq!(back.parts.len(), 2, "{:?}", back.parts);
        assert!(message_id(&raw).is_some_and(|m| !m.contains('<')));
        let pdf = back.parts.iter().find(|p| p.name == "quote.pdf").unwrap();
        assert_eq!(pdf.data, b"%PDF-1.4");
        assert!(!pdf.inline);
        let logo = back.parts.iter().find(|p| p.content_id.is_some()).unwrap();
        assert!(logo.inline);
        assert_eq!(logo.data, b"\x89PNG");
    }
}
