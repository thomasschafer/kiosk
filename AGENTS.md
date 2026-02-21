# Agent instructions

## Dev environment
Use the Nix dev shell for all project tooling commands unless explicitly told otherwise.
This includes build, test, lint, formatting, and any `cargo`/Rust-related command.
This prevents failures due to missing toolchain/system binaries.
Examples: `nix develop -c cargo test`, `nix develop -c cargo run`, `nix develop -c cargo clippy`.
