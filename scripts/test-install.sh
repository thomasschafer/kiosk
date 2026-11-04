#!/usr/bin/env bash
# Tests install.sh against a fake release, served the way GitHub serves releases, and a fake
# app.
set -euo pipefail

PROJECT_ROOT=$(cd "$(dirname "$0")/.." && pwd)
TEST_ROOT=${KIOSK_INSTALL_TEST_ROOT:-/tmp/herdr-app-kiosk-install-test}
case "$TEST_ROOT" in
    /tmp/* | /private/tmp/*) ;;
    *)
        printf 'KIOSK_INSTALL_TEST_ROOT must be below /tmp or /private/tmp: %s\n' "$TEST_ROOT" >&2
        exit 2
        ;;
esac

server=
cleanup() {
    [[ -n $server ]] && kill "$server" 2>/dev/null || true
    rm -rf -- "$TEST_ROOT"
}
trap cleanup EXIT
cleanup

VERSION=v$(sed -n 's/^version = "\([^"]*\)"$/\1/p' "$PROJECT_ROOT/Cargo.toml" | head -n 1)
TARGET=x86_64-unknown-linux-gnu
releases=$TEST_ROOT/releases
mkdir -p "$releases/download/$VERSION" "$TEST_ROOT/bin" "$TEST_ROOT/mod/kiosk"

binary=herdr-app-kiosk-$VERSION-$TARGET
printf '#!/bin/sh\necho released\n' >"$releases/download/$VERSION/$binary"
cp "$PROJECT_ROOT/mods/kiosk/main.js" "$PROJECT_ROOT/mods/kiosk/mod.json" "$TEST_ROOT/mod/kiosk/"
tar -czf "$releases/download/$VERSION/herdr-app-kiosk-mod-$VERSION.tar.gz" -C "$TEST_ROOT/mod" kiosk
(
    cd "$releases/download/$VERSION"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum herdr-app-kiosk-* >SHA256SUMS
    else
        shasum -a 256 herdr-app-kiosk-* >SHA256SUMS
    fi
)

# GitHub answers /releases/latest with a redirect to the latest tag; this does the same.
port_file=$TEST_ROOT/port
python3 - "$releases" "$VERSION" "$port_file" <<'PY' &
import functools, http.server, sys
root, version, port_file = sys.argv[1:]
class Handler(http.server.SimpleHTTPRequestHandler):
    def do_HEAD(self):
        if self.path == "/latest":
            self.send_response(302)
            self.send_header("Location", f"/tag/{version}")
            self.end_headers()
            return
        super().do_HEAD()
    def do_GET(self):
        if self.path == "/latest":
            return self.do_HEAD()
        super().do_GET()
    def log_message(self, *args):
        pass
server = http.server.HTTPServer(("127.0.0.1", 0), functools.partial(Handler, directory=root))
open(port_file, "w").write(str(server.server_port))
server.serve_forever()
PY
server=$!
for _ in $(seq 100); do [[ -s $port_file ]] && break; sleep 0.05; done
[[ -s $port_file ]] || { echo "FAIL: the fake release server did not start" >&2; exit 1; }

cat >"$TEST_ROOT/bin/herdr-app" <<SH
#!/bin/sh
printf '%s %s\n' "\$HERDR_APP_HOME" "\$*" >>"$TEST_ROOT/herdr-app-calls"
SH
chmod +x "$TEST_ROOT/bin/herdr-app"
port=$(cat "$port_file")
export PATH="$TEST_ROOT/bin:$PATH" KIOSK_RELEASES="http://127.0.0.1:$port" \
    KIOSK_TARGET=$TARGET KIOSK_SKIP_ATTESTATION=1 HERDR_APP_HOME="$TEST_ROOT/default-home"

fail() {
    printf 'FAIL: %s\n' "$*" >&2
    exit 1
}

assert_installed() {
    local mod=$1/mods/kiosk
    [[ -x $mod/bin/herdr-app-kiosk ]] || fail "no executable binary in $mod"
    cmp -s "$mod/bin/herdr-app-kiosk" "$releases/download/$VERSION/$binary" || fail "$mod has a different binary"
    cmp -s "$mod/main.js" "$PROJECT_ROOT/mods/kiosk/main.js" || fail "$mod has a different main.js"
    cmp -s "$mod/mod.json" "$PROJECT_ROOT/mods/kiosk/mod.json" || fail "$mod has a different mod.json"
    grep -qx "$1 check $mod" "$TEST_ROOT/herdr-app-calls" || fail "the app did not check $mod"
}

installer=$TEST_ROOT/install.sh
cp "$PROJECT_ROOT/scripts/install.sh" "$installer"

printf 'test: installs the latest release, verified\n'
home=$TEST_ROOT/home
HERDR_APP_HOME=$home sh "$installer" >"$TEST_ROOT/out" 2>&1 || fail "$(cat "$TEST_ROOT/out")"
grep -q "downloading herdr-app-kiosk $VERSION" "$TEST_ROOT/out" || fail "did not find $VERSION: $(cat "$TEST_ROOT/out")"
grep -q "verified $binary: checksum" "$TEST_ROOT/out" || fail "the binary was not verified: $(cat "$TEST_ROOT/out")"
assert_installed "$home"

printf 'test: reinstalling a given release replaces the whole folder\n'
touch "$home/mods/kiosk/stale.js"
HERDR_APP_HOME=$home sh "$installer" --version "$VERSION" >/dev/null 2>&1
[[ ! -e $home/mods/kiosk/stale.js ]] || fail "a stale file survived"
[[ ! -e $home/mods/kiosk.old ]] || fail "the old install was left behind"
assert_installed "$home"

printf 'test: the app home follows XDG_CONFIG_HOME\n'
env -u HERDR_APP_HOME XDG_CONFIG_HOME="$TEST_ROOT/xdg" sh "$installer" >/dev/null 2>&1
assert_installed "$TEST_ROOT/xdg/herdr-app"

printf 'test: a file that does not match its checksum is refused\n'
tampered=$TEST_ROOT/tampered
cp -R "$releases" "$tampered"
printf 'tampered\n' >>"$tampered/download/$VERSION/$binary"
if HERDR_APP_HOME=$TEST_ROOT/tampered-home KIOSK_RELEASES="file://$tampered" sh "$installer" --version "$VERSION" 2>"$TEST_ROOT/out"; then
    fail "installed a tampered binary"
fi
grep -q 'does not match its checksum' "$TEST_ROOT/out" || fail "unclear refusal: $(cat "$TEST_ROOT/out")"
[[ ! -e $TEST_ROOT/tampered-home/mods/kiosk ]] || fail "installed something anyway"

printf 'test: with the GitHub CLI, build attestations are checked too\n'
gh_bin=$TEST_ROOT/gh-bin
mkdir -p "$gh_bin"
cat >"$gh_bin/gh" <<SH
#!/bin/sh
printf '%s\n' "\$*" >>"$TEST_ROOT/gh-calls"
[ -z "\${FAKE_GH_REFUSES:-}" ]
SH
chmod +x "$gh_bin/gh"
attested=$TEST_ROOT/attested
env -u KIOSK_SKIP_ATTESTATION PATH="$gh_bin:$PATH" HERDR_APP_HOME="$attested" sh "$installer" >"$TEST_ROOT/out" 2>&1 ||
    fail "$(cat "$TEST_ROOT/out")"
grep -q "verified $binary: checksum and build attestation" "$TEST_ROOT/out" || fail "no attestation check: $(cat "$TEST_ROOT/out")"
grep -q "attestation verify .*$binary --repo thomasschafer/herdr-app-kiosk" "$TEST_ROOT/gh-calls" ||
    fail "gh was asked: $(cat "$TEST_ROOT/gh-calls")"
assert_installed "$attested"
if env -u KIOSK_SKIP_ATTESTATION PATH="$gh_bin:$PATH" FAKE_GH_REFUSES=1 HERDR_APP_HOME="$TEST_ROOT/unattested" \
    sh "$installer" 2>"$TEST_ROOT/out"; then
    fail "installed a binary without a valid attestation"
fi
grep -q 'has no valid build attestation' "$TEST_ROOT/out" || fail "unclear refusal: $(cat "$TEST_ROOT/out")"
[[ ! -e $TEST_ROOT/unattested/mods/kiosk ]] || fail "installed something anyway"

printf 'test: a linked development install is left alone\n'
linked=$TEST_ROOT/linked
mkdir -p "$linked/mods"
ln -s "$PROJECT_ROOT/mods/kiosk" "$linked/mods/kiosk"
if HERDR_APP_HOME=$linked sh "$installer" 2>"$TEST_ROOT/out"; then
    fail "installed over a link"
fi
grep -q 'is a link' "$TEST_ROOT/out" || fail "unclear refusal: $(cat "$TEST_ROOT/out")"
[[ ! -e $PROJECT_ROOT/mods/kiosk/bin ]] || fail "wrote into the linked checkout"

printf 'test: options are checked\n'
if sh "$installer" --frobnicate 2>"$TEST_ROOT/out"; then fail "took an unknown option"; fi
grep -q 'unknown option --frobnicate' "$TEST_ROOT/out" || fail "unclear error: $(cat "$TEST_ROOT/out")"
if sh "$installer" --from-source 2>"$TEST_ROOT/out"; then fail "built without a checkout"; fi
grep -q 'needs a checkout' "$TEST_ROOT/out" || fail "unclear error: $(cat "$TEST_ROOT/out")"
if sh "$installer" --version latest 2>"$TEST_ROOT/out"; then fail "took a version that is not a tag"; fi
grep -q 'not a release tag' "$TEST_ROOT/out" || fail "unclear error: $(cat "$TEST_ROOT/out")"

printf 'install tests passed\n'
