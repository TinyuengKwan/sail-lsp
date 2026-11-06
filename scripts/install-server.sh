#!/bin/sh
# Compatibility entry point for the managed source installer.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
case "${1:-}" in
    ''|--*) ;;
    *) echo 'Use --prefix INSTALL_ROOT; the old positional binary directory is no longer accepted.' >&2; exit 1;;
esac
exec sh "$root/install.sh" --from-source --path "$root" "$@"
