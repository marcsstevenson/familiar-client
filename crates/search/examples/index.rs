// SPDX-License-Identifier: MIT OR Apache-2.0
//! Builds the search index for a store, as famd's indexer does, for trying
//! search on a synthetic store (see the store's `seed` example).
//!
//! ```sh
//! cargo run --release -p familiar-search --example index -- <data dir>
//! ```

fn main() {
    let Some(dir) = std::env::args().nth(1) else {
        eprintln!("usage: index <data dir>");
        std::process::exit(2);
    };
    let dir = std::path::Path::new(&dir);
    let mut w = familiar_store::Writer::open(&dir.join("store")).expect("open store");
    let (mut ix, _) =
        familiar_search::Indexer::open_for(&dir.join("search"), &mut w).expect("open index");
    let n = ix.drain(&mut w).expect("index");
    eprintln!("indexed {n} messages");
}
