#!/usr/bin/env bash
# Activate a saved window policy version (Spec 096).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="${1:?usage: activate-window-policy.sh VERSION}"
export POLY_STRATEGY_CONTROL_DIR="${POLY_STRATEGY_CONTROL_DIR:-$ROOT/var/strategy-control}"
cd "$ROOT"
if [[ ! -x target/release/btc-window-policy ]]; then
  cargo build -q --release -p btc-window-strategy --bin btc-window-policy
fi
exec ./target/release/btc-window-policy activate "$VERSION"
