#!/usr/bin/env bash
# S.A.M Mission Control — Installer
# curl -fsSL https://raw.githubusercontent.com/tinybluedev/sam-mission-control/main/install.sh | bash
set -euo pipefail

REPO="tinybluedev/sam-mission-control"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"
BIN_NAME="sam"
BUILD_LOG="/tmp/sam-build-$(date +%s).log"

# ── Colors ──────────────────────────────────────────────────────
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
CYAN='\033[0;36m'; BOLD='\033[1m'; DIM='\033[2m'; RESET='\033[0m'
BLUE='\033[0;34m'; MAGENTA='\033[0;35m'

# ── Spinner ──────────────────────────────────────────────────────
SPIN_FRAMES=('⠋' '⠙' '⠹' '⠸' '⠼' '⠴' '⠦' '⠧' '⠇' '⠏')
SPIN_PID=""
spin_start() {
    local msg="$1"
    ( i=0; while true; do
        printf "\r  ${CYAN}${SPIN_FRAMES[$((i % 10))]}${RESET}  ${msg}   "
        sleep 0.08; ((i++))
    done ) &
    SPIN_PID=$!
}
spin_stop() {
    if [ -n "$SPIN_PID" ]; then kill "$SPIN_PID" 2>/dev/null; SPIN_PID=""; fi
    printf "\r\033[K"
}
ok()   { spin_stop; echo -e "  ${GREEN}✓${RESET}  $1"; }
fail() { spin_stop; echo -e "  ${RED}✗${RESET}  $1"; exit 1; }
info() { echo -e "  ${DIM}·${RESET}  $1"; }

# ── Detect distro ────────────────────────────────────────────────
detect_os() {
    local OS ARCH DISTRO
    OS="$(uname -s)"
    ARCH="$(uname -m)"
    if [ "$OS" = "Linux" ]; then
        if [ -f /etc/os-release ]; then
            # shellcheck source=/dev/null
            . /etc/os-release
            DISTRO="${PRETTY_NAME:-Linux}"
        elif command -v lsb_release &>/dev/null; then
            DISTRO="$(lsb_release -ds 2>/dev/null || echo Linux)"
        else
            DISTRO="Linux"
        fi
    elif [ "$OS" = "Darwin" ]; then
        DISTRO="macOS $(sw_vers -productVersion 2>/dev/null || true)"
    else
        fail "Unsupported OS: $OS — S.A.M supports Linux and macOS"
    fi
    echo "$DISTRO ($ARCH)"
}

# ── ASCII Banner ─────────────────────────────────────────────────
clear
echo ""
echo -e "${CYAN}${BOLD}"
cat << 'LOGO'
   ╔═══════════════════════════════════════════════════════╗
   ║                                                       ║
   ║    ███████╗ █████╗ ███╗   ███╗                        ║
   ║    ██╔════╝██╔══██╗████╗ ████║                        ║
   ║    ███████╗███████║██╔████╔██║   Mission Control      ║
   ║    ╚════██║██╔══██║██║╚██╔╝██║                        ║
   ║    ███████║██║  ██║██║ ╚═╝ ██║   Fleet Orchestration  ║
   ║    ╚══════╝╚═╝  ╚═╝╚═╝     ╚═╝                        ║
   ║                                                       ║
   ║         Strange Artificial Machine  v2.0              ║
   ║                                                       ║
   ╚═══════════════════════════════════════════════════════╝
LOGO
echo -e "${RESET}"
sleep 0.4

OS_LABEL="$(detect_os)"
info "System:  ${BOLD}${OS_LABEL}${RESET}"
info "User:    ${BOLD}${USER:-$(whoami)}${RESET}"
info "Install: ${BOLD}${INSTALL_DIR}/${BIN_NAME}${RESET}"
echo ""

# ── Git ──────────────────────────────────────────────────────────
if ! command -v git &>/dev/null; then
    fail "git not found — install git and re-run"
fi

# ── Rust / Cargo ─────────────────────────────────────────────────
if ! command -v cargo &>/dev/null; then
    spin_start "Installing Rust toolchain via rustup..."
    if ! curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --quiet >> "$BUILD_LOG" 2>&1; then
        fail "Rust install failed — see $BUILD_LOG"
    fi
    # shellcheck source=/dev/null
    source "$HOME/.cargo/env" 2>/dev/null || true
    ok "Rust installed: $(rustc --version 2>/dev/null | cut -d' ' -f1-2)"
else
    ok "Rust: $(rustc --version 2>/dev/null | cut -d' ' -f1-2)"
fi

# ── Clone ────────────────────────────────────────────────────────
SAM_TMP=$(mktemp -d)
trap 'rm -rf "$SAM_TMP"' EXIT

spin_start "Cloning sam-mission-control..."
if ! git clone --depth 1 "https://github.com/$REPO.git" "$SAM_TMP/sam" >> "$BUILD_LOG" 2>&1; then
    fail "Clone failed — check internet connection"
fi
ok "Repository cloned"
cd "$SAM_TMP/sam"

# ── Build (silent, with animated progress) ───────────────────────
spin_start "Compiling release binary (this takes 2–5 min on first run)..."
if ! cargo build --release >> "$BUILD_LOG" 2>&1; then
    spin_stop
    echo -e "\n  ${RED}✗  Build failed.${RESET} Full log: ${BUILD_LOG}"
    echo ""
    tail -20 "$BUILD_LOG" | sed 's/^/     /'
    echo ""
    fail "Try: rustup update stable && re-run installer"
fi
ok "Binary compiled"

# ── Install ──────────────────────────────────────────────────────
spin_start "Installing to ${INSTALL_DIR}/${BIN_NAME}..."
if [ -w "$INSTALL_DIR" ]; then
    cp target/release/sam-mission-control "$INSTALL_DIR/$BIN_NAME"
else
    if ! sudo cp target/release/sam-mission-control "$INSTALL_DIR/$BIN_NAME" >> "$BUILD_LOG" 2>&1; then
        fail "Install failed — try: INSTALL_DIR=~/.local/bin bash install.sh"
    fi
fi
ok "Installed: ${INSTALL_DIR}/${BIN_NAME}"

# ── PATH check ───────────────────────────────────────────────────
if ! command -v "$BIN_NAME" &>/dev/null; then
    echo ""
    echo -e "  ${YELLOW}⚠${RESET}  ${INSTALL_DIR} is not in your PATH."
    echo -e "     Add this to your shell config:"
    echo -e "     ${CYAN}export PATH=\"${INSTALL_DIR}:\$PATH\"${RESET}"
fi

# ── Done ─────────────────────────────────────────────────────────
echo ""
echo -e "  ${GREEN}${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
echo -e "  ${GREEN}${BOLD}  S.A.M Mission Control is installed ✓ ${RESET}"
echo -e "  ${GREEN}${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
echo ""
echo -e "  Launch the first-run setup:"
echo ""
echo -e "     ${CYAN}${BOLD}sam init${RESET}"
echo ""
echo -e "  ${DIM}sam init will guide you through everything — no manual config needed.${RESET}"
echo ""
