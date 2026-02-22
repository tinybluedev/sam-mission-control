#!/usr/bin/env bash
# S.A.M Mission Control — Installer
# Usage: curl -fsSL https://raw.githubusercontent.com/tinybluedev/sam-mission-control/main/install.sh | bash
set -euo pipefail

REPO="tinybluedev/sam-mission-control"
INSTALL_DIR="/usr/local/bin"
BIN_NAME="sam"

echo ""
echo "  🛰️  S.A.M Mission Control — Installer"
echo "  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Check for Rust
if ! command -v cargo &>/dev/null; then
    echo "  ⚠️  Rust not found. Installing via rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
fi

echo "  [1/4] Cloning repository..."
TMPDIR=$(mktemp -d)
git clone --depth 1 "https://github.com/$REPO.git" "$TMPDIR/sam" 2>/dev/null
cd "$TMPDIR/sam"

echo "  [2/4] Building (release mode)..."
cargo build --release --quiet 2>/dev/null

echo "  [3/4] Installing to $INSTALL_DIR/$BIN_NAME..."
if [ -w "$INSTALL_DIR" ]; then
    cp target/release/sam-mission-control "$INSTALL_DIR/$BIN_NAME"
else
    sudo cp target/release/sam-mission-control "$INSTALL_DIR/$BIN_NAME"
fi

echo "  [4/4] Cleaning up..."
rm -rf "$TMPDIR"

echo ""
echo "  ✅ Installed! Run 'sam init' to set up your fleet."
echo ""
echo "  Quick start:"
echo "    sam init --db-host <mysql-ip> --db-pass '<password>'"
echo "    sam"
echo ""
echo "  Add agents:"
echo "    sam onboard <agent-ip>"
echo ""
