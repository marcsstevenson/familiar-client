// SPDX-License-Identifier: MIT OR Apache-2.0
//! Plain text from message bodies, for indexing.

/// Most text indexed from one body. Longer bodies (huge newsletters, logs
/// pasted into mail) are cut here so one message can't bloat the index.
pub const MAX_BODY_CHARS: usize = 100_000;

/// The visible text of an HTML body: tags, comments, `<script>`, `<style>`
/// and `<head>` dropped, common entities decoded, whitespace collapsed. Not a
/// full HTML parser; it only needs to find the words.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let lower = html.to_ascii_lowercase();
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'<' {
            let rest = &lower[i..];
            if rest.starts_with("<!--") {
                i = rest.find("-->").map_or(bytes.len(), |e| i + e + 3);
                continue;
            }
            let skip_to = ["script", "style", "head", "title"]
                .iter()
                .find(|t| {
                    rest[1..].starts_with(*t)
                        && rest[1 + t.len()..]
                            .chars()
                            .next()
                            .is_some_and(|c| c == '>' || c.is_ascii_whitespace())
                })
                .map(|t| format!("</{t}"));
            if let Some(end_tag) = skip_to {
                i = match rest.find(&end_tag) {
                    Some(e) => {
                        let after = i + e;
                        lower[after..]
                            .find('>')
                            .map_or(bytes.len(), |g| after + g + 1)
                    }
                    None => bytes.len(),
                };
                out.push(' ');
                continue;
            }
            i = rest.find('>').map_or(bytes.len(), |e| i + e + 1);
            out.push(' ');
            continue;
        }
        if c == b'&' {
            if let Some((ch, len)) = entity(&html[i..]) {
                out.push(ch);
                i += len;
                continue;
            }
        }
        // Copy one UTF-8 character.
        let len = utf8_len(c);
        out.push_str(&html[i..(i + len).min(html.len())]);
        i += len;
    }
    collapse(&out)
}

/// The text to index for a body.
pub fn body_text(raw: &[u8], is_html: bool) -> String {
    let s = String::from_utf8_lossy(raw);
    let mut t = if is_html {
        html_to_text(&s)
    } else {
        collapse(&s)
    };
    if t.len() > MAX_BODY_CHARS {
        let mut cut = MAX_BODY_CHARS;
        while !t.is_char_boundary(cut) {
            cut -= 1;
        }
        t.truncate(cut);
    }
    t
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xff => 4,
        _ => 1,
    }
}

fn entity(s: &str) -> Option<(char, usize)> {
    let end = s[..s.len().min(12)].find(';')?;
    let name = &s[1..end];
    let ch = match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "ndash" => '–',
        "mdash" => '—',
        "rsquo" | "lsquo" => '\'',
        "rdquo" | "ldquo" => '"',
        "hellip" => '…',
        "copy" => '©',
        _ => {
            let n = name.strip_prefix('#')?;
            let code = match n.strip_prefix(['x', 'X']) {
                Some(h) => u32::from_str_radix(h, 16).ok()?,
                None => n.parse().ok()?,
            };
            char::from_u32(code)?
        }
    };
    Some((ch, end + 1))
}

fn collapse(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for w in s.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_markup() {
        let html = r#"<html><head><title>T</title><style>p{color:red}</style></head>
            <body><!-- hidden --><p>Hi &amp; <b>welcome</b>&nbsp;to the AGM&#33; caf&eacute; &#x263A;</p>
            <script type="x">var secret = 1;</script><div>Bye</div></body></html>"#;
        let t = html_to_text(html);
        assert_eq!(t, "Hi & welcome to the AGM! caf&eacute; ☺ Bye");
        assert!(!t.contains("secret") && !t.contains("color") && !t.contains("hidden"));
    }

    #[test]
    fn keeps_non_ascii_and_survives_junk() {
        assert_eq!(html_to_text("<p>Tēnā koe</p>"), "Tēnā koe");
        for junk in [
            "<",
            "<p",
            "&",
            "&#;",
            "&#xzz;",
            "<script>",
            "<!--",
            "a < b > c",
        ] {
            let _ = html_to_text(junk);
        }
    }

    #[test]
    fn body_text_is_capped() {
        let big = "word ".repeat(MAX_BODY_CHARS);
        assert!(body_text(big.as_bytes(), false).len() <= MAX_BODY_CHARS);
        assert_eq!(body_text(b"a\n\n b", false), "a b");
    }
}
