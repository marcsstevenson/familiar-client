// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam`, the command line. Its subcommands are generated from the command registry.

fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(familiar_cli::main_with(std::env::args_os()))
}
