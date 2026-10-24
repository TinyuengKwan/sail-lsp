#!/bin/sh
# Build the server the way it is distributed, and put it where editors look.
#
# The binary that shipped in `editors/code/server/` was built against
# `libz3.so.4.16` and started on a machine carrying `libz3.so.4`; it died
# before its first message, which reads exactly like "no diagnostics". Two
# editors on this machine were running servers months out of date at the
# same time, and a whole day went into diagnosing behaviour that had been
# fixed in the tree. Hence one script: `--features vendored` (Z3 compiled
# from source and linked statically), the linkage checked rather than
# assumed, and every location updated together.
#
#     scripts/install-server.sh [PREFIX]     # default ~/.local/bin
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
PREFIX=${1:-$HOME/.local/bin}
TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/target}

echo "==> building (release, vendored Z3) — first run compiles Z3, ~6 min"
( cd "$ROOT" && cargo build --release -p sail-lsp --features vendored )
BIN=$TARGET_DIR/release/sail-lsp
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }

# A distributed server may depend on libc and libstdc++ and nothing else.
if command -v ldd >/dev/null 2>&1; then
    if ldd "$BIN" | grep -q z3; then
        echo "FAIL: the binary links libz3 — it will not start where that exact" >&2
        echo "      version is missing. Was --features vendored dropped?" >&2
        ldd "$BIN" | grep z3 >&2
        exit 1
    fi
    echo "==> linkage: no libz3 (statically linked)"
fi

mkdir -p "$PREFIX"
install -m 755 "$BIN" "$PREFIX/sail-lsp"
echo "==> installed $PREFIX/sail-lsp"

# The VS Code extension prefers its bundled binary over $PATH, so a stale
# one there wins over anything installed above.
BUNDLED=$ROOT/editors/code/server
mkdir -p "$BUNDLED"
install -m 755 "$BIN" "$BUNDLED/sail-lsp"
echo "==> updated $BUNDLED/sail-lsp (the VS Code extension's own copy)"

case ":$PATH:" in
    *":$PREFIX:"*) ;;
    *) echo "note: $PREFIX is not on PATH; set \`sail-lsp.server.path\` instead" ;;
esac

echo
echo "Restart the language server for this to take effect —"
echo "an editor keeps the process it started."
