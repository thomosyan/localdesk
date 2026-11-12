#!/usr/bin/env bash
# Cross-build the Windows GNU client from Apple Silicon macOS.
# Requires: brew install mingw-w64
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "This helper is intended to run on macOS." >&2
  exit 1
fi

if ! command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
  echo "Missing MinGW-w64. Install it with: brew install mingw-w64" >&2
  exit 1
fi

rustup target add x86_64-pc-windows-gnu
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
  cargo build --release -p localdesk-windows-client --target x86_64-pc-windows-gnu
