#!/usr/bin/env bash
# scan-secrets.sh — Run gitleaks secret scanning locally
# Usage: bash scripts/scan-secrets.sh [--staged]
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CONFIG="$REPO_ROOT/.gitleaks.toml"

if ! command -v gitleaks &>/dev/null; then
    echo "❌  gitleaks not found. Install it first:"
    echo "    macOS:  brew install gitleaks"
    echo "    Linux:  https://github.com/gitleaks/gitleaks/releases"
    exit 1
fi

echo "🔍  Running gitleaks secret scan…"

EXIT_CODE=0
if [[ "${1:-}" == "--staged" ]]; then
    echo "    Mode: staged files only"
    gitleaks protect --staged --config="$CONFIG" --source="$REPO_ROOT" --verbose || EXIT_CODE=$?
else
    echo "    Mode: full repository history"
    gitleaks detect --config="$CONFIG" --source="$REPO_ROOT" --verbose || EXIT_CODE=$?
fi

if [ "$EXIT_CODE" -eq 0 ]; then
    echo "✅  No secrets detected."
else
    echo "❌  Secrets detected — resolve the findings above before committing."
fi
exit "$EXIT_CODE"
