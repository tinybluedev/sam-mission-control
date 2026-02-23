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

# ── Animated Timeline (build/install) ──────────────────────────────
TIMELINE_LABELS=(
    "Compile release binary"
    "Install binary"
)
# Rough first-run estimates on commodity laptops/VMs; actual time varies by hardware.
TIMELINE_ETA_SECS=(240 20)
TIMELINE_STATUS=("pending" "pending")
TIMELINE_LINES=0
TIMELINE_UPDATE_INTERVAL=0.2

fmt_eta() {
    local sec="$1"
    if [ "$sec" -lt 60 ]; then
        printf "~%ss" "$sec"
    else
        printf "~%sm%02ss" $((sec / 60)) $((sec % 60))
    fi
}

timeline_render() {
    local frame="$1"
    local active_idx="${2:-}"
    local active_left="${3:-0}"
    local idx dot eta

    if [ -t 1 ] && [ "$TIMELINE_LINES" -gt 0 ]; then
        # Move cursor up to redraw the timeline in place (avoids log-dump output).
        printf "\033[%sA" "$TIMELINE_LINES"
    fi

    echo -e "  ${BOLD}Progress Timeline${RESET}"
    for idx in "${!TIMELINE_LABELS[@]}"; do
        case "${TIMELINE_STATUS[$idx]}" in
            running)
                dot="${CYAN}${SPIN_FRAMES[$((frame % ${#SPIN_FRAMES[@]}))]}${RESET}"
                eta="$(fmt_eta "$active_left") left"
                ;;
            done)
                dot="${GREEN}●${RESET}"
                eta="done"
                ;;
            fail)
                dot="${RED}●${RESET}"
                eta="failed"
                ;;
            *)
                dot="${DIM}○${RESET}"
                eta="$(fmt_eta "${TIMELINE_ETA_SECS[$idx]}")"
                ;;
        esac
        echo -e "    ${dot} ${TIMELINE_LABELS[$idx]} ${DIM}(${eta})${RESET}"
    done

    TIMELINE_LINES=$(( ${#TIMELINE_LABELS[@]} + 1 ))
}

timeline_run() {
    local idx="$1"
    local eta="$2"
    shift 2
    local cmd=("$@")
    local start now elapsed left frame=0

    TIMELINE_STATUS[$idx]="running"
    start=$(date +%s)
    "${cmd[@]}" >> "$BUILD_LOG" 2>&1 &
    local pid=$!
    while kill -0 "$pid" 2>/dev/null; do
        now=$(date +%s)
        elapsed=$((now - start))
        left=$((eta - elapsed))
        if [ "$left" -lt 0 ]; then left=0; fi
        timeline_render "$frame" "$idx" "$left"
        sleep "$TIMELINE_UPDATE_INTERVAL"
        frame=$((frame + 1))
    done

    if wait "$pid"; then
        TIMELINE_STATUS[$idx]="done"
        timeline_render "$frame" "$idx" 0
        return 0
    fi

    TIMELINE_STATUS[$idx]="fail"
    timeline_render "$frame" "$idx" 0
    return 1
}

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
timeline_render 0
if ! timeline_run 0 "${TIMELINE_ETA_SECS[0]}" cargo build --release; then
    echo -e "\n  ${RED}✗  Build failed.${RESET} Full log: ${BUILD_LOG}"
    echo ""
    tail -20 "$BUILD_LOG" | sed 's/^/     /'
    echo ""
    fail "Try: rustup update stable && re-run installer"
fi

# ── Install ──────────────────────────────────────────────────────
if [ -w "$INSTALL_DIR" ]; then
    if ! timeline_run 1 "${TIMELINE_ETA_SECS[1]}" cp target/release/sam-mission-control "$INSTALL_DIR/$BIN_NAME"; then
        fail "Install failed — try: INSTALL_DIR=~/.local/bin bash install.sh"
    fi
else
    if ! timeline_run 1 "${TIMELINE_ETA_SECS[1]}" sudo cp target/release/sam-mission-control "$INSTALL_DIR/$BIN_NAME"; then
        fail "Install failed — try: INSTALL_DIR=~/.local/bin bash install.sh"
    fi
fi
echo ""
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
