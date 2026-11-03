# Phase 0 tools

A standalone Cargo workspace, kept apart from the product workspace that WP 1.0 creates.

- `synth`: deterministic synthetic mailbox generator. `cargo run --release -p synth -- --messages 1000 > mail.jsonl`
- `store-bench`: the WP 0.5 SQLite + Tantivy benchmark. See [designs/WP 0.5 Store benchmark.md](../designs/WP%200.5%20Store%20benchmark.md) for results and how to run it.
