#!/bin/sh
# Checks that Cargo.toml and the mod agree on the version, and with the tag when given one
# (`scripts/check-version.sh v0.8.0`), so a release's binary and mod always match.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cargo_version=$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\([^"]*\)"$/\1/p' "$root/Cargo.toml")
mod_version=$(sed -n 's/^const VERSION = "\([^"]*\)";$/\1/p' "$root/mods/kiosk/main.js")
[ -n "$cargo_version" ] || { echo "no version in Cargo.toml" >&2; exit 1; }
[ "$cargo_version" = "$mod_version" ] || {
    echo "Cargo.toml says $cargo_version and mods/kiosk/main.js says ${mod_version:-nothing}" >&2
    exit 1
}
if [ $# -gt 0 ] && [ "$1" != "v$cargo_version" ]; then
    echo "the tag is $1 but the version is $cargo_version" >&2
    exit 1
fi
echo "$cargo_version"
