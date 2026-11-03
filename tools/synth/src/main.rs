//! `synth [--seed N] [--messages N]` writes a synthetic mailbox as JSON lines:
//! one `{"accounts":[...],"folders":[...]}` line, then one message per line.
//! `--vocab N` prints the N most frequent words instead.

use std::io::{BufWriter, Write};

fn main() -> std::io::Result<()> {
    let mut seed = 42u64;
    let mut messages = 1_000usize;
    let mut vocab: Option<usize> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let v = args.next().unwrap_or_default();
        match a.as_str() {
            "--seed" => seed = v.parse().expect("--seed takes a number"),
            "--messages" => messages = v.parse().expect("--messages takes a number"),
            "--vocab" => vocab = Some(v.parse().expect("--vocab takes a number")),
            _ => {
                eprintln!("usage: synth [--seed N] [--messages N] [--vocab N]");
                std::process::exit(2);
            }
        }
    }
    let gen = synth::Generator::new(seed, messages);
    if let Some(n) = vocab {
        // Print the N most frequent words instead, e.g. to pick search terms.
        for w in gen.vocab().iter().take(n) {
            println!("{w}");
        }
        return Ok(());
    }
    let out = std::io::stdout();
    let mut out = BufWriter::new(out.lock());
    let header =
        serde_json::json!({ "seed": seed, "accounts": gen.accounts(), "folders": gen.folders() });
    writeln!(out, "{header}")?;
    for m in gen {
        serde_json::to_writer(&mut out, &m)?;
        out.write_all(b"\n")?;
    }
    out.flush()
}
