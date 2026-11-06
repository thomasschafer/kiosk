#!/bin/sh
# Installs kiosk for the Herdr app: the kiosk mod and its herdr-app-kiosk binary, together
# in <app home>/mods/kiosk, where the mod looks for them.
#
#   curl -fsSL https://raw.githubusercontent.com/thomasschafer/herdr-app-kiosk/main/scripts/install.sh | sh
#   sh scripts/install.sh [--version <tag>] [--from-source]
#
# It downloads a release's mod and prebuilt binary (the latest, or --version) and checks
# both against the release's SHA256SUMS, and, when the GitHub CLI is installed, against the
# release's build attestations. Run from a checkout with --from-source, it installs that
# checkout's mod with a binary built from it. The app home is $HERDR_APP_HOME, else
# $XDG_CONFIG_HOME/herdr-app, else ~/.config/herdr-app, as the app has it.
set -eu

REPOSITORY=thomasschafer/herdr-app-kiosk
RELEASES=${KIOSK_RELEASES:-"https://github.com/$REPOSITORY/releases"}
MOD_NAME=kiosk

say() { printf 'install: %s\n' "$*" >&2; }
fail() {
    say "$*"
    exit 1
}

from_source=false
version=
while [ $# -gt 0 ]; do
    case $1 in
        --from-source) from_source=true ;;
        --version)
            [ $# -ge 2 ] || fail '--version needs a tag, such as v0.1.0'
            version=$2
            shift
            ;;
        -h | --help)
            sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *) fail "unknown option $1 (see --help)" ;;
    esac
    shift
done

if [ -n "${HERDR_APP_HOME:-}" ]; then
    app_home=$HERDR_APP_HOME
elif [ -n "${XDG_CONFIG_HOME:-}" ]; then
    app_home=$XDG_CONFIG_HOME/herdr-app
else
    [ -n "${HOME:-}" ] || fail 'HOME is not set, so the app home is unknown; set HERDR_APP_HOME'
    app_home=$HOME/.config/herdr-app
fi
mod_dir=$app_home/mods/$MOD_NAME

checkout=
if dir=$(CDPATH='' cd -- "$(dirname -- "$0")/.." 2>/dev/null && pwd) &&
    [ -f "$dir/Cargo.toml" ] && [ -f "$dir/mods/$MOD_NAME/main.js" ]; then
    checkout=$dir
fi
if $from_source && [ -z "$checkout" ]; then
    fail '--from-source needs a checkout: clone the repository and run scripts/install.sh from it'
fi

# A linked checkout is a development install: writing into it would put the binary in the
# checkout. It keeps working as it is.
if [ -L "$mod_dir" ]; then
    fail "$mod_dir is a link (to $(readlink "$mod_dir")); remove it to install a copy instead"
fi

# Staged in the app home, on the same filesystem as the mods, so the swap below is two renames.
mkdir -p "$app_home/mods" || fail "could not create $app_home/mods"
tmp=$(mktemp -d "$app_home/.herdr-app-kiosk-install.XXXXXX") || fail "could not create a temporary directory in $app_home"
previous=$tmp/previous
activated=false
# However the script ends, an earlier install moved aside for the swap below goes back
# unless the new one took its place; staging is only removed once nothing is left in it.
cleanup() {
    if ! $activated && [ -e "$previous" ]; then
        if [ ! -e "$mod_dir" ] && mv -- "$previous" "$mod_dir"; then
            say "put the earlier install back in $mod_dir"
        else
            say "could not put the earlier install back; it is in $previous"
            return
        fi
    fi
    rm -rf -- "$tmp"
}
trap cleanup EXIT
trap 'exit 130' HUP INT TERM

download() {
    if command -v curl >/dev/null 2>&1; then
        curl --fail --location --silent --show-error --output "$2" "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget --quiet --output-document="$2" "$1"
    else
        fail 'curl or wget is needed to download the release'
    fi
}

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{ print $1 }'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{ print $1 }'
    elif command -v openssl >/dev/null 2>&1; then
        openssl dgst -sha256 "$1" | awk '{ print $NF }'
    else
        fail 'no SHA-256 tool (sha256sum, shasum or openssl) is available'
    fi
}

# Checks a downloaded release file against SHA256SUMS and, with the GitHub CLI, the
# release's build attestation.
verify() {
    file=$1
    name=$(basename -- "$file")
    expected=$(awk -v name="$name" '$2 == name || $2 == "*" name { print $1 }' "$tmp/SHA256SUMS" | sed -n '1p')
    [ "${#expected}" -eq 64 ] || fail "SHA256SUMS has no entry for $name"
    actual=$(sha256 "$file")
    [ "$(printf '%s' "$expected" | tr 'A-F' 'a-f')" = "$actual" ] || fail "$name does not match its checksum"
    if [ -n "${KIOSK_SKIP_ATTESTATION:-}" ]; then
        say "verified $name: checksum (KIOSK_SKIP_ATTESTATION skips its build attestation)"
    elif ! command -v gh >/dev/null 2>&1; then
        say "verified $name: checksum (install the GitHub CLI to check its build attestation too)"
    elif ! gh auth status >/dev/null 2>&1; then
        say "verified $name: checksum (log in to the GitHub CLI to check its build attestation too)"
    else
        gh attestation verify "$file" --repo "$REPOSITORY" --signer-workflow "$REPOSITORY/.github/workflows/release.yml" \
            --source-ref "refs/tags/$version" --deny-self-hosted-runners >/dev/null ||
            fail "$name has no valid build attestation from $REPOSITORY's release workflow for $version"
        say "verified $name: checksum and build attestation"
    fi
}

# The release target for this computer, or $KIOSK_TARGET to choose another (such as the
# x86_64 binary under Rosetta).
target() {
    if [ -n "${KIOSK_TARGET:-}" ]; then
        echo "$KIOSK_TARGET"
        return
    fi
    os=$(uname -s 2>/dev/null || true)
    arch=$(uname -m 2>/dev/null || true)
    case "$os:$arch" in
        Linux:x86_64 | Linux:amd64) echo x86_64-unknown-linux-gnu ;;
        Linux:aarch64 | Linux:arm64) echo aarch64-unknown-linux-gnu ;;
        Darwin:x86_64 | Darwin:amd64) echo x86_64-apple-darwin ;;
        Darwin:arm64 | Darwin:aarch64) echo aarch64-apple-darwin ;;
        *) fail "there is no prebuilt binary for $os/$arch; install from source (see the README)" ;;
    esac
}

staged=$tmp/$MOD_NAME
if $from_source; then
    say "building $checkout"
    command -v cargo >/dev/null 2>&1 || fail 'cargo is needed to build from source: https://rustup.rs/'
    (cd "$checkout" && cargo build --locked --release) || fail 'the build failed'
    mkdir -p "$staged/bin"
    cp "$checkout/mods/$MOD_NAME/main.js" "$checkout/mods/$MOD_NAME/mod.json" "$staged/"
    install -m 755 "${CARGO_TARGET_DIR:-$checkout/target}/release/herdr-app-kiosk" "$staged/bin/herdr-app-kiosk"
else
    if [ -z "$version" ]; then
        # The latest release's page redirects to its tag.
        location=$(curl --fail --silent --show-error --head --output /dev/null --write-out '%{redirect_url}' "$RELEASES/latest") ||
            fail "could not find the latest release at $RELEASES"
        version=${location##*/}
    fi
    case $version in
        v[0-9]*) ;;
        *) fail "not a release tag: $version" ;;
    esac
    binary=herdr-app-kiosk-$version-$(target)
    mod=herdr-app-kiosk-mod-$version.tar.gz
    say "downloading herdr-app-kiosk $version"
    for file in SHA256SUMS "$binary" "$mod"; do
        download "$RELEASES/download/$version/$file" "$tmp/$file" || fail "could not download $file from release $version"
    done
    verify "$tmp/$binary"
    verify "$tmp/$mod"
    tar -xzf "$tmp/$mod" -C "$tmp"
    if [ ! -f "$staged/main.js" ] || [ ! -f "$staged/mod.json" ]; then
        fail "$mod does not hold the $MOD_NAME mod"
    fi
    mkdir -p "$staged/bin"
    install -m 755 "$tmp/$binary" "$staged/bin/herdr-app-kiosk"
fi

# Swapped in whole, so the app never loads half of an update. A running picker keeps the
# old binary it has open.
if [ -e "$mod_dir" ]; then
    mv -- "$mod_dir" "$previous" || fail "could not move the installed $mod_dir aside; it is unchanged"
fi
mv -- "$staged" "$mod_dir" || fail "could not install $mod_dir"
activated=true
say "installed $mod_dir"

if command -v herdr-app >/dev/null 2>&1; then
    HERDR_APP_HOME=$app_home herdr-app check "$mod_dir" >&2 || fail "the app could not load $mod_dir"
fi
if [ "$(uname -s 2>/dev/null || true)" = Darwin ]; then
    shortcut=cmd-k
else
    shortcut=ctrl-shift-r
fi
cat >&2 <<EOF
install: bind a key to open it in $app_home/keys.json, for example:
  { "global": { "$shortcut": "mod.open:kiosk" } }
EOF
