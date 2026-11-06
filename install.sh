#!/bin/sh
# Bootstrap a verified, platform-specific sail-lsp distribution. No Rust,
# Node.js or Python is needed unless --from-source is selected.
set -eu
fail() { printf '%s\n' "sail-lsp: $*" >&2; exit 1; }
version=latest
prefix=${SAIL_LSP_HOME:-${XDG_DATA_HOME:-$HOME/.local/share}/sail-lsp}
base=${SAIL_LSP_RELEASE_BASE_URL:-https://github.com/TinyuengKwan/sail-lsp/releases}
archive=
checksum=
source_dir=
revision=
from_source=false
lifecycle=false
expect=
for argument do
    if [ -n "$expect" ]; then
        case "$expect" in
            --version) version=$argument;; --prefix) prefix=$argument;; --archive) archive=$argument;;
            --sha256) checksum=$argument;; --path) source_dir=$argument;; --rev) revision=$argument;;
            --release-base-url) base=$argument;;
        esac
        expect=
        continue
    fi
    case "$argument" in
        --from-source) from_source=true;; --path|--rev) from_source=true; expect=$argument;;
        --uninstall|--rollback|--list) lifecycle=true;;
        --version|--prefix|--archive|--sha256|--release-base-url|--config-home|--editor-root|--editor|--vsix|--code-command|--profile) expect=$argument;;
        --help|-h)
            cat <<'HELP'
Install:  sh install.sh [--version TAG] [--prefix DIR] [--editor EDITOR]
Source:   sh install.sh --from-source [--rev REF | --path CHECKOUT]
Offline:  sh install.sh --archive FILE [--sha256 HASH]
Manage:   sh install.sh --list | --rollback [--version ID] | --uninstall
Options:  --no-modify-path --profile FILE --config-home DIR --editor-root DIR
          --vsix FILE --code-command PATH --release-base-url URL
Editors:  vscode, zed, neovim, helix, emacs (comma-separated or repeated)
HELP
            exit 0;;
    esac
done
[ -z "$expect" ] || fail "$expect requires a value"
case "$version" in ''|-*|*[!a-zA-Z0-9._-]*) fail 'invalid version';; esac
case "$revision" in -*) fail 'invalid revision';; esac
work=$(mktemp -d "${TMPDIR:-/tmp}/sail-lsp-install.XXXXXXXX")
trap 'rm -rf "$work"' EXIT HUP INT TERM
if [ "$lifecycle" = true ]; then
    [ -x "$prefix/bin/sail-lsp" ] || fail "no managed executable at $prefix/bin/sail-lsp"
    cp "$prefix/bin/sail-lsp" "$work/sail-lsp"
    "$work/sail-lsp" install "$@"
    exit $?
fi
if [ "$from_source" = true ]; then
    command -v cargo >/dev/null 2>&1 || fail 'source installation requires Rust (cargo), CMake and a C++20 compiler with std::format support (for example GCC 13+)'
    if [ -z "$source_dir" ]; then
        script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
        if [ -f "$script_dir/crates/sail-lsp/Cargo.toml" ] && [ -z "$revision" ] && [ "$version" = latest ]; then
            source_dir=$script_dir
        else
            command -v git >/dev/null 2>&1 || fail 'source installation requires git'
            source_dir=$work/source
            mkdir -p "$source_dir"
            ref=${revision:-$version}
            [ "$ref" != latest ] || ref=main
            git -C "$source_dir" init --quiet
            git -C "$source_dir" remote add origin https://github.com/TinyuengKwan/sail-lsp.git
            git -C "$source_dir" fetch --quiet --depth 1 origin "$ref"
            git -C "$source_dir" checkout --quiet --detach FETCH_HEAD
        fi
    fi
    source_dir=$(CDPATH= cd -- "$source_dir" && pwd)
    [ -f "$source_dir/crates/sail-lsp/Cargo.toml" ] || fail 'not a sail-lsp checkout'
    if [ -n "$revision" ]; then
        expected=$(git -C "$source_dir" rev-parse --verify "$revision^{commit}") || fail 'cannot resolve --rev'
        actual=$(git -C "$source_dir" rev-parse HEAD) || fail 'cannot determine checkout revision'
        [ "$expected" = "$actual" ] || fail 'local checkout does not match --rev'
        status=$(git -C "$source_dir" status --porcelain --untracked-files=normal) || fail 'cannot inspect checkout'
        [ -z "$status" ] || fail '--rev requires a clean checkout; omit it for working-tree builds'
    fi
    (cd "$source_dir" && cargo build --release --locked --features vendored --bin sail-lsp)
    target_dir=${CARGO_TARGET_DIR:-$source_dir/target}
    case "$target_dir" in /*) ;; *) target_dir=$source_dir/$target_dir;; esac
    if [ -n "${CARGO_BUILD_TARGET:-}" ]; then target_dir=$target_dir/$CARGO_BUILD_TARGET; fi
    cp "$target_dir/release/sail-lsp" "$work/source-installer"
    "$work/source-installer" install "$@" --path "$source_dir"
    exit $?
fi
case "$(uname -s)" in Linux) os=unknown-linux-gnu;; Darwin) os=apple-darwin;; *) fail 'use install.ps1 on Windows; this bootstrap supports Linux and macOS';; esac
case "$(uname -m)" in x86_64|amd64) arch=x86_64;; arm64|aarch64) arch=aarch64;; *) fail 'no prebuilt binary for this architecture; use --from-source';; esac
target=$arch-$os
fetch() {
    case "$1" in
        file://*) cp "${1#file://}" "$2";;
        https://*) command -v curl >/dev/null 2>&1 || fail 'curl is required'
            curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fLsS --connect-timeout 15 --max-time 300 "$1" -o "$2";;
        *) fail 'release URLs must use HTTPS or an explicit file:// offline mirror';;
    esac
}
if [ -z "$archive" ]; then
    asset=sail-lsp-$target.tar.gz
    if [ "$version" = latest ]; then location=$base/latest/download; else location=$base/download/$version; fi
    archive=$work/$asset
    fetch "$location/$asset" "$archive"
    fetch "$location/$asset.sha256" "$work/checksum"
    checksum=$(awk 'NR==1 {print $1}' "$work/checksum")
else
    [ -f "$archive" ] || fail "archive does not exist: $archive"
    if [ -z "$checksum" ]; then
        [ -f "$archive.sha256" ] || fail 'offline archives require a .sha256 sidecar or --sha256'
        checksum=$(awk 'NR==1 {print $1}' "$archive.sha256")
    fi
fi
[ "${#checksum}" -eq 64 ] || fail 'invalid SHA256'
case "$checksum" in *[!a-fA-F0-9]*) fail 'invalid SHA256';; esac
if command -v sha256sum >/dev/null 2>&1; then actual=$(sha256sum "$archive" | awk '{print $1}')
elif command -v shasum >/dev/null 2>&1; then actual=$(shasum -a 256 "$archive" | awk '{print $1}')
elif command -v openssl >/dev/null 2>&1; then actual=$(openssl dgst -sha256 "$archive" | awk '{print $NF}')
else fail 'a SHA256 tool (sha256sum, shasum or openssl) is required'; fi
[ "$(printf '%s' "$actual" | tr '[:upper:]' '[:lower:]')" = "$(printf '%s' "$checksum" | tr '[:upper:]' '[:lower:]')" ] || fail 'archive SHA256 mismatch'
# Check before extraction, including link/special-file entries. The Rust
# installer rechecks every entry, the checksum and executable provenance.
tar -tzf "$archive" > "$work/entries"
awk '/^\// || /(^|\/)\.\.($|\/)/ || /\\/ {exit 1}' "$work/entries" || fail 'unsafe archive path'
tar -tvzf "$archive" > "$work/types"
awk 'substr($0,1,1)!="-" && substr($0,1,1)!="d" {exit 1}' "$work/types" || fail 'archive contains links or special files'
mkdir "$work/unpacked"
tar -xzf "$archive" -C "$work/unpacked"
[ -f "$work/unpacked/manifest.json" ] && [ -f "$work/unpacked/sail-lsp" ] || fail 'incomplete distribution'
chmod u+x "$work/unpacked/sail-lsp"
"$work/unpacked/sail-lsp" install "$@" --archive "$archive" --sha256 "$checksum"
