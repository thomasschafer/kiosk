# Agent instructions

## Text
Please use sentence case unless some other casing e.g. title case is absolutely necessary.

## Code style
- Code should be as DRY as reasonably possible. This doesn't just apply to exact copies of code: if there are repeated patterns, we should extract these out for re-use when reasonably possible.
- We should aim to use Rust's features to simplify code - better to generate something with a macro or similar than risk it going out of sync. For instance, often when we enumerate over all variants of an enum we could instead use a macro, attributes on the struct fields or similar.

## State modeling goal
- Prefer type-driven state machines over loose flags.
- Invalid states should be unrepresentable wherever practical.
- Group mode-specific state into typed structs (for example, sessions-only fields in a `SessionsViewState`) rather than keeping them as top-level `AppState` fields.
- Represent lifecycle explicitly with enums (for example, `Loading` vs `Ready`) rather than booleans.
- Prefer typed transitions and helper methods that enforce invariants rather than ad-hoc field mutation spread across files.
- Favor ownership patterns that make cleanup automatic (for example, mode-owned poller handles that cancel on drop).

## Creating PRs
In PR descriptions:
- Don't be too verbose - keep things concise but still include all relevant information for a reviewer.
- Don't add pointless verification sections (e.g. detailing the commands you ran to test).
- Include instructions on how to manually test the changes if possible - if not possible explain briefly why it can't be tested manually and give alternatives to verify the changes.

## Dev environment
Use the Nix dev shell for all project tooling commands unless explicitly told otherwise.
This includes build, test, lint, formatting, and any `cargo`/Rust-related command.
This prevents failures due to missing toolchain/system binaries.
Examples: `nix develop -c cargo test`, `nix develop -c cargo run`, `nix develop -c cargo clippy`.

## Running kiosk in the app
`scripts/app-sandbox` runs the released Herdr app headless in Docker and drives kiosk there; see its README. Prefer it to the person's own app, whose mods and config it leaves alone.
