#!/usr/bin/env bash
# S.A.M Mission Control — Installer
# Usage: curl -fsSL https://raw.githubusercontent.com/tinybluedev/sam-mission-control/main/install.sh | bash
set -euo pipefail

REPO="tinybluedev/sam-mission-control"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"
BIN_NAME="sam"

echo ""
echo "  🛰️  S.A.M Mission Control — Installer"
echo "  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# ── OS Detection ────────────────────────────────────────────────
OS="$(uname -s)"
ARCH="$(uname -m)"
echo "  Detected OS: $OS ($ARCH)"

case "$OS" in
    Linux)   ;;
    Darwin)  ;;
    *)
        echo "  ❌ Unsupported operating system: $OS"
        echo "     S.A.M supports Linux and macOS."
        exit 1
        ;;
esac

# ── Dependency checks ───────────────────────────────────────────
if ! command -v git &>/dev/null; then
    echo "  ❌ git is not installed."
    case "$OS" in
        Linux)  echo "     Install with: sudo apt install git  (Debian/Ubuntu)" ;;
        Darwin) echo "     Install with: brew install git  or  xcode-select --install" ;;
    esac
    exit 1
fi

if ! command -v curl &>/dev/null; then
    echo "  ❌ curl is not installed."
    case "$OS" in
        Linux)  echo "     Install with: sudo apt install curl" ;;
        Darwin) echo "     Install with: brew install curl" ;;
    esac
    exit 1
fi

# ── Rust / Cargo ────────────────────────────────────────────────
if ! command -v cargo &>/dev/null; then
    echo "  ⚠️  Rust not found. Installing via rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --quiet
    # shellcheck source=/dev/null
    source "$HOME/.cargo/env"
    if ! command -v cargo &>/dev/null; then
        echo "  ❌ Rust installation failed. Please install manually: https://rustup.rs"
        exit 1
    fi
    echo "  ✅ Rust installed: $(rustc --version)"
else
    echo "  ✅ Rust found: $(rustc --version)"
fi

# ── Clone ───────────────────────────────────────────────────────
echo "  [1/4] Cloning repository..."
SAM_TMP=$(mktemp -d)
trap 'rm -rf "$SAM_TMP"' EXIT

if ! git clone --depth 1 "https://github.com/$REPO.git" "$SAM_TMP/sam" 2>&1; then
    echo "  ❌ Failed to clone $REPO. Check your internet connection."
    exit 1
fi
cd "$SAM_TMP/sam"

# ── Build ───────────────────────────────────────────────────────
echo "  [2/4] Building (release mode) — this may take a few minutes..."
if ! cargo build --release 2>&1; then
    echo "  ❌ Build failed. See output above for details."
    echo "     Ensure Rust 1.85+ is installed: rustup update stable"
    exit 1
fi

# ── Install ─────────────────────────────────────────────────────
echo "  [3/4] Installing to $INSTALL_DIR/$BIN_NAME..."
if [ ! -d "$INSTALL_DIR" ]; then
    echo "  ❌ Install directory does not exist: $INSTALL_DIR"
    echo "     Override with: INSTALL_DIR=~/.local/bin bash install.sh"
    exit 1
fi

if [ -w "$INSTALL_DIR" ]; then
    cp target/release/sam-mission-control "$INSTALL_DIR/$BIN_NAME"
else
    echo "  ℹ️  $INSTALL_DIR is not writable. Trying sudo..."
    if ! sudo cp target/release/sam-mission-control "$INSTALL_DIR/$BIN_NAME"; then
        echo "  ❌ Installation failed. Try:"
        echo "     INSTALL_DIR=~/.local/bin bash install.sh"
        exit 1
    fi
fi

# Verify the binary is accessible
if ! command -v "$BIN_NAME" &>/dev/null; then
    echo "  ⚠️  $INSTALL_DIR is not in your PATH."
    echo "     Add it with: export PATH=\"$INSTALL_DIR:\$PATH\""
fi

# ── Done ────────────────────────────────────────────────────────
echo "  [4/4] Done."
echo ""
echo "  ✅ Installed $BIN_NAME $(target/release/sam-mission-control version 2>/dev/null | head -1 || true)"
echo ""
echo "  Quick start:"
echo "    sam init --db-host <mysql-ip> --db-pass '<password>'"
echo "    sam"
echo ""
echo "  Add agents:"
echo "    sam onboard <agent-ip>"
echo ""
