//! WP 0.5 store benchmark.
//!
//! ```text
//! store-bench load  --data DIR [--messages 500000] [--seed 42]
//! store-bench bench --data DIR [--iters 1000] [--out results.json]
//! store-bench list  --data DIR --folder ID [--limit 50] --json
//! ```

mod bench;
mod store;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "SQLite + Tantivy store benchmark for Familiar (WP 0.5)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Generate a synthetic mailbox and load it into a new store.
    Load {
        #[arg(long)]
        data: PathBuf,
        #[arg(long, default_value_t = 500_000)]
        messages: usize,
        #[arg(long, default_value_t = 42)]
        seed: u64,
        /// Keep and index bodies only for the last N days (the sync window).
        /// Without it every body is kept and indexed: the stress case.
        #[arg(long)]
        body_window_days: Option<i64>,
    },
    /// Run the measurements against a loaded store.
    Bench {
        #[arg(long)]
        data: PathBuf,
        #[arg(long, default_value_t = 42)]
        seed: u64,
        #[arg(long, default_value_t = 1000)]
        iters: usize,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Print the first page of a folder, like `fam mail list`.
    List {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        folder: i64,
        #[arg(long, default_value_t = 50)]
        limit: i64,
        #[arg(long)]
        json: bool,
    },
    /// Print the first page of search results, newest first, like `fam mail search`.
    Search {
        #[arg(long)]
        data: PathBuf,
        query: String,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Load {
            data,
            messages,
            seed,
            body_window_days,
        } => {
            eprintln!(
                "loading {messages} messages (seed {seed}) into {}",
                data.display()
            );
            let rep = store::load(&data, seed, messages, body_window_days)?;
            let json = serde_json::to_string_pretty(&rep)?;
            std::fs::write(data.join("load-report.json"), &json)?;
            println!("{json}");
        }
        Cmd::Bench {
            data,
            seed,
            iters,
            out,
        } => {
            let exe = std::env::current_exe()?;
            let mut rep = serde_json::to_value(bench::run(&data, seed, iters, &exe)?)?;
            if let Ok(load) = std::fs::read_to_string(data.join("load-report.json")) {
                rep["load"] = serde_json::from_str(&load)?;
            }
            let json = serde_json::to_string_pretty(&rep)?;
            if let Some(out) = out {
                std::fs::write(out, &json)?;
            }
            println!("{json}");
        }
        Cmd::List {
            data,
            folder,
            limit,
            json,
        } => {
            let conn = store::open_reader(&store::Paths::new(&data).db)?;
            let rows = bench::folder_page(&conn, folder, limit)?;
            use std::io::Write;
            let mut out = std::io::stdout().lock();
            if json {
                serde_json::to_writer(&mut out, &rows)?;
                writeln!(out)?;
            } else {
                for r in rows {
                    writeln!(out, "{}  {:<30}  {}", r.received_at, r.from_name, r.subject)?;
                }
            }
        }
        Cmd::Search { data, query, limit } => {
            let paths = store::Paths::new(&data);
            let conn = store::open_reader(&paths.db)?;
            let rows = bench::Search::open(&paths)?.by_date(&conn, &query, limit)?;
            println!("{}", serde_json::to_string(&rows)?);
        }
    }
    Ok(())
}
