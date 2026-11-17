#!/bin/bash
# install.sh — build and install lanbeam.
#
#   - builds the Rust binary (cargo build --release)
#   - installs ~/.local/bin/lanbeam
#   - registers ~/.local/share/nautilus/scripts/Share on LAN
#
# Run from the lanbeam project directory.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

BIN_DIR="$HOME/.local/bin"
NAUTILUS_SCRIPT_DIR="$HOME/.local/share/nautilus/scripts"
MAN_PAGE_DIR="$HOME/.local/share/man/man1"

BINARY="target/release/lanbeam"
DEST_BIN="$BIN_DIR/lanbeam"
DEST_SCRIPT="$NAUTILUS_SCRIPT_DIR/Share on LAN"

echo "[lanbeam] building release binary..."
cargo build --release --manifest-path "$SCRIPT_DIR/Cargo.toml"

echo "[lanbeam] installing binary to $DEST_BIN"
mkdir -p "$BIN_DIR"
install -m 0755 "$BINARY" "$DEST_BIN"

echo "[lanbeam] registering Nautilus script at $DEST_SCRIPT"
mkdir -p "$NAUTILUS_SCRIPT_DIR"
install -m 0755 "$SCRIPT_DIR/nautilus/Share on LAN" "$DEST_SCRIPT"

echo "[lanbeam] install complete."
echo "  Run:   $DEST_BIN send <path>"
echo "  GUI:   $DEST_BIN send <path> --gui"
echo "  Script: $DEST_SCRIPT"
