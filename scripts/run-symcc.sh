#!/usr/bin/env bash
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
default_root="$HOME/.local/opt/cvc5-1.3.1"
if [[ -z "${CVC5:-}" ]]; then
  CVC5=$(find "${CVC5_INSTALL_DIR:-$default_root}" -type f -name cvc5 -print -quit 2>/dev/null || true)
fi
if [[ -z "${CVC5:-}" || ! -x "$CVC5" ]]; then
  bash "$repo/scripts/install-cvc5.sh"
  CVC5=$(find "${CVC5_INSTALL_DIR:-$default_root}" -type f -name cvc5 -print -quit)
fi
export CVC5
"$CVC5" --version
cd "$repo"
cargo test -p pru-policy --features symcc --test symcc -- --nocapture
