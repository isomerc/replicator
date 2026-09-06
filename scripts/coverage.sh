#!/usr/bin/env bash
# Run the Rust test suite under llvm-cov and print a per-file summary.
#
#   scripts/coverage.sh            # summary table
#   scripts/coverage.sh --html     # also write + report an HTML report
#   scripts/coverage.sh --open     # HTML report, opened in a browser
#
# Requires the flake devshell (`nix develop` / direnv), which supplies
# cargo-llvm-cov and the llvm-tools-preview rustup component.
set -euo pipefail

cd "$(dirname "$0")/../src-tauri"

if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
  echo "cargo-llvm-cov not found - enter the devshell first (nix develop)." >&2
  exit 1
fi

# The Tauri command layer, the OS keyring binding and the ESI HTTP
# client can't run without an app handle, a real keychain or the
# network. They're thin shims over logic that IS covered, so counting
# them would just depress the number without pointing at real risk.
IGNORE='(lib\.rs|state\.rs|keychain\.rs|testutil\.rs)$'

case "${1:-}" in
  --html)
    cargo llvm-cov --lib --ignore-filename-regex "$IGNORE" --html
    echo "report: src-tauri/target/llvm-cov/html/index.html"
    ;;
  --open)
    cargo llvm-cov --lib --ignore-filename-regex "$IGNORE" --open
    ;;
  *)
    cargo llvm-cov --lib --ignore-filename-regex "$IGNORE" --summary-only
    ;;
esac
