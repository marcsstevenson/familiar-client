// SPDX-License-Identifier: MIT OR Apache-2.0
//! Markdown bodies: `--markdown` on the compose commands and
//! `fam draft edit`, which opens a draft in `$EDITOR` as Markdown.
//!
//! Mail is written line by line, so a single line break is kept as a line
//! break ("Thanks,\nMarc" stays on two lines), unlike plain CommonMark.

use pulldown_cmark::{Event, Options, Parser};

/// Markdown as body HTML. Tables, strikethrough and task lists are on;
/// HTML in the Markdown passes through as written.
pub fn to_html(markdown: &str) -> String {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let events = Parser::new_ext(markdown, opts).map(|e| match e {
        Event::SoftBreak => Event::HardBreak,
        e => e,
    });
    let mut out = String::with_capacity(markdown.len() * 3 / 2);
    pulldown_cmark::html::push_html(&mut out, events);
    out
}

/// Body HTML as Markdown, for editing. Formatting Markdown can't say
/// (fonts, colours) is dropped.
pub fn from_html(html: &str) -> String {
    let conv = htmd::HtmlToMarkdown::builder()
        .options(htmd::options::Options {
            bullet_list_marker: htmd::options::BulletListMarker::Dash,
            ul_bullet_spacing: 1,
            ol_number_spacing: 1,
            link_style: htmd::options::LinkStyle::InlinedPreferAutolinks,
            ..Default::default()
        })
        .skip_tags(vec!["head", "style", "script", "title", "meta"])
        .build();
    let md = conv
        .convert(html)
        .unwrap_or_else(|_| familiar_search::text::html_to_lines(html));
    // `<br>` comes out as a trailing two spaces, and the HTML's own line
    // break after it as a space starting the next line. A plain line break
    // means the same here (see `to_html`).
    let mut lines = Vec::new();
    let mut after_br = false;
    for l in md.lines() {
        let l = if after_br {
            l.strip_prefix(' ').unwrap_or(l)
        } else {
            l
        };
        after_br = l.ends_with("  ");
        lines.push(l.strip_suffix("  ").unwrap_or(l));
    }
    lines.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_breaks_are_kept() {
        assert_eq!(to_html("Thanks,\nMarc"), "<p>Thanks,<br />\nMarc</p>\n");
    }

    #[test]
    fn formatting_becomes_html() {
        let h = to_html("Hi **Ana**,\n\n- one\n- two\n\nSee [the plan](https://x.test/p).");
        assert!(h.contains("<strong>Ana</strong>"), "{h}");
        assert!(h.contains("<ul>\n<li>one</li>"), "{h}");
        assert!(
            h.contains("<a href=\"https://x.test/p\">the plan</a>"),
            "{h}"
        );
    }

    #[test]
    fn html_round_trips_through_markdown() {
        let md = "Hi **Ana**,\n\n- one\n- two\n\nThanks,\nMarc";
        assert_eq!(from_html(&to_html(md)), md);
    }

    #[test]
    fn plain_compose_html_reads_as_text() {
        let html = "Hello there<br>\nsecond line<br>\n<div><br></div>";
        assert_eq!(from_html(html), "Hello there\nsecond line");
    }
}
