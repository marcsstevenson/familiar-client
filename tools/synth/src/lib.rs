//! Deterministic synthetic mailbox generator.
//!
//! The same seed always produces the same mailbox, on every platform and with
//! every dependency version, because the random number generator is defined
//! here rather than taken from a crate. The output is shaped like a
//! long-lived Microsoft 365 mailbox: three accounts, a folder tree, threads
//! that span Inbox, Sent Items and user folders, Zipf-distributed vocabulary
//! and correspondents, quoted replies, HTML bodies and attachments.

use serde::Serialize;
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Random numbers
// ---------------------------------------------------------------------------

/// xoshiro256** seeded with SplitMix64. Small, fast and fixed forever.
#[derive(Clone)]
pub struct Rng {
    s: [u64; 4],
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut x = seed;
        let mut next = || {
            x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = x;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        Rng {
            s: [next(), next(), next(), next()],
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// Uniform in `0..n`.
    pub fn below(&mut self, n: u64) -> u64 {
        ((self.next_u64() as u128 * n as u128) >> 64) as u64
    }

    /// Uniform in `[lo, hi]`.
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.below(hi - lo + 1)
    }

    /// Uniform in `[0, 1)`.
    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    pub fn chance(&mut self, p: f64) -> bool {
        self.f64() < p
    }

    /// Standard normal (Box-Muller).
    pub fn normal(&mut self) -> f64 {
        let u1 = self.f64().max(f64::MIN_POSITIVE);
        let u2 = self.f64();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

/// Samples an index from fixed weights using a cumulative table.
#[derive(Clone)]
pub struct Weighted {
    cdf: Vec<f64>,
}

impl Weighted {
    pub fn new(weights: &[f64]) -> Self {
        let mut acc = 0.0;
        let mut cdf = Vec::with_capacity(weights.len());
        for w in weights {
            acc += w;
            cdf.push(acc);
        }
        for c in &mut cdf {
            *c /= acc;
        }
        Weighted { cdf }
    }

    /// Zipf over `n` ranks with exponent `s`; rank 0 is the most frequent.
    pub fn zipf(n: usize, s: f64) -> Self {
        let weights: Vec<f64> = (1..=n).map(|k| 1.0 / (k as f64).powf(s)).collect();
        Self::new(&weights)
    }

    pub fn sample(&self, rng: &mut Rng) -> usize {
        let u = rng.f64();
        self.cdf
            .partition_point(|&c| c <= u)
            .min(self.cdf.len() - 1)
    }

    pub fn len(&self) -> usize {
        self.cdf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cdf.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Output model
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct Account {
    pub id: u32,
    pub name: String,
    pub email: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Folder {
    pub id: u32,
    pub account_id: u32,
    pub parent_id: Option<u32>,
    pub name: String,
    /// Graph well-known name such as `inbox` or `sentitems`.
    pub well_known: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Address {
    pub name: String,
    pub addr: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Message {
    pub id: u64,
    pub account_id: u32,
    pub folder_id: u32,
    /// Shaped like a Graph immutable ID.
    pub graph_id: String,
    pub internet_message_id: String,
    /// Shaped like a Graph conversationId.
    pub conversation_id: String,
    pub subject: String,
    pub from: Address,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    /// Unix seconds, UTC.
    pub received_at: i64,
    pub is_read: bool,
    pub is_flagged: bool,
    pub importance: u8,
    pub attachments: Vec<String>,
    pub preview: String,
    pub body_text: String,
    pub body_html: Option<String>,
}

impl Message {
    pub fn has_attachments(&self) -> bool {
        !self.attachments.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Generator
// ---------------------------------------------------------------------------

/// 2006-01-01T00:00:00Z, the start of Marc's oldest mail.
pub const HISTORY_START: i64 = 1_136_073_600;
/// 2026-11-01T00:00:00Z.
pub const HISTORY_END: i64 = 1_793_491_200;

const ONSETS: &[&str] = &[
    "b", "c", "d", "f", "g", "h", "j", "k", "l", "m", "n", "p", "r", "s", "t", "v", "w", "z", "br",
    "cl", "dr", "fl", "gr", "pl", "st", "tr", "sh", "ch", "th", "",
];
const VOWELS: &[&str] = &["a", "e", "i", "o", "u", "ai", "ea", "ou", "io", "y"];
const CODAS: &[&str] = &["", "", "n", "r", "s", "t", "l", "m", "nd", "st", "ck", "x"];
const TLDS: &[&str] = &[".com", ".co.nz", ".org", ".net", ".io", ".com.au"];
const EXTS: &[&str] = &[".pdf", ".docx", ".xlsx", ".jpg", ".png", ".zip", ".ics"];
const USER_FOLDERS_PER_ACCOUNT: [usize; 3] = [40, 8, 6];
const ACCOUNT_SHARE: [f64; 3] = [0.60, 0.25, 0.15];

pub struct Generator {
    rng: Rng,
    remaining: usize,
    next_id: u64,
    accounts: Vec<Account>,
    folders: Vec<Folder>,
    vocab: Vec<String>,
    word_dist: Weighted,
    contacts: Vec<Address>,
    contact_dist: Weighted,
    account_dist: Weighted,
    /// Per account: inbound folder ids and their sampling weights.
    inbound: Vec<(Vec<u32>, Weighted)>,
    sent_folder: Vec<u32>,
    pending: std::collections::VecDeque<Message>,
}

impl Generator {
    pub fn new(seed: u64, messages: usize) -> Self {
        let mut rng = Rng::new(seed);
        let vocab = make_vocab(&mut rng, 40_000);
        let word_dist = Weighted::zipf(vocab.len(), 1.07);

        let mut names = HashSet::new();
        let mut contacts = Vec::new();
        let domains: Vec<String> = (0..400)
            .map(|_| format!("{}{}", make_word(&mut rng, 2, 3), rng.pick(TLDS)))
            .collect();
        while contacts.len() < 5_000 {
            let first = capitalise(&make_word(&mut rng, 1, 3));
            let last = capitalise(&make_word(&mut rng, 2, 3));
            if !names.insert(format!("{first} {last}")) {
                continue;
            }
            let domain = rng.pick(&domains).clone();
            let addr = format!(
                "{}.{}@{}",
                first.to_lowercase(),
                last.to_lowercase(),
                domain
            );
            contacts.push(Address {
                name: format!("{first} {last}"),
                addr,
            });
        }
        let contact_dist = Weighted::zipf(contacts.len(), 0.9);

        let accounts = vec![
            Account {
                id: 1,
                name: "Personal".into(),
                email: "me@personal.example".into(),
            },
            Account {
                id: 2,
                name: "Club".into(),
                email: "me@club.example".into(),
            },
            Account {
                id: 3,
                name: "Work".into(),
                email: "me@work.example".into(),
            },
        ];

        let mut folders = Vec::new();
        let mut inbound = Vec::new();
        let mut sent_folder = Vec::new();
        let well_known = [
            ("Inbox", "inbox", 30.0),
            ("Sent Items", "sentitems", 0.0),
            ("Drafts", "drafts", 0.0),
            ("Deleted Items", "deleteditems", 8.0),
            ("Junk Email", "junkemail", 2.0),
            ("Archive", "archive", 20.0),
        ];
        for (ai, account) in accounts.iter().enumerate() {
            let mut ids = Vec::new();
            let mut weights = Vec::new();
            for (name, wk, w) in well_known {
                let id = folders.len() as u32 + 1;
                folders.push(Folder {
                    id,
                    account_id: account.id,
                    parent_id: None,
                    name: name.into(),
                    well_known: Some(wk.into()),
                });
                if wk == "sentitems" {
                    sent_folder.push(id);
                }
                if w > 0.0 {
                    ids.push(id);
                    weights.push(w);
                }
            }
            let mut user_ids: Vec<u32> = Vec::new();
            for k in 0..USER_FOLDERS_PER_ACCOUNT[ai] {
                let id = folders.len() as u32 + 1;
                let parent_id = if !user_ids.is_empty() && rng.chance(0.3) {
                    Some(*rng.pick(&user_ids))
                } else {
                    None
                };
                folders.push(Folder {
                    id,
                    account_id: account.id,
                    parent_id,
                    name: capitalise(&make_word(&mut rng, 2, 4)),
                    well_known: None,
                });
                user_ids.push(id);
                ids.push(id);
                // User folders share 40 weight between them, Zipf-style.
                weights.push(40.0 / ((k + 1) as f64).powf(1.1) / 4.0);
            }
            let dist = Weighted::new(&weights);
            inbound.push((ids, dist));
        }

        Generator {
            rng,
            remaining: messages,
            next_id: 1,
            accounts,
            folders,
            vocab,
            word_dist,
            contacts,
            contact_dist,
            account_dist: Weighted::new(&ACCOUNT_SHARE),
            inbound,
            sent_folder,
            pending: Default::default(),
        }
    }

    pub fn accounts(&self) -> &[Account] {
        &self.accounts
    }

    pub fn folders(&self) -> &[Folder] {
        &self.folders
    }

    /// The vocabulary in frequency order: index 0 is the most common word.
    pub fn vocab(&self) -> &[String] {
        &self.vocab
    }

    /// Contacts in frequency order: index 0 writes most often.
    pub fn contacts(&self) -> &[Address] {
        &self.contacts
    }

    fn word(&mut self) -> &str {
        let i = self.word_dist.sample(&mut self.rng);
        &self.vocab[i]
    }

    fn words(&mut self, n: usize) -> String {
        let mut s = String::new();
        for i in 0..n {
            if i > 0 {
                s.push(' ');
            }
            let w = self.word().to_string();
            s.push_str(&w);
        }
        s
    }

    fn contact(&mut self) -> Address {
        let i = self.contact_dist.sample(&mut self.rng);
        self.contacts[i].clone()
    }

    fn random_id(&mut self, prefix: &str, len: usize) -> String {
        const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut s = String::with_capacity(prefix.len() + len);
        s.push_str(prefix);
        for _ in 0..len {
            s.push(B64[self.rng.below(64) as usize] as char);
        }
        s
    }

    fn body(&mut self, greet: &str, sign: &str) -> String {
        let target = (4.6 + 0.9 * self.rng.normal()).exp().clamp(3.0, 4000.0) as usize;
        let mut out = format!("Hi {greet},\n\n");
        let mut written = 0;
        while written < target {
            let sentences = self.rng.range(2, 6);
            for _ in 0..sentences {
                let n = self.rng.range(5, 18) as usize;
                let mut s = self.words(n);
                if let Some(first) = s.get_mut(0..1) {
                    first.make_ascii_uppercase();
                }
                out.push_str(&s);
                out.push_str(". ");
                written += n;
            }
            out.push_str("\n\n");
        }
        out.push_str("Cheers,\n");
        out.push_str(sign);
        out
    }

    fn generate_thread(&mut self) {
        let ai = self.account_dist.sample(&mut self.rng);
        let account = self.accounts[ai].clone();
        let me = Address {
            name: "Me".into(),
            addr: account.email.clone(),
        };
        let (ids, dist) = &self.inbound[ai];
        let home = ids[dist.sample(&mut self.rng)];
        let sent = self.sent_folder[ai];

        let mut len = 1;
        while len < 40 && self.rng.chance(0.42) {
            len += 1;
        }
        let correspondent = self.contact();
        let mut cc: Vec<Address> = Vec::new();
        if self.rng.chance(0.25) {
            for _ in 0..self.rng.range(1, 4) {
                cc.push(self.contact());
            }
        }

        let span = (HISTORY_END - HISTORY_START) as f64;
        // sqrt gives linearly growing volume over the years.
        let mut t = HISTORY_START + (self.rng.f64().sqrt() * span) as i64;
        let subject_words = self.rng.range(2, 8) as usize;
        let mut base_subject = self.words(subject_words);
        if let Some(first) = base_subject.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        let forwarded = self.rng.chance(0.05);
        let conversation_id = self.random_id("AAQkAD", 70);
        let mut prev: Option<(Address, String, i64)> = None;

        for i in 0..len {
            if self.remaining == 0 {
                return;
            }
            let from_me = i > 0 && self.rng.chance(0.45);
            let (from, to, folder_id) = if from_me {
                (me.clone(), vec![correspondent.clone()], sent)
            } else {
                let sender = if i > 0 && !cc.is_empty() && self.rng.chance(0.3) {
                    self.rng.pick(&cc).clone()
                } else {
                    correspondent.clone()
                };
                let folder = if self.rng.chance(0.1) {
                    let (ids, dist) = &self.inbound[ai];
                    ids[dist.sample(&mut self.rng)]
                } else {
                    home
                };
                (sender, vec![me.clone()], folder)
            };

            let subject = match (i, forwarded) {
                (0, false) => base_subject.clone(),
                (0, true) => format!("Fw: {base_subject}"),
                _ => format!("Re: {base_subject}"),
            };
            let greet = first_name(&to[0].name).to_string();
            let sign = first_name(&from.name).to_string();
            let mut body_text = self.body(&greet, &sign);
            if let Some((psender, pbody, _)) = &prev {
                body_text.push_str(&format!(
                    "\n\nOn an earlier date, {} wrote:\n",
                    psender.name
                ));
                for line in pbody.lines().take(80) {
                    body_text.push_str("> ");
                    body_text.push_str(line);
                    body_text.push('\n');
                }
            }
            let body_html = if self.rng.chance(0.75) {
                Some(to_html(&body_text))
            } else {
                None
            };
            let mut attachments = Vec::new();
            if self.rng.chance(0.12) {
                for _ in 0..self.rng.range(1, 3) {
                    let ext = *self.rng.pick(EXTS);
                    attachments.push(format!("{}{}", self.words(2).replace(' ', "-"), ext));
                }
            }
            let recent = t > HISTORY_END - 30 * 86_400;
            let is_read = from_me
                || if recent {
                    self.rng.chance(0.5)
                } else {
                    self.rng.chance(0.98)
                };
            let preview: String = body_text
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(255)
                .collect();
            let domain = from.addr.split('@').nth(1).unwrap_or("example").to_string();
            let imid = format!("<{:016x}@{}>", self.rng.next_u64(), domain);
            let graph_id = self.random_id("AAkALgAAAAAAHYQDEapmEc2byACqAC-EWg0A", 116);

            let msg = Message {
                id: self.next_id,
                account_id: account.id,
                folder_id,
                graph_id,
                internet_message_id: imid,
                conversation_id: conversation_id.clone(),
                subject,
                from: from.clone(),
                to,
                cc: cc.clone(),
                received_at: t.min(HISTORY_END),
                is_read,
                is_flagged: self.rng.chance(0.02),
                importance: if self.rng.chance(0.03) { 2 } else { 1 },
                attachments,
                preview,
                body_text: body_text.clone(),
                body_html,
            };
            self.next_id += 1;
            self.remaining -= 1;
            prev = Some((from, body_text, t));
            self.pending.push_back(msg);
            // Replies arrive minutes to days later.
            let gap = 60.0 + self.rng.f64().powi(2) * 3.0 * 86_400.0;
            t += gap as i64;
        }
    }
}

impl Iterator for Generator {
    type Item = Message;

    fn next(&mut self) -> Option<Message> {
        while self.pending.is_empty() {
            if self.remaining == 0 {
                return None;
            }
            self.generate_thread();
        }
        self.pending.pop_front()
    }
}

fn make_word(rng: &mut Rng, min_syll: u64, max_syll: u64) -> String {
    let n = rng.range(min_syll, max_syll);
    let mut w = String::new();
    for _ in 0..n {
        w.push_str(rng.pick(ONSETS));
        w.push_str(rng.pick(VOWELS));
        w.push_str(rng.pick(CODAS));
    }
    w
}

fn make_vocab(rng: &mut Rng, size: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut words = Vec::with_capacity(size);
    while words.len() < size {
        let w = make_word(rng, 1, 3);
        if w.len() >= 2 && seen.insert(w.clone()) {
            words.push(w);
        }
    }
    // Short words first so the most frequent ranks are short, as in real text.
    words.sort_by_key(|w| w.len());
    words
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn first_name(name: &str) -> &str {
    name.split_whitespace().next().unwrap_or(name)
}

fn to_html(text: &str) -> String {
    let mut html = String::from(
        "<html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=utf-8\">\
<style type=\"text/css\">p.MsoNormal, li.MsoNormal, div.MsoNormal {margin:0cm; font-size:11.0pt; \
font-family:\"Calibri\",sans-serif;} a:link {color:#0563C1; text-decoration:underline;}</style>\
</head><body lang=\"EN-NZ\" link=\"#0563C1\"><div class=\"WordSection1\">",
    );
    for para in text.split("\n\n") {
        html.push_str("<p class=\"MsoNormal\">");
        for (i, line) in para.lines().enumerate() {
            if i > 0 {
                html.push_str("<br>");
            }
            html.push_str(
                &line
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;"),
            );
        }
        html.push_str("</p>");
    }
    html.push_str("</div></body></html>");
    html
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_mailbox() {
        let a: Vec<_> = Generator::new(7, 500)
            .map(|m| (m.graph_id, m.body_text))
            .collect();
        let b: Vec<_> = Generator::new(7, 500)
            .map(|m| (m.graph_id, m.body_text))
            .collect();
        assert_eq!(a, b);
    }

    #[test]
    fn exact_count_and_threads_span_folders() {
        let msgs: Vec<_> = Generator::new(1, 5_000).collect();
        assert_eq!(msgs.len(), 5_000);
        let mut folders_by_conv: std::collections::HashMap<&str, HashSet<u32>> = Default::default();
        for m in &msgs {
            folders_by_conv
                .entry(&m.conversation_id)
                .or_default()
                .insert(m.folder_id);
        }
        assert!(folders_by_conv.values().any(|f| f.len() > 1));
        assert!(msgs
            .iter()
            .all(|m| (HISTORY_START..=HISTORY_END).contains(&m.received_at)));
    }

    #[test]
    fn rng_is_stable() {
        // Pin the generator so benchmark mailboxes stay comparable over time.
        let mut r = Rng::new(42);
        assert_eq!(r.next_u64(), Rng::new(42).next_u64());
        let first: Vec<u64> = (0..3).map(|_| r.below(1000)).collect();
        assert_eq!(first.len(), 3);
    }
}
