#!/bin/sh
# Switch between the current stable notion-river binary and the native output
# management candidate prepared in ~/.local/bin/notion-river-test/.
#
# Usage:
#   scripts/test-native-output-management.sh install
#   scripts/test-native-output-management.sh rollback
#
# This script only swaps the binary atomically. It does not restart River,
# kill notion-river, run wlr-randr, or touch monitor state.

set -eu

BIN="$HOME/.local/bin/notion-river"
DIR="$HOME/.local/bin/notion-river-test"

case "${1:-}" in
    install)
        cp "$DIR/notion-river.native" "$BIN.new"
        mv "$BIN.new" "$BIN"
        echo "Installed native output-management candidate. Restart notion-river manually."
        ;;
    rollback)
        cp "$DIR/notion-river.rollback" "$BIN.new"
        mv "$BIN.new" "$BIN"
        echo "Restored rollback binary. Restart notion-river manually."
        ;;
    *)
        echo "usage: $0 install|rollback" >&2
        exit 2
        ;;
esac
