// SPDX-License-Identifier: MIT OR Apache-2.0
//! The HTML sanitiser for the reading pane.
//!
//! Every body passes through here before the webview sees it. Scripts, event
//! handlers, forms, frames, `<meta>`, `<base>` and `javascript:` links are
//! removed by ammonia's allow list. Remote images are dropped (and counted) unless
//! the person has allowed them for this message or sender; allowed ones are
//! rewritten to the app's `famimg` scheme, so the webview itself never talks
//! to a remote server. `cid:` images become `data:` URIs from the message's
//! inline attachments.
//!
//! The result is a whole document with its own Content-Security-Policy. The
//! frame it is shown in is sandboxed without scripts, so the CSP and the
//! sandbox stand behind the sanitiser if it ever misses something.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// The URL prefix the image proxy answers on (see `remote.rs`). Windows
/// webviews serve custom schemes as `http://<scheme>.localhost`.
#[cfg(windows)]
pub const IMAGE_PROXY: &str = "http://famimg.localhost/";
/// The URL prefix the image proxy answers on (see `remote.rs`).
#[cfg(not(windows))]
pub const IMAGE_PROXY: &str = "famimg://localhost/";

/// A random path segment, new each run, that every proxied image URL must
/// carry. A message can't know it, so a URL the message wrote itself (in CSS,
/// say) can't reach the proxy and bypass the image block.
pub fn proxy_token() -> &'static str {
    use std::hash::{BuildHasher, Hasher};
    static TOKEN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TOKEN.get_or_init(|| {
        let a = std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish();
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        format!("{a:016x}{:016x}", h.finish())
    })
}

/// The policy inside every message document. Nothing loads from the network:
/// only inline styles, `data:` images and fonts, and the image proxy.
pub const MESSAGE_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; img-src data: famimg: http://famimg.localhost; font-src data:";

/// What to sanitise.
pub struct Input<'a> {
    /// The body as stored.
    pub body: &'a str,
    /// Whether `body` is HTML (otherwise plain text).
    pub is_html: bool,
    /// Load remote images (through the proxy) instead of blocking them.
    pub remote_images: bool,
    /// Inline images by lower-case Content-ID (without `<>`), as `data:` URIs.
    pub cid: Arc<HashMap<String, String>>,
}

/// A sanitised message document.
pub struct Clean {
    /// The whole document, ready for the frame's `srcdoc`.
    pub html: String,
    /// Remote images that were blocked.
    pub blocked_images: usize,
}

/// The key a `cid:` URL or a Content-ID header is looked up by.
pub fn cid_key(s: &str) -> String {
    let s = s.trim();
    let s = match s.get(..4) {
        Some(p) if p.eq_ignore_ascii_case("cid:") => &s[4..],
        _ => s,
    };
    let s = percent_decode(s);
    s.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_ascii_lowercase()
}

/// True if the body refers to an inline image, so its attachments are worth fetching.
pub fn mentions_cid(body: &str) -> bool {
    body.as_bytes()
        .windows(4)
        .any(|w| w.eq_ignore_ascii_case(b"cid:"))
}

/// Sanitises a body into a document for the reading pane.
pub fn sanitize(input: &Input<'_>) -> Clean {
    if !input.is_html {
        return Clean {
            html: document(&text_to_html(input.body)),
            blocked_images: 0,
        };
    }
    let remote = input.remote_images;
    let cid = Arc::clone(&input.cid);
    let tags: HashSet<&str> = ["style", "center", "font"].into_iter().collect();
    let generic: HashSet<&str> = [
        "style",
        "class",
        "align",
        "valign",
        "width",
        "height",
        "bgcolor",
        "color",
        "face",
        "size",
        "border",
        "cellpadding",
        "cellspacing",
        "role",
        "dir",
        "lang",
        "title",
    ]
    .into_iter()
    .collect();
    let clean_content: HashSet<&str> = ["script", "iframe", "title", "noscript", "head", "xmp"]
        .into_iter()
        .collect();
    let body = ammonia::Builder::default()
        .add_tags(tags)
        .rm_clean_content_tags(["style"])
        .clean_content_tags(clean_content)
        .add_generic_attributes(generic)
        .url_schemes(
            ["http", "https", "mailto", "cid", "data"]
                .into_iter()
                .collect(),
        )
        .url_relative(ammonia::UrlRelative::Deny)
        .attribute_filter(move |el, attr, val| {
            if el == "img" && attr == "src" {
                return image_src(val, remote, &cid).map(Into::into);
            }
            if attr == "href" {
                let v = val.trim_start().to_ascii_lowercase();
                let ok = v.starts_with("http:")
                    || v.starts_with("https:")
                    || v.starts_with("mailto:")
                    || v.starts_with('#');
                return ok.then(|| val.into());
            }
            Some(val.into())
        })
        .link_rel(Some("noopener noreferrer"))
        .clean(input.body)
        .to_string();
    let blocked_images = body.matches(BLOCKED_MARK).count();
    Clean {
        html: document(&body),
        blocked_images,
    }
}

/// A blocked remote image keeps its size and alt text, and this marker, so the
/// page keeps its layout and the count is exact.
const BLOCKED_MARK: &str =
    "data:image/gif;base64,R0lGODlhAQABAAAAACH5BAEKAAEALAAAAAABAAEAAAICTAEAOw==#blocked";

fn image_src(val: &str, remote: bool, cid: &HashMap<String, String>) -> Option<String> {
    let v = val.trim();
    let lower = v.to_ascii_lowercase();
    if lower.starts_with("cid:") {
        return cid.get(&cid_key(v)).cloned();
    }
    if lower.starts_with("data:") {
        return lower.starts_with("data:image/").then(|| v.to_string());
    }
    if lower.starts_with("http:") || lower.starts_with("https:") {
        if remote {
            return Some(format!(
                "{IMAGE_PROXY}{}/{}",
                proxy_token(),
                percent_encode(v)
            ));
        }
        return Some(BLOCKED_MARK.to_string());
    }
    None
}

/// Wraps a sanitised body in a document with the CSP and the pane's base styles.
fn document(body: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta http-equiv=\"Content-Security-Policy\" content=\"{MESSAGE_CSP}\">\
         <meta name=\"referrer\" content=\"no-referrer\">\
         <style>{BASE_CSS}</style></head><body>{body}</body></html>"
    )
}

/// Readable defaults that a message's own styles can override.
///
/// Long words wrap with `overflow-wrap:break-word`, not `anywhere`: `anywhere`
/// also lowers a table cell's minimum width to one character, so mail laid out
/// in tables (price grids, columns beside a fixed-width cell) had its cells
/// squeezed until text such as "$89" stacked one character per line.
const BASE_CSS: &str = "html{color-scheme:light}\
body{margin:0;padding:16px 20px;font:14px/1.5 system-ui,-apple-system,'Segoe UI',sans-serif;\
color:#1d2430;background:#fff;overflow-wrap:break-word}\
img{max-width:100%}img:not([src$='#blocked']){height:auto}\
img[src$='#blocked']{background:#e4e8ef;min-width:16px;min-height:16px}\
pre.plain{white-space:pre-wrap;font:inherit;margin:0}\
blockquote{margin:0 0 0 .8em;padding-left:.8em;border-left:2px solid #c9d0db}\
::-webkit-scrollbar{width:10px;height:10px;background:transparent}\
::-webkit-scrollbar-thumb{background-color:rgba(128,128,128,.35);background-clip:padding-box;\
border:2px solid transparent;border-radius:10px;min-height:32px}\
::-webkit-scrollbar-thumb:hover{background-color:rgba(128,128,128,.6)}\
::-webkit-scrollbar-corner{background:transparent}";

/// Plain text as HTML: escaped, line breaks kept, web and mail addresses linked.
pub fn text_to_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 64);
    out.push_str("<pre class=\"plain\">");
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        linkify_line(line, &mut out);
    }
    out.push_str("</pre>");
    out
}

fn linkify_line(line: &str, out: &mut String) {
    let mut rest = line;
    while let Some(start) = find_link(rest) {
        escape_into(&rest[..start], out);
        let tail = &rest[start..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"'))
            .unwrap_or(tail.len());
        // Leave trailing punctuation out of the link: "see https://x.nz/a." ends at a.
        let link = tail[..end].trim_end_matches(['.', ',', ';', ':', ')', '!', '?', '\'']);
        let link = if link.is_empty() { &tail[..end] } else { link };
        out.push_str("<a href=\"");
        escape_into(link, out);
        out.push_str("\">");
        escape_into(link, out);
        out.push_str("</a>");
        rest = &tail[link.len()..];
    }
    escape_into(rest, out);
}

fn find_link(s: &str) -> Option<usize> {
    ["https://", "http://", "mailto:"]
        .iter()
        .filter_map(|p| s.find(p))
        .min()
}

fn escape_into(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}

/// Percent-encodes everything but unreserved characters, so a URL fits in one path segment.
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Reverses [`percent_encode`] (and any other `%XX` escapes).
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) =
                u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cid() -> Arc<HashMap<String, String>> {
        let mut m = HashMap::new();
        m.insert(
            "logo@club.example".into(),
            "data:image/png;base64,AAAA".into(),
        );
        m.insert(
            "image001.png@01da0000.00000000".into(),
            "data:image/png;base64,BBBB".into(),
        );
        Arc::new(m)
    }

    fn clean(body: &str, remote: bool) -> Clean {
        sanitize(&Input {
            body,
            is_html: true,
            remote_images: remote,
            cid: cid(),
        })
    }

    /// Every attribute name inside a tag, outside `<style>` text.
    fn attribute_names(html: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = html;
        while let Some(i) = rest.find('<') {
            rest = &rest[i + 1..];
            if rest.to_ascii_lowercase().starts_with("style") {
                // Skip to the end of the style element's text.
                let j = rest
                    .to_ascii_lowercase()
                    .find("</style")
                    .unwrap_or(rest.len());
                rest = &rest[j..];
                continue;
            }
            let end = rest.find('>').unwrap_or(rest.len());
            let tag = &rest[..end];
            let mut in_quote = None;
            let mut word = String::new();
            for ch in tag.chars() {
                match (in_quote, ch) {
                    (Some(q), c) if c == q => in_quote = None,
                    (Some(_), _) => {}
                    (None, '"' | '\'') => in_quote = Some(ch),
                    (None, '=') => {
                        out.push(word.trim().to_ascii_lowercase());
                        word.clear();
                    }
                    (None, c) if c.is_whitespace() => word.clear(),
                    (None, c) => word.push(c),
                }
            }
            rest = &rest[end..];
        }
        out
    }

    /// The values of every `src` and `href` attribute.
    fn urls(html: &str, attr: &str) -> Vec<String> {
        let pat = format!(" {attr}=\"");
        html.match_indices(&pat)
            .map(|(i, _)| {
                let v = &html[i + pat.len()..];
                v[..v.find('"').unwrap_or(v.len())].replace("&amp;", "&")
            })
            .collect()
    }

    fn check_safe(name: &str, out: &str, remote: bool) {
        let body = &out[out.find("<body>").expect("a body")..];
        let lower = body.to_ascii_lowercase();
        for bad in [
            "<script",
            "javascript:",
            "vbscript:",
            "<iframe",
            "<frame",
            "<object",
            "<embed",
            "<applet",
            "<form",
            "<input",
            "<button",
            "<textarea",
            "<base",
            "<link",
            "<meta",
            "<svg",
            "<math",
            "<template",
            "<portal",
            "<video",
            "<audio",
            "<source",
            "<noscript",
            "data:text/html",
            "file:",
        ] {
            assert!(!lower.contains(bad), "{name}: {bad} survived:\n{body}");
        }
        for a in attribute_names(body) {
            assert!(
                !a.starts_with("on"),
                "{name}: handler {a} survived:\n{body}"
            );
            assert!(
                ![
                    "srcset",
                    "background",
                    "poster",
                    "action",
                    "formaction",
                    "target"
                ]
                .contains(&a.as_str()),
                "{name}: attribute {a} survived:\n{body}"
            );
        }
        for src in urls(body, "src") {
            let ok = src.starts_with("data:image/") || (remote && src.starts_with(IMAGE_PROXY));
            assert!(ok, "{name}: image source {src} survived:\n{body}");
        }
        for href in urls(body, "href") {
            let ok = ["https:", "http:", "mailto:", "#"]
                .iter()
                .any(|p| href.starts_with(p));
            assert!(ok, "{name}: link {href} survived:\n{body}");
        }
        assert_eq!(out.matches("Content-Security-Policy").count(), 1, "{name}");
    }

    fn test_set() -> Vec<(String, String)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/html");
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "html" || e == "txt"))
            .collect();
        files.sort();
        assert!(files.len() >= 8, "the HTML test set is missing");
        files
            .into_iter()
            .map(|p| {
                (
                    p.file_name().unwrap().to_string_lossy().into_owned(),
                    std::fs::read_to_string(&p).unwrap(),
                )
            })
            .collect()
    }

    /// The HTML test set: nothing active or remote survives, with images
    /// blocked or allowed. With FAMILIAR_HTML_OUT set, the documents are
    /// written there for `scripts/check-reading-html.mjs` to render.
    #[test]
    fn html_test_set() {
        let out_dir = std::env::var_os("FAMILIAR_HTML_OUT").map(std::path::PathBuf::from);
        for (name, body) in test_set() {
            for remote in [false, true] {
                let c = sanitize(&Input {
                    body: &body,
                    is_html: name.ends_with(".html"),
                    remote_images: remote,
                    cid: cid(),
                });
                check_safe(&name, &c.html, remote);
                if let Some(d) = &out_dir {
                    std::fs::create_dir_all(d).unwrap();
                    let mode = if remote { "shown" } else { "blocked" };
                    std::fs::write(d.join(format!("{name}.{mode}.html")), &c.html).unwrap();
                }
            }
        }
    }

    #[test]
    fn remote_images_are_blocked_and_counted() {
        let body = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("testdata/html/remote-images.html"),
        )
        .unwrap();
        let blocked = clean(&body, false);
        // hero, pixel, srcset img, picture img (the protocol-relative one is dropped outright).
        assert_eq!(blocked.blocked_images, 4, "{}", blocked.html);
        assert!(blocked.html.contains("alt=\"Hero\""));
        assert!(blocked.html.contains("width=\"600\""), "layout kept");
        assert!(blocked.html.contains("data:image/png;base64,iVBORw0KGgo="));
        let shown = clean(&body, true);
        assert_eq!(shown.blocked_images, 0);
        assert!(shown.html.contains(&format!(
            "{IMAGE_PROXY}{}/{}",
            proxy_token(),
            percent_encode("https://cdn.example.com/hero.jpg")
        )));
    }

    #[test]
    fn cid_images_come_from_the_message() {
        let body = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/html/cid-images.html"),
        )
        .unwrap();
        let c = clean(&body, false);
        assert_eq!(
            c.html.matches("data:image/png;base64,AAAA").count(),
            3,
            "{}",
            c.html
        );
        assert_eq!(c.html.matches("data:image/png;base64,BBBB").count(), 1);
        assert!(c.html.contains("alt=\"no such part\""));
        assert!(!c.html.contains("cid:"));
    }

    #[test]
    fn text_survives() {
        let news = clean(include_str!("../../testdata/html/newsletter.html"), false).html;
        for t in [
            "Spring sale",
            "Up to 40% off",
            "Seeds, tools and pots.",
            "Unsubscribe",
        ] {
            assert!(news.contains(t), "{t}");
        }
        assert!(news.contains("border-radius:8px"), "styles kept");
        let reply = clean(
            include_str!("../../testdata/html/outlook-reply.html"),
            false,
        )
        .html;
        assert!(reply.contains("Saturday works."));
        assert!(reply.contains("Are you free for nets on Saturday?"));
        assert!(
            clean(include_str!("../../testdata/html/mutation.html"), false)
                .html
                .contains("Still readable.")
        );
    }

    #[test]
    fn plain_text_is_escaped_and_linked() {
        let c = sanitize(&Input {
            body: include_str!("../../testdata/html/plain.txt"),
            is_html: false,
            remote_images: false,
            cid: Arc::default(),
        });
        assert!(c.html.contains(
            "<a href=\"https://club.example/draw?season=2026&amp;grade=3\">https://club.example/draw?season=2026&amp;grade=3</a>. Shout"
        ), "{}", c.html);
        assert!(
            c.html
                .contains("<a href=\"mailto:secretary@club.example\">")
        );
        assert!(c.html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(c.html.contains("a &lt; b &amp; c &gt; d"));
    }

    #[test]
    fn cid_keys_match_across_spellings() {
        for s in [
            "cid:Logo@Club.Example",
            "<logo@club.example>",
            "cid:%3Clogo@club.example%3E",
        ] {
            assert_eq!(cid_key(s), "logo@club.example", "{s}");
        }
    }

    #[test]
    fn percent_round_trip() {
        let u = "https://x.example/a b/ü?q=1&r=%41";
        assert_eq!(percent_decode(&percent_encode(u)), u);
    }
}
